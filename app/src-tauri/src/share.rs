//! The Android share-target bridge (M5 slice 2).
//!
//! `WritanPlugin.kt` receives `ACTION_SEND` intents and parks the shared text
//! in SharedPreferences until the app asks for it. The `take_shared` command
//! drains that slot once a vault is open, so a share that arrives before the
//! first run is set up is not lost.

use tauri::AppHandle;

/// Text shared into the app, as the Kotlin plugin reports it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SharePayload {
    pub text: String,
    #[serde(default)]
    pub subject: Option<String>,
}

/// The registered Kotlin plugin handle; managed on Android only.
#[cfg(target_os = "android")]
pub struct ShareBridge<R: tauri::Runtime>(pub tauri::plugin::PluginHandle<R>);

/// Register the in-repo Kotlin plugin. Inert off Android.
pub fn init<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R> {
    let builder = tauri::plugin::Builder::<R>::new("writan");
    #[cfg(target_os = "android")]
    let builder = builder.setup(|app, api| {
        use tauri::Manager;

        let handle = api.register_android_plugin("com.writan.desktop", "WritanPlugin")?;
        app.manage(ShareBridge(handle));
        Ok(())
    });
    builder.build()
}

/// Take the pending share, if any, clearing it in the native layer.
#[cfg(target_os = "android")]
pub async fn take_pending<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<Option<SharePayload>, String> {
    use tauri::Manager;

    let bridge = app
        .try_state::<ShareBridge<R>>()
        .ok_or_else(|| "share plugin is not registered".to_string())?;
    bridge
        .0
        .run_mobile_plugin_async::<Option<SharePayload>>("take", serde_json::json!({}))
        .await
        .map_err(|e| format!("share plugin error: {e}"))
}

/// Desktop has no share target; nothing is ever pending.
#[cfg(not(target_os = "android"))]
pub async fn take_pending<R: tauri::Runtime>(
    _app: &AppHandle<R>,
) -> Result<Option<SharePayload>, String> {
    Ok(None)
}
