package com.writan.desktop

import android.content.Context
import android.util.Log
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject

/**
 * One background sync pass (M5 slice 2c, ADR-0010 §7).
 *
 * It hands the app-data directory to Rust, which decides what the pass means
 * and replies with JSON: `{"ok":true,"report":…}` for a finished run,
 * `{"ok":false,"retry":true,"reason":…}` when a later period should try again
 * (offline, target busy), and `{"ok":false,"retry":false,"reason":"disabled"}`
 * when the setting is gone — then this worker cancels the job.
 */
class SyncWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
  override fun doWork(): Result {
    // The Keystore-backed keychain needs the Android context before its first
    // entry; MainActivity is not around when only this job wakes the process.
    Keyring.initializeNdkContext(applicationContext)
    val reply = try {
      SyncBridge.run(applicationContext.dataDir.absolutePath)
    } catch (e: Throwable) {
      Log.w(TAG, "sync bridge failed", e)
      return Result.retry()
    }
    val json = try {
      JSONObject(reply)
    } catch (_: Exception) {
      Log.w(TAG, "sync bridge replied with junk: $reply")
      return Result.retry()
    }
    if (json.optBoolean("ok", false)) {
      Log.i(TAG, "background sync finished: ${json.optJSONObject("report")}")
      return Result.success()
    }
    val reason = json.optString("reason", "unknown")
    if (reason == "disabled") {
      Log.i(TAG, "background sync is off in config.json; cancelling the job")
      SyncWork.stop(applicationContext)
      return Result.success()
    }
    Log.w(TAG, "background sync skipped: $reason")
    return if (json.optBoolean("retry", true)) Result.retry() else Result.failure()
  }

  private companion object {
    const val TAG = "WritanSync"
  }
}
