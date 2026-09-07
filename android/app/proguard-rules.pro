# Release minification is disabled in Phase 1. Keep the JNI owner/name stable if R8 is enabled later.
-keep class com.framescope.app.data.RustBridge { *; }
