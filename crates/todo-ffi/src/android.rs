//! Android start-up: hands the app's `Context` to `rustls-platform-verifier`, which reqwest uses to check
//! server certificates against the system trust store. Kotlin calls `NativeInit.init(context)` once in
//! `Application.onCreate`, before any request.

use jni::EnvUnowned;
use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject};

/// `app.todoornot.core.NativeInit.init(Context)`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_todoornot_core_NativeInit_init<'caller>(
    mut env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    context: JObject<'caller>,
) {
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .resolve::<ThrowRuntimeExAndDefault>();
}
