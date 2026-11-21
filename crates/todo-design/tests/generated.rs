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
    if std::env::var_os("TODO_DESIGN_BLESS").is_some() {
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
    assert_eq!(v["type"]["room-title"]["family"], "Fraunces Display");
    assert_eq!(v["metrics"]["desktop"]["row"], 56.0);
}
