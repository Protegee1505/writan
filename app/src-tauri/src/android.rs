//! Android-only JNI bridges (M5 slices 2b + 2c, ADR-0010 §5, §7).
//!
//! Tauri, tao and wry never initialize `ndk-context`, but the Keystore-backed
//! keyring store needs it before it can create a single entry. Kotlin
//! `MainActivity.onCreate` calls the `com.writan.desktop.Keyring` object,
//! which lands in [`Java_com_writan_desktop_Keyring_initializeNdkContext`];
//! the background worker asks again through the same object before its pass.
//!
//! `com.writan.desktop.SyncBridge.run` (the WorkManager path) also lives here:
//! it reuses the live app's workspace when the process is up, and otherwise
//! builds the same state from the app-data directory the worker passes in.

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject, JString};
use serde_json::json;
use tauri::{AppHandle, Manager, Wry};

/// The `applicationContext` from Kotlin, kept alive for the process.
static ANDROID_CONTEXT: OnceLock<GlobalRef> = OnceLock::new();

/// The live app handle, remembered from `setup`; the worker uses it to reuse
/// the already-open workspace instead of opening a second one.
static APP_HANDLE: OnceLock<AppHandle<Wry>> = OnceLock::new();

/// The periodic job and the immediate run can land together; one engine pass
/// at a time.
static WORKER_LOCK: Mutex<()> = Mutex::new(());

/// `com.writan.desktop.Keyring.initializeNdkContext(applicationContext)`.
///
/// Idempotent: `ndk-context` may only be initialized once per process, so a
/// later call (the M5 slice 2c worker asks again) is ignored.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_writan_desktop_Keyring_initializeNdkContext(
    env: JNIEnv,
    _class: JClass,
    context: JObject,
) {
    if ANDROID_CONTEXT.get().is_some() {
        return; // already initialized; do not leak another global reference
    }
    let Ok(global) = env.new_global_ref(context) else {
        return;
    };
    let context_ptr = global.as_obj().as_raw() as *mut c_void;
    let Ok(vm) = env.get_java_vm() else {
        return;
    };
    let vm_ptr = vm.get_java_vm_pointer() as *mut c_void;
    if ANDROID_CONTEXT.set(global).is_err() {
        return; // already initialized
    }
    // SAFETY: the VM pointer and the global reference above stay valid for the
    // lifetime of the process; `ndk-context` stores both.
    unsafe { ndk_context::initialize_android_context(vm_ptr, context_ptr) };
}

/// Remember the live app handle so the worker can reuse its open workspace.
pub fn remember_app_handle(app: &AppHandle<Wry>) {
    let _ = APP_HANDLE.set(app.clone());
}

/// Schedule or cancel the 15-minute WorkManager job through the Kotlin
/// `WritanPlugin`, which owns the WorkManager calls (ADR-0010 §7).
pub async fn set_background_sync(app: &AppHandle<Wry>, enabled: bool) -> Result<(), String> {
    let bridge = app
        .try_state::<crate::share::ShareBridge<Wry>>()
        .ok_or_else(|| "writan plugin is not registered".to_string())?;
    bridge
        .0
        .run_mobile_plugin_async::<serde_json::Value>("background", json!({ "enabled": enabled }))
        .await
        .map(|_| ())
        .map_err(|e| format!("background sync plugin error: {e}"))
}

/// `com.writan.desktop.SyncBridge.run(dataDir)` — one sync pass for the
/// WorkManager job. Replies with JSON (see [`worker_disabled`] and friends);
/// the worker maps `ok`/`retry` onto `Result.success/retry/failure`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_writan_desktop_SyncBridge_run(
    mut env: JNIEnv,
    _this: JObject,
    data_dir: JString,
) -> jni::sys::jstring {
    let dir = env
        .get_string(&data_dir)
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_default();
    let reply = run_worker(&dir);
    env.new_string(reply)
        .map(|value| value.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

/// One engine pass, reusing the live app when this process is it and building
/// the same state from `data_dir` otherwise (the process was started for the
/// job alone, ADR-0010 §7).
fn run_worker(data_dir: &str) -> String {
    let _guard = WORKER_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    if let Some(app) = APP_HANDLE.get() {
        let state = app.state::<crate::state::AppState>();
        return run_pass(state.inner());
    }
    let state = crate::state::AppState::new(PathBuf::from(data_dir));
    state.load_config();
    run_pass(&state)
}

fn run_pass(state: &crate::state::AppState) -> String {
    let mut inner = match state.inner.lock() {
        Ok(inner) => inner,
        Err(_) => return worker_error("state lock poisoned", true),
    };
    if !inner.config.background_sync || !inner.config.sync.enabled {
        // The job outlived the setting (the user opted out, sync was turned
        // off, or the vault changed): tell the worker to cancel itself.
        return worker_disabled();
    }
    match crate::commands::run_sync_locked(state, &mut inner) {
        Ok(report) => json!({ "ok": true, "report": report }).to_string(),
        Err(reason) => worker_error(&reason, true),
    }
}

fn worker_error(reason: &str, retry: bool) -> String {
    json!({ "ok": false, "retry": retry, "reason": reason }).to_string()
}

fn worker_disabled() -> String {
    json!({ "ok": false, "retry": false, "reason": "disabled" }).to_string()
}
