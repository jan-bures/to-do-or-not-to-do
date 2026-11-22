//! Offline recovery uses the same database lock and setup flow as the running server.
#![allow(clippy::unwrap_used)]

mod common;

use common::web::{Env, SITE};
use reqwest::{Method, StatusCode};
use serde_json::json;
use std::process::{Command, Output};
use todo_core::{AccessLevel, AgentId};
use todo_server::Config;
use todo_server::auth::service::SETUP_CODE_FILE;
use todo_server::auth::store::ShareTarget;

fn reset(config: &Config, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_todo-server"))
        .env_remove("TODO_SERVER_CONFIG")
        .env("TODO_SERVER_DATA_DIR", &config.data_dir)
        .env("TODO_SERVER_PUBLIC_ORIGIN", SITE)
        .arg("reset-owner")
        .args(args)
        .output()
        .unwrap()
}

#[tokio::test]
async fn offline_reset_recovers_the_owner_without_rolling_back_tasks() {
    let env = Env::start().await;
    let mut owner = env.owner().await;
    let original_session = owner.cookie.clone();
    let (ceremony, _) = owner.login_assertion().await;
    let list = env.t.create_list("Preserved list").await;
    let task = env.t.create_task(&list, "Preserved task", json!({})).await;
    let svc = &env.t.server.authsvc;
    let (_, agent) = svc
        .create_agent(
            &AgentId::new("helper").unwrap(),
            AccessLevel::Read,
            None,
            None,
            None,
        )
        .unwrap();
    let share = svc
        .create_share(ShareTarget::List(list.parse().unwrap()), None)
        .unwrap();
    let (device, token) = svc.create_device("Phone", None).unwrap();
    let invite = owner
        .post_json("/api/v1/pair/invites", json!({}))
        .await
        .json();
    let pending = env
        .browser()
        .post_json(
            "/api/v1/pair/start",
            json!({"device_name": "Pending phone"}),
        )
        .await
        .json();
    assert_eq!(
        owner
            .post_form(
                "/pair/approve",
                &[("code", pending["user_code"].as_str().unwrap())]
            )
            .await
            .status,
        StatusCode::OK
    );
    assert!(ceremony.is_string());
    let config = env.t.config.clone();
    let denied = reset(&config, &[]);
    assert!(!denied.status.success() && String::from_utf8_lossy(&denied.stderr).contains("--yes"));
    let locked = reset(&config, &["--yes"]);
    assert!(!locked.status.success(), "{locked:?}");
    assert!(
        String::from_utf8_lossy(&locked.stderr).contains("stop it first"),
        "{locked:?}"
    );
    assert!(svc.owner_ready().unwrap());

    let mut code_file = String::new();
    let env = env.restart_with(|db| {
        let conn = rusqlite::Connection::open(db).unwrap();
        conn.execute_batch("CREATE TRIGGER no_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT, 'audit down'); END;").unwrap();
        let failure = reset(&config, &["--yes"]);
        assert!(!failure.status.success(), "{failure:?}");
        for table in ["passkeys", "sessions", "ceremonies"] {
            let n = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap();
            assert_eq!(n, 1, "reset rolls back {table} when audit fails");
        }
        let revoked: Option<i64> = conn.query_row("SELECT revoked_at FROM devices WHERE id = ?1", [device.as_str()], |r| r.get(0)).unwrap();
        assert_eq!(revoked, None);
        conn.execute_batch("DROP TRIGGER no_audit;").unwrap();
        let reset = reset(&config, &["--yes"]);
        assert!(reset.status.success(), "{reset:?}");
        let output = String::from_utf8_lossy(&reset.stdout);
        assert!(output.contains("setup-code.txt") && output.contains("Agent tokens and share links are unchanged"));
        code_file = std::fs::read_to_string(config.data_dir.join(SETUP_CODE_FILE)).unwrap();
        for table in ["passkeys", "sessions", "ceremonies"] {
            let n = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap();
            assert_eq!(n, 0, "reset clears {table}");
        }
    }).await;

    let code = code_file.lines().next().unwrap();
    assert_eq!(
        env.take_code().as_deref(),
        Some(code),
        "the reset code survives the first start"
    );
    assert!(!env.t.server.authsvc.owner_ready().unwrap());
    let rows = env.t.server.authsvc.store().audit_page(100).unwrap();
    let resets = rows
        .iter()
        .filter(|row| row.kind == "owner_reset")
        .collect::<Vec<_>>();
    assert_eq!(resets.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&resets[0].detail).unwrap(),
        json!({
            "actor": "reset-owner CLI", "devices_revoked": 1, "passkeys_removed": 1, "sessions_removed": 1,
        })
    );

    let mut old_browser = env.browser();
    old_browser.cookie = original_session;
    assert!(old_browser.get("/admin").await.status.is_redirection());
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", &token, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        env.browser()
            .post_json(
                "/api/v1/pair/redeem",
                json!({"secret": invite["code"], "device_name": "Other"})
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.browser()
            .post_json(
                "/api/v1/pair/poll",
                json!({"poll_token": pending["poll_token"]})
            )
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.api(Method::GET, &format!("/api/v1/tasks/{task}"), &agent, None)
            .await
            .status,
        StatusCode::OK
    );
    assert!(env.t.server.authsvc.share(&share).unwrap().is_some());

    let mut recovered = env.browser();
    let p = recovered.setup(code).await;
    assert_eq!(p.status, StatusCode::OK, "{}", p.body);
    assert!(env.t.server.authsvc.owner_ready().unwrap());
    assert!(!config.data_dir.join(SETUP_CODE_FILE).exists());
    assert_eq!(env.t.server.authsvc.store().passkeys().unwrap().len(), 1);
    assert!(old_browser.get("/admin").await.status.is_redirection());
    assert!(
        env.t
            .get_ok(&format!("/api/v1/tasks/{task}"))
            .await
            .to_string()
            .contains("Preserved task")
    );
}
