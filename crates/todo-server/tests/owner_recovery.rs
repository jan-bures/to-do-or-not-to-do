//! Offline recovery uses the same database lock and setup flow as the running server.
#![allow(clippy::unwrap_used)]

mod common;

use common::{
    TestServer,
    web::{Env, SITE},
};
use reqwest::{Method, StatusCode};
use serde_json::json;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;
use todo_core::{AccessLevel, AgentId};
use todo_server::Config;
use todo_server::auth::service::SETUP_CODE_FILE;
use todo_server::auth::store::ShareTarget;

const RECOVERY_META: &str = "recovery_code_sha256";

fn reset_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_todo-server"));
    command
        .env_remove("TODO_SERVER_CONFIG")
        .env_remove("TODO_SERVER_DATA_DIR")
        .env("TODO_SERVER_PUBLIC_ORIGIN", SITE)
        .arg("reset-owner");
    command
}

fn reset(config: &Config, args: &[&str]) -> Output {
    reset_command()
        .env("TODO_SERVER_DATA_DIR", &config.data_dir)
        .args(args)
        .output()
        .unwrap()
}

fn setup_file_code(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn reset_in_an_empty_relative_data_directory_creates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let output = reset_command()
        .current_dir(dir.path())
        .arg("--yes")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("requires an existing database") && stderr.contains("todo.sqlite3"));
    assert!(data.read_dir().unwrap().next().is_none());
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
    svc.create_agent(
        &AgentId::new("expired").unwrap(),
        AccessLevel::Read,
        None,
        Some(0),
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

    let setup_file = config.data_dir.join(SETUP_CODE_FILE);
    let mut code = String::new();
    let env = env.restart_with(|db| {
        let conn = rusqlite::Connection::open(db).unwrap();
        conn.execute_batch("CREATE TRIGGER no_audit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT, 'audit down'); END;").unwrap();
        let failure = reset(&config, &["--yes", "--revoke-agents", "--revoke-shares"]);
        assert!(!failure.status.success(), "{failure:?}");
        for table in ["passkeys", "sessions", "ceremonies"] {
            let n = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap();
            assert_eq!(n, 1, "reset rolls back {table} when audit fails");
        }
        let revoked: Option<i64> = conn.query_row("SELECT revoked_at FROM devices WHERE id = ?1", [device.as_str()], |r| r.get(0)).unwrap();
        assert_eq!(revoked, None);
        for table in ["agent_tokens", "share_links"] {
            let n = conn.query_row(&format!("SELECT count(*) FROM {table} WHERE revoked_at IS NOT NULL"), [], |r| r.get::<_, i64>(0)).unwrap();
            assert_eq!(n, 0, "reset rolls back {table} when audit fails");
        }
        conn.execute_batch("DROP TRIGGER no_audit;").unwrap();
        let reset = reset_command()
            .current_dir(&config.data_dir)
            .env("TODO_SERVER_DATA_DIR", ".")
            .arg("--yes")
            .output()
            .unwrap();
        assert!(reset.status.success(), "{reset:?}");
        let output = String::from_utf8_lossy(&reset.stdout);
        for path in [db, &setup_file] {
            assert!(output.contains(path.canonicalize().unwrap().to_str().unwrap()), "{output}");
        }
        assert!(output.contains("Passkeys removed: 1; browser sessions removed: 1; paired devices revoked: 1; invites removed: 0"));
        assert!(output.contains("Active agent tokens remaining: 1 (revoked: 0). Active share links remaining: 1 (revoked: 0)."));
        assert!(output.contains("POST /api/v1/admin/agents/{id}/revoke"));
        assert!(output.contains("POST /api/v1/admin/shares/{id}/revoke"));
        code = setup_file_code(&setup_file);
        assert!(!output.contains(&code));
        assert!(!String::from_utf8_lossy(&reset.stderr).contains(&code));
        for table in ["passkeys", "sessions", "ceremonies"] {
            let n = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap();
            assert_eq!(n, 0, "reset clears {table}");
        }
    }).await;

    assert_eq!(
        env.take_code().as_deref(),
        Some(code.as_str()),
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
            "agents_revoked": 0, "shares_revoked": 0, "agents_remaining": 1, "shares_remaining": 1,
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
    let p = recovered.setup(&code).await;
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

#[tokio::test]
async fn reset_flags_revoke_credentials_independently_and_record_them_in_the_audit() {
    let env = Env::start().await;
    let list = env.t.create_list("List").await;
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
    let config = env.t.config.clone();
    let env = env.restart_with(|_| {
        let output = reset(&config, &["--yes", "--revoke-agents"]);
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Active agent tokens remaining: 0 (revoked: 1). Active share links remaining: 1 (revoked: 0)."));
    }).await;
    assert_eq!(
        env.api(Method::GET, "/api/v1/lists", &agent, None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert!(env.t.server.authsvc.share(&share).unwrap().is_some());
    let env = env.restart_with(|_| {
        let output = reset(&config, &["--yes", "--revoke-shares"]);
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Active agent tokens remaining: 0 (revoked: 0). Active share links remaining: 0 (revoked: 1)."));
    }).await;
    assert!(env.t.server.authsvc.share(&share).unwrap().is_none());
    let reset_counts: Vec<_> = env
        .t
        .server
        .authsvc
        .store()
        .audit_page(100)
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == "owner_reset")
        .map(|row| {
            let detail: serde_json::Value = serde_json::from_str(&row.detail).unwrap();
            (
                detail["agents_revoked"].as_u64().unwrap(),
                detail["shares_revoked"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(reset_counts, [(0, 1), (1, 0)]);
}

#[tokio::test]
async fn startup_rotates_an_expired_offline_code_and_consumes_its_hash() {
    let env = Env::start().await;
    let config = env.t.config.clone();
    let file = config.data_dir.join(SETUP_CODE_FILE);
    let offset = env.offset.clone();
    let mut expired = String::new();
    let env = env
        .restart_with(|_| {
            assert!(reset(&config, &["--yes"]).status.success());
            expired = setup_file_code(&file);
            offset.store(24 * 60 * 60 * 1000, std::sync::atomic::Ordering::SeqCst);
        })
        .await;
    let fresh = env.take_code().unwrap();
    assert!(fresh != expired);
    assert!(setup_file_code(&file) == fresh);
    assert!(
        env.t
            .server
            .authsvc
            .store()
            .meta(RECOVERY_META)
            .unwrap()
            .is_none()
    );
    let mut b = env.browser();
    let denied = b.setup(&expired).await;
    assert_eq!(denied.status, StatusCode::UNAUTHORIZED);
    assert_eq!(b.setup(&fresh).await.status, StatusCode::OK);
}

#[tokio::test]
async fn a_code_loaded_before_its_deadline_still_expires_24_hours_after_reset() {
    let env = Env::start().await;
    let config = env.t.config.clone();
    let offset = env.offset.clone();
    let mut code = String::new();
    let env = env
        .restart_with(|_| {
            assert!(reset(&config, &["--yes"]).status.success());
            code = setup_file_code(&config.data_dir.join(SETUP_CODE_FILE));
            offset.store(
                (24 * 60 - 5) * 60 * 1000,
                std::sync::atomic::Ordering::SeqCst,
            );
        })
        .await;
    let b = env.browser();
    let start = || b.post_json("/auth/setup/start", json!({ "code": code }));
    assert_eq!(start().await.status, StatusCode::OK);
    env.advance(Duration::from_secs(5 * 60));
    assert_eq!(start().await.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn startup_without_an_origin_discards_the_offline_code() {
    let (dir, mut config) = Env::start().await.t.stop().await;
    assert!(reset(&config, &["--yes"]).status.success());
    let setup_file = config.data_dir.join(SETUP_CODE_FILE);
    let offline_code = setup_file_code(&setup_file);
    config.public_origin = None;
    let t = TestServer::boot(dir, config).await;
    let svc = &t.server.authsvc;
    assert!(!svc.setup_open().unwrap());
    assert!(svc.store().meta(RECOVERY_META).unwrap().is_none());
    assert!(!setup_file.exists());

    let (dir, mut config) = t.stop().await;
    config.public_origin = Some(SITE.to_owned());
    let t = TestServer::boot(dir, config).await;
    assert!(t.server.authsvc.setup_open().unwrap());
    assert!(setup_file_code(&setup_file) != offline_code);
}

#[tokio::test]
async fn a_device_cascade_cannot_remove_the_last_passkey_but_can_revoke_with_zero_passkeys() {
    let env = Env::start().await;
    env.owner().await;
    let (device, _) = env.t.server.authsvc.create_device("Issuer", None).unwrap();
    let env = env
        .restart_with(|db| {
            // Model a legacy database whose only surviving passkey descends from this device.
            let conn = rusqlite::Connection::open(db).unwrap();
            assert_eq!(
                conn.execute(
                    "UPDATE passkeys SET issued_via_device = ?1",
                    [device.as_str()]
                )
                .unwrap(),
                1
            );
        })
        .await;
    let svc = &env.t.server.authsvc;
    let error = svc.revoke_device(device.as_str(), None).unwrap_err();
    assert_eq!(error.code, "last_passkey");
    assert!(error.message.contains("todo-server reset-owner"));
    assert_eq!(svc.store().passkeys().unwrap().len(), 1);
    let env = env
        .restart_with(|db| {
            let conn = rusqlite::Connection::open(db).unwrap();
            assert_eq!(conn.execute("DELETE FROM passkeys", []).unwrap(), 1);
        })
        .await;
    assert!(
        env.t
            .server
            .authsvc
            .revoke_device(device.as_str(), None)
            .unwrap()
    );
}
