package com.writan.desktop

import android.content.Context

/**
 * Hands the Android application context to Rust before anything needs it.
 *
 * Tauri never initializes the `ndk-context` crate, but the Keystore-backed
 * keyring store (ADR-0010 §4–5) asks for it when it creates its first entry.
 * [initializeNdkContext] is idempotent: only the first call reaches the native
 * side, so [SyncWorker] (M5 slice 2c) may call it again.
 */
object Keyring {
  init {
    System.loadLibrary("writan_app_lib")
  }

  external fun initializeNdkContext(context: Context)
}
