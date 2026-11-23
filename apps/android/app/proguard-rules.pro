# JNA resolves classes, callbacks and Structure fields by name from native code.
-keep class com.sun.jna.** { *; }
# The portable JNA classes also expose desktop AWT entry points; Android never calls them.
-dontwarn java.awt.Component
-dontwarn java.awt.GraphicsEnvironment
-dontwarn java.awt.HeadlessException
-dontwarn java.awt.Window
-keep class app.todoornot.core.** extends com.sun.jna.Structure { *; }
-keep class app.todoornot.core.** implements com.sun.jna.Callback { *; }
-keep interface app.todoornot.core.** extends com.sun.jna.Callback { *; }
-keep interface app.todoornot.core.** extends com.sun.jna.Library { *; }
# The Rust platform verifier reaches this package over JNI.
-keep class org.rustls.platformverifier.** { *; }
