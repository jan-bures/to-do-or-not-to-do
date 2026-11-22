//! The checked-in exports must match what the token tables generate. To regenerate after changing a token:
//!
//! ```sh
//! TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated
//! ```

use std::path::PathBuf;

fn generated_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("generated")
}

fn check(file: &str, expected: String) {
    let path = generated_dir().join(file);
    // Only the documented value regenerates; `0` or an empty value must not silently pass a stale file.
    if std::env::var("TODO_DESIGN_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(generated_dir()).expect("create generated/");
        std::fs::write(&path, &expected).expect("write generated file");
        return;
    }
    let actual = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        actual == expected,
        "{} is stale. Regenerate it with: TODO_DESIGN_BLESS=1 cargo test -p todo-design --test generated",
        path.display()
    );
}

#[test]
fn tokens_css_is_current() {
    check("tokens.css", todo_design::export::css());
}

#[test]
fn tokens_json_is_current() {
    check("tokens.json", todo_design::export::json());
}

#[test]
fn json_is_valid_and_complete() {
    let v: serde_json::Value =
        serde_json::from_str(&todo_design::export::json()).expect("valid JSON");
    assert_eq!(v["colors"]["light"]["rooms"]["my-day"]["pane"], "#fefbf9");
    assert_eq!(v["colors"]["dark"]["rooms"]["lagoon"]["sky"], "#0c2220");
    assert_eq!(v["colors"]["dusk"]["my-day"]["top"], "#0f1727");
    let title = &v["type"]["room-title"];
    assert_eq!(title["family"], "Geist");
    assert_eq!(title["weight"], 600);
    assert_eq!(title["size"], 42.0);
    assert_eq!(title["lineHeight"], 48.0);
    let eyebrow = &v["type"]["eyebrow-day"];
    assert_eq!(eyebrow["family"], "Geist");
    assert_eq!(eyebrow["weight"], 500);
    assert_eq!(eyebrow["italic"], false);
    assert_eq!(v["type"]["eyebrow-meta"]["family"], "Geist Mono");
    assert_eq!(v["metrics"]["desktop"]["row"], 56.0);
    // The round checkbox is part of both exports, so no client hard-codes its shape.
    assert_eq!(v["radius"]["checkbox"], "circle");
    assert!(todo_design::export::css().contains("--radius-checkbox: 50%;"));
}

#[test]
fn a_checked_box_on_a_selected_row_meets_3_to_1() {
    use todo_design::{Theme, palette_scaled};
    for theme in [Theme::Light, Theme::Dark] {
        for k in [1.0, 0.85] {
            for hue in 0..360 {
                let p = palette_scaled(theme, f64::from(hue), k);
                let ratio = p.check_selected.contrast(p.wash);
                assert!(ratio >= 3.0, "{theme:?} hue {hue} k {k}: {ratio:.2}");
            }
        }
    }
}
