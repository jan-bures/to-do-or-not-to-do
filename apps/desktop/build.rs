//! Embeds the Windows resources: the app icon and the version information (`resources/app.rc`).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/generated/todo.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let macros = [
        format!("VERSION_MAJOR={}", var("CARGO_PKG_VERSION_MAJOR")),
        format!("VERSION_MINOR={}", var("CARGO_PKG_VERSION_MINOR")),
        format!("VERSION_PATCH={}", var("CARGO_PKG_VERSION_PATCH")),
        format!("VERSION_STRING=\"{}\"", var("CARGO_PKG_VERSION")),
    ];
    embed_resource::compile("resources/app.rc", macros)
        .manifest_required()
        .expect("failed to compile the Windows resources");
}
