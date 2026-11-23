//! Embeds the Windows resources: the app icon and the version information (`resources/app.rc`).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=../../assets/brand/generated/todo.ico");
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    println!("cargo:rerun-if-env-changed=TODO_DESKTOP_UPDATE_FEED");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root: toml::Table = include_str!("../../Cargo.toml")
        .parse()
        .expect("invalid root Cargo.toml");
    let repository = root["workspace"]["package"]["repository"]
        .as_str()
        .expect("missing workspace.package.repository");
    let feed = std::env::var("TODO_DESKTOP_UPDATE_FEED").unwrap_or_else(|_| repository.to_owned());
    assert!(!feed.contains(['\n', '\r']), "invalid desktop update feed");
    println!("cargo:rustc-env=TODO_DESKTOP_UPDATE_FEED={}", feed.trim());

    let var = |name: &str| std::env::var(name).unwrap_or_default();
    let macros = [
        format!("VERSION_MAJOR={}", var("CARGO_PKG_VERSION_MAJOR")),
        format!("VERSION_MINOR={}", var("CARGO_PKG_VERSION_MINOR")),
        format!("VERSION_PATCH={}", var("CARGO_PKG_VERSION_PATCH")),
        format!("VERSION_STRING=\"{}\"", var("CARGO_PKG_VERSION")),
    ];
    // For the exe, and for the test binaries too: tests load the icon resource the way the tray does.
    embed_resource::compile_for_everything("resources/app.rc", macros)
        .manifest_required()
        .expect("failed to compile the Windows resources");
}
