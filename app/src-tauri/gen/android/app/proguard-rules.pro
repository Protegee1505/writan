# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# M5 slice 2: reached by name only (JNI + Tauri reflection), so keep them.
-keep class com.writan.desktop.WritanPlugin { *; }
-keep class com.writan.desktop.Keyring { *; }
-keep class com.writan.desktop.SyncBridge { *; }
-keep class com.writan.desktop.SyncWork { *; }
-keep class com.writan.desktop.SyncWorker { *; }

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile