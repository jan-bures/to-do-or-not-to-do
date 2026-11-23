//! The assistant's local reads agree with REST on visible data, without using REST's full DTOs.
#![allow(clippy::unwrap_used)]

mod common;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::mcp::{args, grants, ok};
use common::{TestServer, id};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use todo_core::AccessLevel;
use todo_core::ids::ListId;
use todo_core::query::EvalContext;
use todo_core::time::Timestamp;
use todo_tools::assistant::{self, Projection, ReadError};

fn ids(page: &Value) -> Vec<&str> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn scoped_lists_and_queries_match_rest_and_mcp() {
    let server = TestServer::start().await;
    let inbox = server.create_list("Inbox").await;
    let hidden = server.create_list("Hidden").await;
    server
        .ok(json!({"cmd":"update_list","id":inbox,"features":{"workflow":true}}))
        .await;
    let early = server
        .create_task(
            &inbox,
            "Early",
            json!({"start_date":"2030-01-05", "status":"in_progress"}),
        )
        .await;
    let late = server
        .create_task(
            &inbox,
            "Late",
            json!({"start_date":"2030-01-07", "status":"in_progress"}),
        )
        .await;
    let done = server
        .create_task(&inbox, "Finished", json!({"start_date":"2030-01-01"}))
        .await;
    server.ok(json!({"cmd":"complete_task","id":done})).await;
    let private = server
        .create_task(
            &hidden,
            "Hidden",
            json!({"start_date":"2029-12-31","status":"in_progress"}),
        )
        .await;
    let (_, token) = common::mcp::agent(
        &server,
        "reader",
        &grants(AccessLevel::None, &[(&inbox, AccessLevel::Read)]),
        None,
    );
    let mcp = common::mcp::connect(&server, &token).await;
    let state = server.server.engine.state();
    let read = Projection::new(
        &state,
        &BTreeSet::from([inbox.parse().unwrap()]),
        EvalContext::utc(Timestamp::UNIX_EPOCH),
    );

    let (status, _, rest_lists) = server
        .request(Method::GET, "/api/v1/lists", Some(&token), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let local_lists = json!(
        read.list_lists(&assistant::LIST_LISTS.decode(args(json!({}))).unwrap())
            .unwrap()
    );
    let mcp_lists = ok(&mcp, "list_lists", json!({})).await;
    assert_eq!(ids(&local_lists), ids(&rest_lists));
    assert_eq!(ids(&local_lists), ids(&mcp_lists));
    assert_eq!(
        local_lists["items"][0]["features"],
        mcp_lists["items"][0]["features"]
    );
    assert_eq!(local_lists["items"][0]["open_count"], 2);
    assert_eq!(local_lists["items"][0]["completed_count"], 1);

    for (query, expected) in [
        (
            "status:in_progress start<=2030-01-07 sort:start",
            vec![early.as_str(), late.as_str()],
        ),
        ("done sort:status", vec![done.as_str()]),
    ] {
        let response = server
            .client
            .get(format!("{}/api/v1/tasks", server.url))
            .bearer_auth(&token)
            .query(&[("query", query)])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let rest: Value = response.json().await.unwrap();
        let external = ok(&mcp, "query_tasks", json!({"query":query})).await;
        let local = json!(
            read.query_tasks(
                &assistant::QUERY_TASKS
                    .decode(args(json!({"query":query})))
                    .unwrap()
            )
            .unwrap()
        );
        assert_eq!(ids(&rest), expected);
        assert_eq!(ids(&local), ids(&rest));
        assert_eq!(ids(&local), ids(&external));
        for (local, external) in local["items"]
            .as_array()
            .unwrap()
            .iter()
            .zip(external["items"].as_array().unwrap())
        {
            for field in ["list", "title", "status", "start_date", "due", "priority"] {
                assert_eq!(local[field], external[field], "{field}");
            }
        }
    }
    let query = "status:in_progress sort:start";
    let query_args = |cursor: Option<&str>| {
        assistant::QUERY_TASKS
            .decode(args(json!({"query":query,"limit":1,"cursor":cursor})))
            .unwrap()
    };
    let first = read.query_tasks(&query_args(None)).unwrap();
    assert_eq!(first.items[0].id.to_string(), early);
    let cursor = first.next_cursor.unwrap();
    let second = read.query_tasks(&query_args(Some(&cursor))).unwrap();
    assert_eq!(second.items[0].id.to_string(), late);
    assert!(second.next_cursor.is_none());

    let replace_id = |replacement: &str| {
        let body = URL_SAFE_NO_PAD.decode(&cursor).unwrap();
        URL_SAFE_NO_PAD.encode(
            String::from_utf8(body)
                .unwrap()
                .replace(&early, replacement),
        )
    };
    for unavailable in [private, id()] {
        let cursor = replace_id(&unavailable);
        assert!(matches!(
            read.query_tasks(&query_args(Some(&cursor))),
            Err(ReadError::InvalidCursor(_))
        ));
    }
}

#[tokio::test]
async fn scoped_reads_omit_excluded_references_and_do_not_search_notes() {
    let server = TestServer::start().await;
    let inbox = server.create_list("Inbox").await;
    let collection = id();
    server
        .ok(json!({"cmd":"create_collection","id":collection,"name":"Excluded Collection"}))
        .await;
    let hidden = id();
    server
        .ok(json!({"cmd":"create_list","id":hidden,"name":"Excluded List","collection":collection}))
        .await;
    let hidden_tag = id();
    server
        .ok(json!({"cmd":"create_tag","id":hidden_tag,"name":"Excludedtag"}))
        .await;
    let hidden_task = server
        .create_task(&hidden, "Excludedtitle", json!({"tags":[hidden_tag]}))
        .await;
    let live = server
        .create_task(&inbox, "Visible", json!({"notes":"Privatenote"}))
        .await;
    let step = id();
    server
        .ok(json!({"cmd":"create_step","id":step,"task":live,"title":"Visiblecheck"}))
        .await;
    let link = id();
    server.ok(json!({"cmd":"add_link","id":link,"task":live,"kind":{"type":"web","url":"https://example.com","title":"Private link"}})).await;
    let deleted = server.create_task(&inbox, "Deleted", json!({})).await;
    server
        .ok(json!({"cmd":"delete","id":{"kind":"task","id":deleted}}))
        .await;

    let live_id = live.parse().unwrap();
    let hidden_task_id = hidden_task.parse().unwrap();
    let allowed = BTreeSet::from([inbox.parse::<ListId>().unwrap()]);
    let state = server.server.engine.state();
    let mut state_with_ref = (*state).clone();
    // A reference on a visible row must not disclose its destination in another list.
    state_with_ref.tasks.get_mut(&live_id).unwrap().merged_into = Some(hidden_task_id);
    let ctx = EvalContext::utc(Timestamp::UNIX_EPOCH);
    let read = Projection::new(&state_with_ref, &allowed, ctx.clone());
    let query = |text: &str| {
        assistant::QUERY_TASKS
            .decode(args(json!({"query":text})))
            .unwrap()
    };
    let get = |id: &str| {
        assistant::GET_TASK
            .decode(args(json!({"task_id":id})))
            .unwrap()
    };
    let detail = json!(read.get_task(&get(&live)).unwrap());
    assert_eq!(detail["steps"][0]["title"], "Visiblecheck");
    for field in ["notes", "merged_into", "links", "reminders", "attachments"] {
        assert!(detail.get(field).is_none());
        assert!(detail["task"].get(field).is_none());
    }
    let lists = json!(read.list_lists(&Default::default()).unwrap());
    assert_eq!(ids(&lists), [&inbox]);
    assert_eq!(lists["items"][0]["open_count"], 1);
    for excluded in [
        &collection,
        &hidden,
        &hidden_task,
        &hidden_tag,
        &link,
        &deleted,
    ] {
        assert!(!detail.to_string().contains(excluded));
        assert!(!lists.to_string().contains(excluded));
    }
    for text in [
        "Excludedtitle",
        "Privatenote",
        "tag:Excludedtag",
        r#"collection:"Excluded Collection""#,
        "Deleted",
    ] {
        assert!(
            read.query_tasks(&query(text)).unwrap().items.is_empty(),
            "{text}"
        );
    }
    assert_eq!(
        read.query_tasks(&query("visiblecheck")).unwrap().items[0].id,
        live_id
    );
    assert_eq!(ids(&server.query_tasks("Privatenote").await.1), [&live]); // REST intentionally searches notes.
    for unavailable in [&hidden_task, &deleted, &id()] {
        assert!(matches!(
            read.get_task(&get(unavailable)),
            Err(ReadError::NotFound)
        ));
    }
    assert_eq!(read.inbox(), Some(inbox.parse().unwrap()));
    assert!(
        Projection::new(&state, &BTreeSet::new(), ctx.clone())
            .inbox()
            .is_none()
    );

    server
        .ok(json!({"cmd":"move_task","id":live,"list":hidden}))
        .await;
    let moved = Projection::new(&server.server.engine.state(), &allowed, ctx);
    assert!(matches!(
        moved.get_task(&get(&live)),
        Err(ReadError::NotFound)
    ));
    assert!(moved.query_tasks(&query("")).unwrap().items.is_empty());
    assert_eq!(
        moved.list_lists(&Default::default()).unwrap().items[0].open_count,
        0
    );
}
