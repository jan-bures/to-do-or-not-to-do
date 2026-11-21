//! The email admin section (`/admin/ingest`): the Inbox, the sender allowlist and the list keys.
//!
//! Routing is explicit: a key names one list by ID, and only keys set here work; everything else goes
//! to the Inbox picked here (see [`crate::ingest`]). The page flags an Inbox or key whose list was
//! deleted.
//!
//! Every change is owner-only: it needs the session's CSRF token and a recent passkey check
//! (step-up), and is written to the audit log in the same transaction as the change.

use super::html::{PageError, esc, time};
use super::pages::{
    PageResult, bad_input, csrf_field, guard, not_found, ok_page, owner_chrome, sorted_structure,
};
use crate::app::AppState;
use crate::auth::session::{OwnerSession, blocking};
use crate::ingest::store::{By, KeyError};
use crate::ingest::{INBOX_KEY, normalize_address};
use axum::Form;
use axum::extract::State;
use axum::response::{IntoResponse, Redirect};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Write as _;
use todo_core::State as CoreState;
use todo_core::ids::ListId;

/// What agents learn from an email task, said next to every destination picker.
const VISIBILITY: &str = "Agents that can read this list see the email's subject, sender and any kept body. Share pages never show them.";

/// `Collection / List` for a live list, or `None` if the list is gone.
fn list_label(state: &CoreState, list: ListId) -> Option<String> {
    let l = state
        .lists
        .get(&list)
        .filter(|_| state.is_live(list.into()))?;
    Some(match l.collection.and_then(|c| state.collections.get(&c)) {
        Some(c) => format!("{} / {}", c.name, l.name),
        None => l.name.clone(),
    })
}

/// `<option>`s for every live list, with `selected` on `current`.
fn list_options(state: &CoreState, current: Option<ListId>) -> String {
    let (cols, lists) = sorted_structure(state);
    let col_names: HashMap<_, _> = cols.into_iter().collect();
    let mut out = String::new();
    for (id, col, name) in &lists {
        let label = match col.and_then(|c| col_names.get(&c)) {
            Some(c) => format!("{} / {}", esc(c), esc(name)),
            None => esc(name),
        };
        let sel = if current == Some(*id) {
            " selected"
        } else {
            ""
        };
        let _ = write!(out, r#"<option value="{id}"{sel}>{label}</option>"#);
    }
    out
}

/// `GET /admin/ingest`.
pub async fn page(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let ingest = app.ingest.clone();
    let state = app.engine.state();
    let (senders, keys, inbox) = {
        let ingest = ingest.clone();
        blocking(&app.authsvc, move |_| {
            Ok::<_, crate::db::StoreError>((
                ingest.store.senders()?,
                ingest.store.list_keys()?,
                ingest.store.inbox()?,
            ))
        })
        .await??
    };
    let csrf = csrf_field(&s);
    let mut body = String::new();
    if ingest.enabled() {
        body.push_str(
            "<p>Email ingest is <strong>on</strong>: an email forwarded from an allowed address to the ingest address becomes a task.</p>",
        );
    } else {
        body.push_str(
            "<p>Email ingest is <strong>off</strong>: set <code>ingest.email_hmac_secret_file</code> in the server config to turn it on.</p>",
        );
    }

    // The Inbox.
    let inbox_label = inbox.and_then(|l| list_label(&state, l));
    let _ = write!(
        body,
        "<h2>Inbox</h2><p>Plain <code>todo@</code> mail, <code>todo+{INBOX_KEY}@</code>, and keys that name no list go to this list.</p>"
    );
    match (inbox, &inbox_label) {
        (None, _) => body.push_str(
            r#"<p class="error"><strong>No Inbox is set.</strong> Email is refused (the Worker keeps retrying it) until you pick one.</p>"#,
        ),
        (Some(_), None) => body.push_str(
            r#"<p class="error"><strong>The Inbox list was deleted.</strong> Email is refused (the Worker keeps retrying it) until you pick another.</p>"#,
        ),
        (Some(_), Some(l)) => {
            let _ = write!(body, "<p>The Inbox is <strong>{}</strong>.</p>", esc(l));
        }
    }
    let _ = write!(
        body,
        r#"<form method="post" action="/admin/ingest/inbox">{csrf}
<label>Inbox <select name="list"><option value="">(none)</option>{opts}</select></label>
<p class="hint">{VISIBILITY}</p>
<button type="submit">Save Inbox</button>
</form>"#,
        opts = list_options(&state, inbox.filter(|_| inbox_label.is_some())),
    );

    // Senders.
    body.push_str(
        "<h2>Allowed senders</h2><p>Only mail forwarded from these addresses becomes a task. The Email Worker has its own copy of the list (<code>ALLOWED_SENDERS</code>); keep both up to date.</p><table><tr><th>Address</th><th>Added</th><th>From</th><th></th></tr>",
    );
    for r in &senders {
        let _ = write!(
            body,
            r#"<tr><td>{a}</td><td>{at}</td><td>{src}</td><td><form method="post" action="/admin/ingest/senders/remove">{csrf}<input type="hidden" name="address" value="{a}"><button type="submit" class="danger">Remove</button></form></td></tr>"#,
            a = esc(&r.address),
            at = esc(&time(r.added_at)),
            src = esc(&r.source),
        );
    }
    let _ = write!(
        body,
        r#"</table>
<form method="post" action="/admin/ingest/senders">{csrf}
<label>Address <input name="address" type="email" required maxlength="320"></label>
<button type="submit">Add sender</button>
</form>"#
    );

    // List keys.
    let _ = write!(
        body,
        "<h2>List keys</h2><p><code>todo+&lt;key&gt;@</code> puts the task in the list the key names. Only the keys below work; list names are never matched. A key is lowercase letters, digits, <code>-</code> and <code>_</code>, starting with a letter or digit; <code>{INBOX_KEY}</code> is reserved for the Inbox.</p><table><tr><th>Key</th><th>List</th><th></th></tr>"
    );
    for (key, list) in &keys {
        let label = match list_label(&state, *list) {
            Some(l) => esc(&l),
            None => r#"<span class="error"><strong>list deleted</strong>: this key's mail goes to the Inbox</span>"#.to_owned(),
        };
        let _ = write!(
            body,
            r#"<tr><td><code>{k}</code></td><td>{label}</td><td><form method="post" action="/admin/ingest/keys/remove">{csrf}<input type="hidden" name="key" value="{k}"><button type="submit" class="danger">Remove</button></form></td></tr>"#,
            k = esc(key),
        );
    }
    let _ = write!(
        body,
        r#"</table>
<form method="post" action="/admin/ingest/keys">{csrf}
<label>Key <input name="key" required maxlength="64" pattern="[a-z0-9][a-z0-9_\-]{{0,63}}"></label>
<label>List <select name="list" required>{opts}</select></label>
<p class="hint">{VISIBILITY}</p>
<button type="submit">Add key</button>
</form>"#,
        opts = list_options(&state, None),
    );
    ok_page("Email", &owner_chrome(&app.authsvc, &s), &body)
}

/// The sender forms.
#[derive(Debug, Deserialize)]
pub struct SenderForm {
    csrf: String,
    address: String,
}

/// `POST /admin/ingest/senders`.
pub async fn sender_add(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<SenderForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let address = normalize_address(&f.address).ok_or_else(|| bad_input("not an email address"))?;
    let (ingest, ip) = (app.ingest.clone(), s.ip);
    blocking(&app.authsvc, move |svc| {
        let by = By { at: svc.now(), ip };
        ingest.store.add_sender(&address, by)
    })
    .await??;
    Ok(Redirect::to("/admin/ingest").into_response())
}

/// `POST /admin/ingest/senders/remove`.
pub async fn sender_remove(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<SenderForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let address = normalize_address(&f.address).ok_or_else(not_found)?;
    let (ingest, ip) = (app.ingest.clone(), s.ip);
    let removed = blocking(&app.authsvc, move |svc| {
        let by = By { at: svc.now(), ip };
        ingest.store.remove_sender(&address, by)
    })
    .await??;
    if !removed {
        return Err(not_found());
    }
    Ok(Redirect::to("/admin/ingest").into_response())
}

/// A list picked in a form: a live list, or empty.
fn picked(state: &CoreState, raw: &str) -> Result<Option<ListId>, PageError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let list: ListId = raw.parse().map_err(|_| bad_input("unknown list"))?;
    if list_label(state, list).is_none() {
        return Err(bad_input("unknown list"));
    }
    Ok(Some(list))
}

/// The Inbox form.
#[derive(Debug, Deserialize)]
pub struct InboxForm {
    csrf: String,
    list: String,
}

/// `POST /admin/ingest/inbox`: a live list, or empty to unset.
pub async fn inbox_set(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<InboxForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let list = picked(&app.engine.state(), &f.list)?;
    let (ingest, ip) = (app.ingest.clone(), s.ip);
    blocking(&app.authsvc, move |svc| {
        let by = By { at: svc.now(), ip };
        ingest.store.set_inbox(list, by)
    })
    .await??;
    Ok(Redirect::to("/admin/ingest").into_response())
}

/// The list key forms.
#[derive(Debug, Deserialize)]
pub struct KeyForm {
    csrf: String,
    key: String,
    #[serde(default)]
    list: String,
}

/// `POST /admin/ingest/keys`: `key` now names `list` (a live list).
pub async fn key_add(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<KeyForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let list = picked(&app.engine.state(), &f.list)?.ok_or_else(|| bad_input("pick a list"))?;
    let key = f.key.trim().to_owned();
    let (ingest, ip) = (app.ingest.clone(), s.ip);
    let added = blocking(&app.authsvc, move |svc| {
        let by = By { at: svc.now(), ip };
        ingest.store.add_list_key(&key, list, by)
    })
    .await??;
    match added {
        Ok(()) => Ok(Redirect::to("/admin/ingest").into_response()),
        Err(KeyError::Invalid) => Err(bad_input(
            "a key must be 1 to 64 lowercase letters, digits, - or _, starting with a letter or digit",
        )),
        Err(KeyError::Reserved) => Err(bad_input("the key inbox always means the Inbox")),
        Err(KeyError::Taken) => Err(bad_input(
            "that key is set already; remove it first to point it at another list",
        )),
    }
}

/// `POST /admin/ingest/keys/remove`.
pub async fn key_remove(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<KeyForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let key = f.key.trim().to_owned();
    let (ingest, ip) = (app.ingest.clone(), s.ip);
    let removed = blocking(&app.authsvc, move |svc| {
        let by = By { at: svc.now(), ip };
        ingest.store.remove_list_key(&key, by)
    })
    .await??;
    if removed.is_none() {
        return Err(not_found());
    }
    Ok(Redirect::to("/admin/ingest").into_response())
}
