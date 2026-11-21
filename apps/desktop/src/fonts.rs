//! Bundled fonts, loaded from files at startup (never system fonts).
//!
//! The directory is resolved in this order:
//! 1. `--fonts-dir`;
//! 2. the `TODO_FONTS_DIR` environment variable;
//! 3. an `assets/fonts` directory next to the executable or in any directory above it (the installed
//!    layout, and the repository's shared assets directory once the design tokens land);
//! 4. a `target/fonts` directory above the executable, which `scripts/fetch_fonts.py` fills in the meantime.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use gpui::App;

use crate::theme::font;

pub const ENV_VAR: &str = "TODO_FONTS_DIR";

/// Every directory that may hold the fonts, in priority order.
pub fn candidate_dirs(
    explicit: Option<&Path>,
    env: Option<&Path>,
    exe: Option<&Path>,
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = explicit
        .into_iter()
        .chain(env)
        .map(Path::to_path_buf)
        .collect();
    if let Some(dir) = exe.and_then(Path::parent) {
        out.extend(dir.ancestors().map(|a| a.join("assets").join("fonts")));
        out.extend(dir.ancestors().map(|a| a.join("target").join("fonts")));
    }
    out
}

fn is_font(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
}

fn font_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_font(p))
        .collect();
    files.sort();
    files
}

/// The first candidate directory that contains font files.
pub fn find_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    let env = std::env::var_os(ENV_VAR).map(PathBuf::from);
    let exe = std::env::current_exe().ok();
    candidate_dirs(explicit, env.as_deref(), exe.as_deref())
        .into_iter()
        .find(|d| !font_files(d).is_empty())
}

/// Loads every font in the fonts directory into GPUI's text system. Returns the families the app uses
/// that no loaded file provides (empty when all is well).
pub fn load(cx: &App, explicit: Option<&Path>) -> Vec<&'static str> {
    let families = [font::DISPLAY, font::UI, font::MONO];
    let Some(dir) = find_dir(explicit) else {
        tracing::warn!("no bundled fonts found; set --fonts-dir or {ENV_VAR}");
        return families.to_vec();
    };
    let bytes: Vec<Cow<'static, [u8]>> = font_files(&dir)
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .map(Cow::Owned)
        .collect();
    tracing::debug!("loading {} font files from {}", bytes.len(), dir.display());
    if let Err(e) = cx.text_system().add_fonts(bytes) {
        tracing::warn!("loading fonts from {} failed: {e:#}", dir.display());
    }
    let names = cx.text_system().all_font_names();
    families
        .into_iter()
        .filter(|family| !names.iter().any(|n| n == family))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_then_env_then_assets_then_target() {
        let exe = PathBuf::from("/repo/apps/desktop/target/release/todo-desktop.exe");
        let dirs = candidate_dirs(Some(Path::new("/x")), Some(Path::new("/env")), Some(&exe));
        assert_eq!(dirs[0], PathBuf::from("/x"));
        assert_eq!(dirs[1], PathBuf::from("/env"));
        assert_eq!(
            dirs[2],
            PathBuf::from("/repo/apps/desktop/target/release/assets/fonts")
        );
        let repo_assets = dirs
            .iter()
            .position(|d| d == Path::new("/repo/assets/fonts"));
        let target_fonts = dirs
            .iter()
            .position(|d| d == Path::new("/repo/apps/desktop/target/fonts"));
        assert!(repo_assets.is_some() && target_fonts.is_some());
        // The shared assets directory wins over the stopgap download.
        assert!(repo_assets < target_fonts);
    }

    #[test]
    fn without_an_executable_only_explicit_dirs_remain() {
        assert_eq!(candidate_dirs(None, None, None), Vec::<PathBuf>::new());
        assert_eq!(
            candidate_dirs(None, Some(Path::new("/env")), None),
            vec![PathBuf::from("/env")]
        );
    }

    #[test]
    fn only_font_files_are_picked_up() {
        let dir = std::env::temp_dir().join(format!("todo-fonts-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        for f in ["A.ttf", "B.OTF", "OFL.txt", "c.woff2"] {
            std::fs::write(dir.join(f), b"x").expect("write");
        }
        let names: Vec<String> = font_files(&dir)
            .iter()
            .filter_map(|p| p.file_name()?.to_str().map(String::from))
            .collect();
        assert_eq!(names, vec!["A.ttf", "B.OTF"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
