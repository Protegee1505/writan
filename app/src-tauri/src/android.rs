//! Android-only JNI bridges (M5 slice 2b, ADR-0010 §5).
//!
//! Tauri, tao and wry never initialize `ndk-context`, but the Keystore-backed
//! keyring store needs it before it can create a single entry. Kotlin
//! `MainActivity.onCreate` calls the `com.writan.desktop.Keyring` object,
//! which lands in [`Java_com_writan_desktop_Keyring_initializeNdkContext`].

use std::ffi::c_void;
use std::sync::OnceLock;

use jni::JNIEnv;
use jni::objects::{GlobalRef, JClass, JObject};

/// The `applicationContext` from Kotlin, kept alive for the process.
static ANDROID_CONTEXT: OnceLock<GlobalRef> = OnceLock::new();

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
