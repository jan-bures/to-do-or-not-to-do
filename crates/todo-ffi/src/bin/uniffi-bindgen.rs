//! The UniFFI bindings generator, built from the same `uniffi` version as the library so the generated
//! Kotlin always matches the scaffolding.

fn main() {
    uniffi::uniffi_bindgen_main();
}
