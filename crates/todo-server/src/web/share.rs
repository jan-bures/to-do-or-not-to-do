//! `GET /s/{token}`: a read-only share page.
//!
//! It shows only the shared list (or the lists of the shared collection): task titles, completion,
//! due dates, and each task's steps with their completion. Never notes, links, attachments, email
//! metadata, tags, reminders, agent activity or anything outside the shared target. Deleted items are
//! left out.
//!
//! The page has no JavaScript, a stricter CSP than the other pages (`default-src 'none'`, only the
//! server's own stylesheet), `noindex`, and `no-store`. Unknown, revoked and rotated-away tokens all
//! get the same 404: the token is hashed and the hash looked up.
//!
//! **Cost.** Requests are rate-limited per client IP (`auth.share_requests_per_minute`) before the
//! token is looked up. The lookup and rendering run on the blocking pool under the shared read
//! admission permits (503 `overloaded` when they are all taken). A page shows at most
//! [`MAX_LISTS`] lists, [`MAX_TASKS`] tasks in all and [`MAX_STEPS`] steps per task, saying how many
//! were left out; steps are grouped by task in one pass.

use super::html::{esc, html};
use crate::api::{Work, blocking};
use crate::app::AppState;
use crate::auth::session::ClientIp;
use crate::auth::store::ShareTarget;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, RETRY_AFTER};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use todo_core::State as CoreState;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::{Step, Task};

/// Most lists a collection share shows.
pub const MAX_LISTS: usize = 50;
/// Most tasks a share page shows, over all its lists.
pub const MAX_TASKS: usize = 500;
/// Most steps shown per task.
pub const MAX_STEPS: usize = 50;

/// The share page's content security policy.
pub const SHARE_CSP: &str = "default-src 'none'; style-src 'self'; img-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

fn with_headers(mut res: Response) -> Response {
    let h = res.headers_mut();
    h.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static(SHARE_CSP));
    h.insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    h.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn shell(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<meta name="referrer" content="no-referrer">
<title>{title}</title>
<link rel="stylesheet" href="/assets/app.css">
</head>
<body class="share">
<main>
<h1>{title}</h1>
{body}
</main>
</body>
</html>
"#,
        title = esc(title)
    )
}

fn not_found() -> Response {
    with_headers(html(
        StatusCode::NOT_FOUND,
        shell(
            "Not found",
            "<p>This link does not exist or no longer works.</p>",
        ),
    ))
}

fn check(done: bool) -> &'static str {
    if done { "&#x2611;" } else { "&#x2610;" }
}

fn state_class(done: bool) -> &'static str {
    if done { "done" } else { "open" }
}

fn more(out: &mut String, n: usize, what: &str) {
    if n > 0 {
        let _ = write!(out, r#"<p class="hint">And {n} more {what} not shown.</p>"#);
    }
}

fn render_task(t: &Task, steps: Option<&Vec<&Step>>, out: &mut String) {
    let done = t.is_done();
    let _ = write!(
        out,
        r#"<li class="{}"><span class="check">{}</span> {}"#,
        state_class(done),
        check(done),
        esc(&t.title)
    );
    if let Some(due) = t.due {
        let _ = write!(
            out,
            r#" <span class="due">due {}</span>"#,
            esc(&due.to_string())
        );
    }
    if let Some(steps) = steps.filter(|s| !s.is_empty()) {
        out.push_str("<ul class=\"steps\">");
        for s in steps.iter().take(MAX_STEPS) {
            let done = s.completed_at.is_some();
            let _ = write!(
                out,
                r#"<li class="{}"><span class="check">{}</span> {}</li>"#,
                state_class(done),
                check(done),
                esc(&s.title)
            );
        }
        out.push_str("</ul>");
        more(out, steps.len().saturating_sub(MAX_STEPS), "steps");
    }
    out.push_str("</li>");
}

/// The live tasks of `list`, open first, each group in manual order.
fn tasks_of(state: &CoreState, list: ListId) -> Vec<&Task> {
    let mut tasks: Vec<&Task> = state
        .tasks
        .values()
        .filter(|t| t.list == list && state.is_live(t.id.into()))
        .collect();
    tasks.sort_by(|a, b| {
        a.is_done()
            .cmp(&b.is_done())
            .then(a.position.cmp(&b.position))
            .then(a.id.cmp(&b.id))
    });
    tasks
}

/// The live steps of `tasks`, by task, in order: one pass over all steps.
fn steps_of<'a>(state: &'a CoreState, tasks: &HashSet<TaskId>) -> HashMap<TaskId, Vec<&'a Step>> {
    let mut by_task: HashMap<TaskId, Vec<&Step>> = HashMap::new();
    for s in state.steps.values() {
        if s.deleted.is_none() && tasks.contains(&s.task) {
            by_task.entry(s.task).or_default().push(s);
        }
    }
    for steps in by_task.values_mut() {
        steps.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
    }
    by_task
}

/// Renders the lists (with their headings when `headings`), within the page caps.
fn render_lists(state: &CoreState, lists: &[(ListId, &str)], headings: bool, out: &mut String) {
    let mut budget = MAX_TASKS;
    let mut shown: Vec<(ListId, &str, Vec<&Task>, usize)> = Vec::new();
    for (id, name) in lists.iter().take(MAX_LISTS) {
        let all = tasks_of(state, *id);
        let total = all.len();
        let keep: Vec<&Task> = all.into_iter().take(budget).collect();
        budget -= keep.len();
        shown.push((*id, name, keep, total));
    }
    let ids: HashSet<TaskId> = shown
        .iter()
        .flat_map(|(_, _, tasks, _)| tasks.iter().map(|t| t.id))
        .collect();
    let steps = steps_of(state, &ids);
    for (_, name, tasks, total) in &shown {
        if headings {
            let _ = write!(out, "<h2>{}</h2>", esc(name));
        }
        if *total == 0 {
            out.push_str("<p class=\"hint\">No tasks.</p>");
            continue;
        }
        if !tasks.is_empty() {
            out.push_str("<ul class=\"tasks\">");
            for t in tasks {
                render_task(t, steps.get(&t.id), out);
            }
            out.push_str("</ul>");
        }
        more(out, total - tasks.len(), "tasks");
    }
    more(out, lists.len().saturating_sub(MAX_LISTS), "lists");
}

/// Renders the page for a target, or `None` if it no longer exists.
#[must_use]
pub fn render(state: &CoreState, target: ShareTarget) -> Option<(String, String)> {
    let mut body = String::new();
    match target {
        ShareTarget::List(id) => {
            let l = state
                .lists
                .get(&id)
                .filter(|l| state.is_live(l.id.into()))?;
            render_lists(state, &[(id, l.name.as_str())], false, &mut body);
            Some((l.name.clone(), body))
        }
        ShareTarget::Collection(id) => {
            let c = state
                .collections
                .get(&id)
                .filter(|c| state.is_live(c.id.into()))?;
            let mut lists: Vec<_> = state
                .lists
                .values()
                .filter(|l| l.collection == Some(id) && state.is_live(l.id.into()))
                .collect();
            lists.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
            let lists: Vec<(ListId, &str)> =
                lists.iter().map(|l| (l.id, l.name.as_str())).collect();
            render_lists(state, &lists, true, &mut body);
            Some((c.name.clone(), body))
        }
    }
}

fn busy(status: StatusCode, retry_after: u32, msg: &str) -> Response {
    let mut res = with_headers(html(status, shell("Busy", &format!("<p>{}</p>", esc(msg)))));
    if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
        res.headers_mut().insert(RETRY_AFTER, v);
    }
    res
}

/// `GET /s/{token}`.
pub async fn show(
    State(app): State<AppState>,
    ClientIp(ip): ClientIp,
    Path(token): Path<String>,
) -> Response {
    if let Some(ip) = ip
        && let Err(wait) = app.authsvc.share_limiter.check(&ip)
    {
        return busy(
            StatusCode::TOO_MANY_REQUESTS,
            wait,
            "Too many requests; try again in a moment.",
        );
    }
    // Share tokens are 22 base64url characters; anything else cannot match.
    if token.len() > 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return not_found();
    }
    let (svc, engine) = (app.authsvc.clone(), app.engine.clone());
    let rendered = blocking(&app, Work::Read, move || {
        let row = svc.share(&token)?;
        Ok::<_, crate::db::StoreError>(row.and_then(|r| render(&engine.state(), r.target)))
    })
    .await;
    match rendered {
        Ok(Ok(Some((title, body)))) => with_headers(html(StatusCode::OK, shell(&title, &body))),
        Ok(Ok(None)) => not_found(),
        Ok(Err(e)) => {
            tracing::error!(error = %e, "cannot look up a share link");
            not_found()
        }
        Err(e) => {
            let status = e.into_response().status();
            busy(status, 5, "The server is busy; try again in a moment.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use todo_core::ids::CollectionId;
    use todo_core::time::Timestamp;
    use todo_core::{Actor, apply, decide};

    fn run(state: &mut CoreState, cmd: serde_json::Value) {
        let owner = Actor::owner("pc").unwrap();
        let cmd = serde_json::from_value(cmd).unwrap();
        let at = Timestamp::from_second(1_700_000_000).unwrap();
        for e in decide(state, &owner, &cmd, at).unwrap() {
            apply(state, &e).unwrap();
        }
    }

    fn new_id(n: &mut u8) -> [u8; 10] {
        *n = n.wrapping_add(1);
        let mut b = [0; 10];
        b[0] = *n;
        b[1] = n.wrapping_mul(7);
        b
    }

    #[test]
    fn pages_are_capped_and_say_what_was_left_out() {
        let mut s = CoreState::new();
        let mut n = 0u8;
        let col = CollectionId::new_v7(1, new_id(&mut n));
        run(
            &mut s,
            serde_json::json!({ "cmd": "create_collection", "id": col, "name": "C" }),
        );
        let mut lists = Vec::new();
        for i in 0..MAX_LISTS + 2 {
            let l = ListId::new_v7(2 + i as u64, [9; 10]);
            run(
                &mut s,
                serde_json::json!({ "cmd": "create_list", "id": l, "name": format!("L{i}"), "collection": col }),
            );
            lists.push(l);
        }
        // The first list alone has more tasks than a page shows; its first task more steps.
        let first_task = TaskId::new_v7(1, [1; 10]);
        for i in 0..MAX_TASKS + 3 {
            let t = if i == 0 {
                first_task
            } else {
                TaskId::new_v7(1000 + i as u64, [2; 10])
            };
            run(
                &mut s,
                serde_json::json!({ "cmd": "create_task", "id": t, "list": lists[0], "title": format!("T{i}") }),
            );
        }
        run(
            &mut s,
            serde_json::json!({ "cmd": "create_task", "id": TaskId::new_v7(9, [8; 10]), "list": lists[1], "title": "later" }),
        );
        for i in 0..MAX_STEPS + 4 {
            let step = todo_core::ids::StepId::new_v7(5000 + i as u64, [3; 10]);
            run(
                &mut s,
                serde_json::json!({ "cmd": "create_step", "id": step, "task": first_task, "title": format!("S{i}") }),
            );
        }

        let (_, body) = render(&s, ShareTarget::Collection(col)).unwrap();
        assert_eq!(body.matches("<h2>").count(), MAX_LISTS, "lists capped");
        assert!(body.contains("And 2 more lists not shown."), "{body}");
        let tasks = body.matches("<ul class=\"steps\">").count();
        assert_eq!(tasks, 1);
        assert_eq!(
            body.matches("<li class=").count(),
            MAX_TASKS + MAX_STEPS,
            "tasks and steps capped"
        );
        assert!(body.contains("And 3 more tasks not shown."));
        assert!(body.contains("And 4 more steps not shown."));
        // The task budget is spent on the first list; later lists say so instead of listing tasks.
        assert!(
            body.contains("<h2>L1</h2><p class=\"hint\">And 1 more tasks not shown.</p>"),
            "{body}"
        );
        assert!(body.contains("<h2>L2</h2><p class=\"hint\">No tasks.</p>"));

        let (_, one) = render(&s, ShareTarget::List(lists[0])).unwrap();
        assert!(!one.contains("<h2>"));
        assert!(one.contains("And 3 more tasks not shown."));
    }

    #[test]
    fn steps_go_to_their_own_task() {
        let mut s = CoreState::new();
        let l = ListId::new_v7(1, [1; 10]);
        let (a, b) = (TaskId::new_v7(2, [2; 10]), TaskId::new_v7(3, [3; 10]));
        run(
            &mut s,
            serde_json::json!({ "cmd": "create_list", "id": l, "name": "L" }),
        );
        for (t, name) in [(a, "A"), (b, "B")] {
            run(
                &mut s,
                serde_json::json!({ "cmd": "create_task", "id": t, "list": l, "title": name }),
            );
        }
        for (i, (t, title)) in [(a, "a-step"), (b, "b-step"), (a, "a-step-2")]
            .into_iter()
            .enumerate()
        {
            let step = todo_core::ids::StepId::new_v7(10 + i as u64, [4; 10]);
            run(
                &mut s,
                serde_json::json!({ "cmd": "create_step", "id": step, "task": t, "title": title }),
            );
        }
        let (_, body) = render(&s, ShareTarget::List(l)).unwrap();
        let a_at = body.find("> A<ul").expect("A with steps");
        let b_at = body.find("> B<ul").expect("B with steps");
        let (first, second) = if a_at < b_at {
            (&body[a_at..b_at], &body[b_at..])
        } else {
            (&body[a_at..], &body[b_at..a_at])
        };
        assert!(
            first.contains("a-step") && first.contains("a-step-2") && !first.contains("b-step")
        );
        assert!(second.contains("b-step") && !second.contains("a-step"));
    }
}
