package com.writan.desktop

/**
 * Rust entry points for the background worker (M5 slice 2c, ADR-0010 §7).
 *
 * `run` is one engine pass over the vault in `dataDir`; the library reuses
 * the live app's workspace when the process is up and otherwise opens the
 * same state from the directory alone. The reply is JSON — see `SyncWorker`.
 */
object SyncBridge {
  init {
    System.loadLibrary("writan_app_lib")
  }

  external fun run(dataDir: String): String
}
