//! Pairing a device against an in-process `todo-server`, then syncing with the token it gets.
#![cfg(feature = "native")]
#![allow(clippy::unwrap_used)]

mod server;

use server::*;
use std::time::Duration;
use todo_client::native::HttpTransport;
use todo_client::pairing::{self, PairError, Polled};
use todo_client::{MemoryStorage, Transport};
use todo_core::command::Placement;
use todo_core::ids::ListId;
use todo_core::{Actor, Command};

const WAIT: Duration = Duration::from_secs(20);

async fn paired_server() -> TestServer {
    TestServer::start_with(|c| {
        c.public_origin = Some("https://todo.example.com".into());
    })
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_approved_pairing_yields_a_token_that_syncs() {
    let server = paired_server().await;
    let anon = HttpTransport::anonymous(&server.url()).unwrap();
    let started = pairing::start(&anon, "Test desktop").await.unwrap();
    assert!(
        started
            .verification_uri
            .starts_with("https://todo.example.com/pair?code=")
    );
    assert!(started.verification_uri.ends_with(&started.user_code));

    // Not decided yet.
    let polled = pairing::poll(&anon, &started.poll_token).await.unwrap();
    assert!(matches!(polled, Polled::Pending { .. }));

    // The owner approves in the browser (here: straight on the server).
    let auth = &server.running.authsvc;
    let (_, approver_token) = auth.create_device("Approver", None).unwrap();
    let approver = HttpTransport::new(&server.url(), &approver_token).unwrap();
    let at = auth
        .pairings
        .pending(&started.user_code, auth.now())
        .unwrap()
        .created_at;
    todo_client::device_pairing::review(&approver, &started.user_code, Some(at))
        .await
        .unwrap();
    let Polled::Approved { device_id, token } =
        pairing::poll(&anon, &started.poll_token).await.unwrap()
    else {
        panic!("expected approval");
    };
    // The token is handed out exactly once.
    assert_eq!(
        pairing::poll(&anon, &started.poll_token).await,
        Err(PairError::Expired)
    );

    // The new device syncs with the token, as the actor the server named.
    let actor = Actor::Owner { device: device_id };
    let device = client(
        HttpTransport::new(&server.url(), &token).unwrap(),
        MemoryStorage::new(),
        actor.clone(),
    );
    let other = client(server.transport(TOKEN_A), MemoryStorage::new(), alpha());
    let (hd, ho) = (spawn(&device), spawn(&other));
    let list = ListId::from_uuid(device.new_uuid());
    device
        .submit(Command::CreateList {
            id: list,
            collection: None,
            name: "Paired".into(),
            color: None,
            placement: Placement::Last,
        })
        .unwrap();
    eventually("the other client sees the list", WAIT, || {
        other.view().lists.contains_key(&list)
    })
    .await;
    assert_eq!(server.state().lists[&list].created.by, actor);
    stop(&device, hd).await;
    stop(&other, ho).await;
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_or_unknown_pairing_gives_no_token() {
    let server = paired_server().await;
    let anon = HttpTransport::anonymous(&server.url()).unwrap();
    let started = pairing::start(&anon, "Test desktop").await.unwrap();
    let auth = &server.running.authsvc;
    auth.pairings.deny(&started.user_code, auth.now());
    assert_eq!(
        pairing::poll(&anon, &started.poll_token).await,
        Ok(Polled::Denied)
    );
    assert_eq!(
        pairing::poll(&anon, "no-such-poll-token").await,
        Err(PairError::Expired)
    );
    // A bad device name is refused with the server's reason.
    assert!(matches!(
        pairing::start(&anon, "").await,
        Err(PairError::Refused { status: 422, .. })
    ));
    // The anonymous transport sends no token: the API proper refuses it.
    let res = anon
        .send(todo_client::platform::Request::get("/api/v1/lists"))
        .await
        .unwrap();
    assert_eq!(res.status, 401);
    server.stop().await;
}
