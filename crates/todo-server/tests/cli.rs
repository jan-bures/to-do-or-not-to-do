//! The `database-path` subcommand, which deploy scripts use to find the database to copy before a
//! deploy: it must name exactly the file the server opens, and refuse a config the server would refuse.

use std::path::Path;
use std::process::{Command, Output};

fn database_path(cwd: &Path, config: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_todo-server"))
        .arg("--config")
        .arg(config)
        .arg("database-path")
        .current_dir(cwd)
        .env_remove("TODO_SERVER_CONFIG")
        .env_remove("TODO_SERVER_DATA_DIR")
        .output()
        .expect("run todo-server")
}

fn stdout_path(o: &Output) -> std::path::PathBuf {
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    std::path::PathBuf::from(String::from_utf8(o.stdout.clone()).expect("utf-8").trim())
}

#[test]
fn a_relative_data_dir_is_resolved_against_the_working_directory() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("server.toml");
    std::fs::write(&config, "data_dir = \"state\"\n").expect("write config");
    let cwd = dir.path().join("work");
    // The data directory exists only so both sides can be canonicalized for the comparison.
    std::fs::create_dir_all(cwd.join("state")).expect("mkdir");

    let path = stdout_path(&database_path(&cwd, &config));
    assert!(path.is_absolute(), "{}", path.display());
    assert_eq!(path.file_name(), Some("todo.sqlite3".as_ref()));
    let parent = path
        .parent()
        .expect("a parent")
        .canonicalize()
        .expect("exists");
    assert_eq!(parent, cwd.join("state").canonicalize().expect("exists"));
}

#[test]
fn an_absolute_data_dir_is_printed_as_is() {
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().join("data");
    let config = dir.path().join("server.toml");
    // TOML literal strings keep Windows backslashes as they are.
    std::fs::write(&config, format!("data_dir = '{}'\n", data.display())).expect("write config");

    let path = stdout_path(&database_path(dir.path(), &config));
    assert_eq!(path, data.join("todo.sqlite3"));
}

#[test]
fn an_invalid_config_fails_and_prints_no_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("server.toml");
    std::fs::write(&config, "time_zone = \"Mars/Olympus\"\n").expect("write config");

    let o = database_path(dir.path(), &config);
    assert!(!o.status.success());
    assert!(
        o.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );
}

#[test]
fn deployment_config_check_accepts_a_not_yet_switched_web_dir_but_rejects_unknown_sections() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("server.toml");
    let known = "data_dir = 'state'\n[web]\ndir = 'current/web'\n";
    std::fs::write(&config, known).expect("write config");
    assert_eq!(
        stdout_path(&database_path(dir.path(), &config)),
        dir.path().join("state/todo.sqlite3")
    );
    assert!(!dir.path().join("state").exists());

    std::fs::write(&config, format!("{known}\n[future]\nenabled = true\n")).expect("write config");
    let o = database_path(dir.path(), &config);
    assert!(!o.status.success());
    assert!(o.stdout.is_empty());
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown field `future`"));
}
