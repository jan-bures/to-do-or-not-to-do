package app.todoornot.core

import android.content.Context

/**
 * Loads the Rust core (`libtodo_ffi.so`) and hands it the app context, which its TLS stack needs to
 * check certificates against Android's trust store. Call [ensure] once, before any network request.
 */
object NativeInit {
    @Volatile
    private var done = false

    /** `Java_app_todoornot_core_NativeInit_init` in crates/todo-ffi/src/android.rs. */
    @JvmStatic
    private external fun init(context: Context)

    @Synchronized
    fun ensure(app: Context) {
        if (done) return
        System.loadLibrary("todo_ffi")
        init(app.applicationContext)
        done = true
    }
}
