package com.writan.desktop

import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    // The Keystore-backed keyring store needs the app context before the
    // webview setup (super.onCreate) can unlock sync (ADR-0010 §5).
    Keyring.initializeNdkContext(applicationContext)
    super.onCreate(savedInstanceState)
  }
}
