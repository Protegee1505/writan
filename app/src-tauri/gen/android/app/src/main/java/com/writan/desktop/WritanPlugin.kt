package com.writan.desktop

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONObject

/**
 * Receives Android share intents (`ACTION_SEND` with `text/plain`) and parks
 * the payload until the web app asks for it.
 *
 * The Rust `take_shared` command calls the [take] command through
 * `PluginHandle::run_mobile_plugin_async`, so no JS plugin permission needs to
 * be declared. The pending share lives in SharedPreferences: a share that
 * arrives before the vault is set up survives process death, and `take`
 * clears it exactly once.
 */
@TauriPlugin
class WritanPlugin(private val activity: Activity) : Plugin(activity) {
  private val prefs by lazy {
    activity.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
  }

  /** Cold start from a share: the SEND intent is the activity's launch intent. */
  override fun load(webView: WebView) {
    super.load(webView)
    handleSendIntent(activity.intent)
  }

  /** A share while the app is already running. */
  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    handleSendIntent(intent)
  }

  private fun handleSendIntent(intent: Intent?) {
    if (intent == null || intent.action != Intent.ACTION_SEND) return
    // The activity can be recreated around the same intent, so remember that
    // we already stored it (an extra survives on the Intent object).
    if (intent.getBooleanExtra(CONSUMED_FLAG, false)) return
    val text = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
    val subject = intent.getStringExtra(Intent.EXTRA_SUBJECT)
      ?: intent.getStringExtra(Intent.EXTRA_TITLE)
    val payload = JSONObject().apply {
      put("text", text)
      put("subject", subject ?: JSONObject.NULL)
    }
    prefs.edit().putString(KEY_PENDING, payload.toString()).apply()
    intent.putExtra(CONSUMED_FLAG, true)
  }

  /** Hand the pending share to Rust and clear it; `null` when none is pending. */
  @Command
  fun take(invoke: Invoke) {
    val raw = prefs.getString(KEY_PENDING, null)
    if (raw != null) prefs.edit().remove(KEY_PENDING).apply()
    if (raw == null) {
      invoke.resolve()
    } else {
      invoke.resolve(JSObject(raw))
    }
  }

  private companion object {
    const val PREFS = "writan-share"
    const val KEY_PENDING = "pending"
    const val CONSUMED_FLAG = "com.writan.desktop.share.consumed"
  }
}
