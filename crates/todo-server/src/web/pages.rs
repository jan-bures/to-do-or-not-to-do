//! Server-rendered pages: sign-in, setup, device pairing, and the owner's admin sections.
//!
//! Admin pages need a signed-in session. Every form posts the session's CSRF token, and every
//! owner-only change (devices, agent tokens and grants, share links, pairing approval, the email
//! allowlist and list keys in [`super::ingest`]) also needs a passkey verification within the last
//! five minutes (step-up).

use super::html::{Chrome, PageError, esc, html, page, time};
use super::safe_next;
use crate::api::error::ApiError;
use crate::app::AppState;
use crate::auth::pairing::format_code;
use crate::auth::service::AuthService;
use crate::auth::session::{MaybeSession, OwnerSession, blocking, check_csrf, clear_cookie};
use crate::auth::store::{AgentRow, ShareTarget};
use axum::Form;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::http::header::SET_COOKIE;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use todo_core::ids::{CollectionId, ListId};
use todo_core::{AccessLevel, AgentId, Grants, State as CoreState};

pub(super) type PageResult = Result<Response, PageError>;

pub(super) fn owner_chrome<'a>(svc: &AuthService, s: &'a OwnerSession) -> Chrome<'a> {
    Chrome {
        csrf: Some(&s.row.csrf),
        script: true,
        fresh: Some(svc.is_fresh(&s.row)),
    }
}

pub(super) fn ok_page(title: &str, chrome: &Chrome<'_>, body: &str) -> PageResult {
    Ok(html(StatusCode::OK, page(title, chrome, body)))
}

pub(super) fn not_found() -> PageError {
    PageError::from(ApiError::not_found("there is nothing here"))
}

pub(super) fn csrf_field(s: &OwnerSession) -> String {
    format!(
        r#"<input type="hidden" name="csrf" value="{}">"#,
        esc(&s.row.csrf)
    )
}

/// Checks the CSRF token and the step-up window of a form post.
pub(super) fn guard(
    app: &AppState,
    s: &OwnerSession,
    csrf: &str,
    step_up: bool,
) -> Result<(), PageError> {
    check_csrf(&s.row, Some(csrf))?;
    if step_up {
        app.authsvc
            .require_step_up(&s.row)
            .map_err(|e| PageError::with_csrf(e, &s.row.csrf))?;
    }
    Ok(())
}

const LEVELS: [(AccessLevel, &str); 4] = [
    (AccessLevel::None, "none"),
    (AccessLevel::Read, "read"),
    (AccessLevel::Capture, "capture"),
    (AccessLevel::Plan, "plan"),
];

fn level_name(l: AccessLevel) -> &'static str {
    LEVELS
        .iter()
        .find(|(x, _)| *x == l)
        .map_or("none", |(_, n)| n)
}

fn parse_level(s: &str) -> Option<AccessLevel> {
    LEVELS.iter().find(|(_, n)| *n == s).map(|(l, _)| *l)
}

fn level_select(name: &str, selected: Option<AccessLevel>, inherit: bool) -> String {
    let mut out = format!(r#"<select name="{}">"#, esc(name));
    if inherit {
        let sel = if selected.is_none() { " selected" } else { "" };
        let _ = write!(out, r#"<option value="inherit"{sel}>inherit</option>"#);
    }
    for (l, n) in LEVELS {
        let sel = if selected == Some(l) { " selected" } else { "" };
        let _ = write!(out, r#"<option value="{n}"{sel}>{n}</option>"#);
    }
    out.push_str("</select>");
    out
}

// ---- sign-in and setup ----------------------------------------------------------------------------

/// A form or query.
#[derive(Debug, Deserialize)]
pub struct NextQuery {
    #[serde(default)]
    next: Option<String>,
}

/// `GET /`.
pub async fn root(State(app): State<AppState>, _s: OwnerSession) -> Redirect {
    // The web app when it is installed, the admin pages otherwise.
    Redirect::to(if app.web.is_some() { "/app" } else { "/admin" })
}

/// `GET /login`.
pub async fn login(
    State(app): State<AppState>,
    MaybeSession(session): MaybeSession,
    Query(q): Query<NextQuery>,
) -> PageResult {
    let next = safe_next(q.next.as_deref());
    if session.is_some() {
        return Ok(Redirect::to(&next).into_response());
    }
    let setup = blocking(&app.authsvc, AuthService::setup_open).await??;
    let mut body = format!(
        r#"<p>Sign in with your passkey.</p>
<p><button type="button" data-action="login" data-next="{}">Sign in with passkey</button></p>"#,
        esc(&next)
    );
    if setup {
        body.push_str(
            r#"<p>No passkey is registered yet. <a href="/setup">Set up this server</a> with the one-time code from its log.</p>"#,
        );
    }
    let chrome = Chrome {
        script: true,
        ..Chrome::default()
    };
    ok_page("Sign in", &chrome, &body)
}

/// `GET /auth/verify`: verify a signed-in session before handing out credentials.
pub async fn verify(
    State(app): State<AppState>,
    s: OwnerSession,
    Query(q): Query<NextQuery>,
) -> PageResult {
    let next = safe_next(q.next.as_deref());
    let body = format!(
        r#"<p>Verify with your passkey before pairing or approving a new device.</p>
<p><button type="button" data-action="step-up" data-next="{}">Verify with passkey</button></p>"#,
        esc(&next)
    );
    ok_page("Verify to pair", &owner_chrome(&app.authsvc, &s), &body)
}

/// `GET /setup`: exists until the first passkey is registered, then 404 for good.
pub async fn setup(State(app): State<AppState>) -> PageResult {
    if !blocking(&app.authsvc, AuthService::setup_open).await?? {
        return Err(not_found());
    }
    let body = r#"<p>Enter the one-time setup code from the file <code>setup-code.txt</code> in the server's data directory (the server's log says where it is), then register your first passkey.</p>
<form id="setup-form" data-action="setup">
<label>Setup code <input name="code" autocomplete="one-time-code" required spellcheck="false" autocapitalize="characters"></label>
<button type="submit">Register passkey</button>
</form>"#;
    let chrome = Chrome {
        script: true,
        ..Chrome::default()
    };
    ok_page("Set up", &chrome, body)
}

/// A form or query.
#[derive(Debug, Deserialize)]
pub struct CsrfForm {
    csrf: String,
}

/// `POST /auth/logout`.
pub async fn logout(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<CsrfForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, false)?;
    blocking(&app.authsvc, move |svc| svc.end_session(&s.row, s.ip)).await??;
    let mut res = Redirect::to("/login").into_response();
    res.headers_mut().insert(SET_COOKIE, clear_cookie());
    Ok(res)
}

// ---- pairing --------------------------------------------------------------------------------------

/// `GET /pair/invite`: the link credential is in the fragment, never in the HTTP request.
pub async fn invite() -> PageResult {
    let chrome = Chrome {
        script: true,
        ..Chrome::default()
    };
    ok_page(
        "Pair this browser",
        &chrome,
        r#"<p>Choose a name for this device, then register a passkey to sign in. Only continue if you requested this link from your own signed-in app.</p>
<form data-action="redeem-invite" method="post" action="/pair/invite">
<label>Device name <input id="invite-device-name" maxlength="64" autocomplete="off" required></label>
<button type="submit">Register passkey</button>
</form>"#,
    )
}

/// A form or query.
#[derive(Debug, Deserialize)]
pub struct CodeQuery {
    #[serde(default)]
    code: Option<String>,
}

/// `GET /pair`: shows a pending device to a signed-in owner, with approve and deny buttons.
pub async fn pair(
    State(app): State<AppState>,
    MaybeSession(session): MaybeSession,
    Query(q): Query<CodeQuery>,
) -> PageResult {
    let code = q.code.unwrap_or_default();
    let code = code.trim();
    let Some(s) = session else {
        let next = if code.is_empty() {
            "/pair".to_owned()
        } else {
            format!(
                "/pair?code={}",
                crate::auth::session::encode_component(code)
            )
        };
        let body = format!(
            r#"<p>A device wants to connect to your to-do server. Sign in with your passkey to see it and approve it.</p>
<p><button type="button" data-action="login" data-next="{}">Sign in with passkey</button></p>"#,
            esc(&next)
        );
        let chrome = Chrome {
            script: true,
            ..Chrome::default()
        };
        return ok_page("Pair a device", &chrome, &body);
    };
    let svc = &app.authsvc;
    let chrome = owner_chrome(svc, &s);
    if code.is_empty() {
        let body = r#"<form method="get" action="/pair"><label>Code shown on the device <input name="code" required spellcheck="false" autocapitalize="characters"></label> <button type="submit">Continue</button></form>"#;
        return ok_page("Pair a device", &chrome, body);
    }
    if !svc.may_look_up_codes(&s.row) {
        return Err(PageError::from(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_codes",
            "too many wrong codes; sign out and in again to continue",
        )));
    }
    let Some(p) = svc.pairings.pending(code, svc.now()) else {
        svc.code_lookup_failed(&s.row);
        return ok_page(
            "Pair a device",
            &chrome,
            "<p class=\"error\">There is no pending request with this code. It may have expired (codes last five minutes); start again on the device.</p>",
        );
    };
    let fresh = svc.is_fresh(&s.row);
    let code = format_code(&p.user_code);
    let mut body = format!(
        r#"<p>Device <strong>{name}</strong> asked to connect at {at}, with code <code>{code}</code>.</p>
<p>Only approve it if you started this on your own device and the code matches.</p>"#,
        name = esc(&p.device_name),
        at = esc(&time(p.created_at)),
        code = esc(&code),
    );
    if fresh {
        let _ = write!(
            body,
            r#"<form method="post" action="/pair/approve" class="inline">{csrf}<input type="hidden" name="code" value="{code}"><button type="submit">Approve</button></form>
<form method="post" action="/pair/deny" class="inline">{csrf}<input type="hidden" name="code" value="{code}"><button type="submit" class="secondary">Deny</button></form>"#,
            csrf = csrf_field(&s),
            code = esc(&code),
        );
    } else {
        body.push_str(
            r#"<p>Verify with your passkey first. <button type="button" data-action="step-up">Verify with passkey</button></p>"#,
        );
    }
    ok_page("Pair a device", &chrome, &body)
}

/// A form or query.
#[derive(Debug, Deserialize)]
pub struct PairForm {
    csrf: String,
    code: String,
}

async fn decide(app: AppState, s: OwnerSession, f: PairForm, approve: bool) -> PageResult {
    guard(&app, &s, &f.csrf, approve)?;
    let svc = &app.authsvc;
    let Some(p) = svc.pairings.decide(&f.code, approve, svc.now()) else {
        return Err(PageError::from(ApiError::not_found(
            "there is no pending request with this code; it may have expired or been decided already",
        )));
    };
    let ip = s.ip;
    let name = p.device_name.clone();
    blocking(svc, move |svc| {
        let kind = if approve {
            crate::auth::service::audit::PAIRING_APPROVED
        } else {
            crate::auth::service::audit::PAIRING_DENIED
        };
        svc.audit(kind, ip, serde_json::json!({ "device_name": name }));
    })
    .await?;
    let body = if approve {
        format!(
            "<p>Approved <strong>{}</strong>. The device finishes signing in by itself; you can close this page.</p>",
            esc(&p.device_name)
        )
    } else {
        format!("<p>Denied <strong>{}</strong>.</p>", esc(&p.device_name))
    };
    ok_page("Pair a device", &owner_chrome(svc, &s), &body)
}

/// `POST /pair/approve`.
pub async fn pair_approve(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<PairForm>,
) -> PageResult {
    decide(app, s, f, true).await
}

/// `POST /pair/deny`.
pub async fn pair_deny(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<PairForm>,
) -> PageResult {
    decide(app, s, f, false).await
}

// ---- overview -------------------------------------------------------------------------------------

/// `GET /admin`.
pub async fn overview(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let passkeys = blocking(&app.authsvc, |svc| svc.store().passkeys()).await??;
    let mut body = String::from(
        "<p>Manage the devices, agents and share links that can reach your to-do lists.</p><h2>Passkeys</h2><table><tr><th>Name</th><th>Registered</th><th>Last used</th></tr>",
    );
    for p in &passkeys {
        let _ = write!(
            body,
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&p.name),
            esc(&time(p.created_at)),
            esc(&p.last_used_at.map_or_else(|| "never".into(), time)),
        );
    }
    body.push_str(
        r#"</table>
<p><label>Name <input id="passkey-name" value="Another passkey" maxlength="64"></label> <button type="button" data-action="add-passkey">Add a passkey</button></p>
<p class="hint">Register a second passkey (another device or a security key) so you cannot be locked out.</p>"#,
    );
    ok_page("Overview", &owner_chrome(&app.authsvc, &s), &body)
}

// ---- devices --------------------------------------------------------------------------------------

/// `GET /admin/devices`.
pub async fn devices(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let rows = blocking(&app.authsvc, |svc| svc.store().devices()).await??;
    let mut body = String::from(
        r#"<p>Devices sign in by pairing: start pairing in the app, then approve the code on <a href="/pair">the pairing page</a>.</p>
<table><tr><th>Name</th><th>ID</th><th>Paired</th><th>Last used</th><th></th></tr>"#,
    );
    for d in &rows {
        let action = match d.revoked_at {
            Some(at) => format!("revoked {}", esc(&time(at))),
            None => format!(
                r#"<form method="post" action="/admin/devices/{id}/revoke">{csrf}<button type="submit" class="danger">Revoke</button></form>"#,
                id = esc(d.id.as_str()),
                csrf = csrf_field(&s),
            ),
        };
        let _ = write!(
            body,
            "<tr><td>{}</td><td><code>{}</code></td><td>{}</td><td>{}</td><td>{action}</td></tr>",
            esc(&d.name),
            esc(d.id.as_str()),
            esc(&time(d.created_at)),
            esc(&d.last_used_at.map_or_else(|| "never".into(), time)),
        );
    }
    body.push_str("</table><h2>Browser passkeys</h2><ul>");
    let passkeys = blocking(&app.authsvc, |svc| svc.store().passkeys()).await??;
    for p in &passkeys {
        let _ = write!(
            body,
            "<li>{} · {}</li>",
            esc(&p.name),
            esc(&time(p.created_at))
        );
    }
    body.push_str("</ul>");
    ok_page("Devices", &owner_chrome(&app.authsvc, &s), &body)
}

/// `POST /admin/devices/{id}/revoke`.
pub async fn device_revoke(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<String>,
    Form(f): Form<CsrfForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let ip = s.ip;
    if !blocking(&app.authsvc, move |svc| svc.revoke_device(&id, ip)).await?? {
        return Err(not_found());
    }
    Ok(Redirect::to("/admin/devices").into_response())
}

// ---- agents ---------------------------------------------------------------------------------------

fn agent_status(a: &AgentRow, now: i64) -> String {
    if let Some(at) = a.revoked_at {
        format!("revoked {}", time(at))
    } else if a.expires_at.is_some_and(|x| x <= now) {
        "expired".into()
    } else if let Some(x) = a.expires_at {
        format!("active, expires {}", time(x))
    } else {
        "active".into()
    }
}

/// `GET /admin/agents`.
pub async fn agents(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let rows = blocking(&app.authsvc, AuthService::agents).await??;
    let now = app.authsvc.now();
    let mut body = String::from(
        "<p>Each agent gets one token. Its default level applies wherever no per-collection or per-list grant says otherwise.</p><table><tr><th>Agent</th><th>Default</th><th>Writes per hour</th><th>Status</th><th>Last used</th><th></th></tr>",
    );
    for a in &rows {
        let active = a.revoked_at.is_none();
        let actions = if active {
            format!(
                r#"<a href="/admin/agents/{id}">Grants</a> <form method="post" action="/admin/agents/{id}/revoke" class="inline">{csrf}<button type="submit" class="danger">Revoke</button></form>"#,
                id = a.id,
                csrf = csrf_field(&s),
            )
        } else {
            String::new()
        };
        let _ = write!(
            body,
            "<tr><td><code>{}</code></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{actions}</td></tr>",
            esc(a.agent.as_str()),
            level_name(a.grants.default),
            a.writes_per_hour,
            esc(&agent_status(a, now)),
            esc(&a.last_used_at.map_or_else(|| "never".into(), time)),
        );
    }
    let _ = write!(
        body,
        r#"</table>
<h2>New agent token</h2>
<form method="post" action="/admin/agents">{csrf}
<label>Agent name <input name="name" required pattern="[a-z0-9_-]{{1,64}}" title="lowercase letters, digits, - and _"></label>
<label>Default level {levels}</label>
<label>Writes per hour <input name="writes_per_hour" inputmode="numeric" placeholder="{wph}"></label>
<label>Expires after (days) <input name="expires_in_days" inputmode="numeric" placeholder="never"></label>
<button type="submit">Create token</button>
</form>"#,
        csrf = csrf_field(&s),
        levels = level_select("default", Some(AccessLevel::Read), false),
        wph = app.authsvc.config().agent_writes_per_hour,
    );
    ok_page("Agents", &owner_chrome(&app.authsvc, &s), &body)
}

/// A form or query.
#[derive(Debug, Deserialize)]
pub struct NewAgentForm {
    csrf: String,
    name: String,
    default: String,
    #[serde(default)]
    writes_per_hour: String,
    #[serde(default)]
    expires_in_days: String,
}

pub(super) fn bad_input(msg: &str) -> PageError {
    PageError::from(ApiError::new(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid",
        msg,
    ))
}

fn optional_number(s: &str, what: &str) -> Result<Option<u32>, PageError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<u32>() {
        Ok(n) if n > 0 => Ok(Some(n)),
        _ => Err(bad_input(&format!(
            "{what} must be a positive whole number"
        ))),
    }
}

/// `POST /admin/agents`: creates a token and shows it once.
pub async fn agent_create(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<NewAgentForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let agent = AgentId::new(f.name.trim()).map_err(|_| {
        bad_input("the agent name must be 1 to 64 lowercase letters, digits, - or _")
    })?;
    let default = parse_level(&f.default).ok_or_else(|| bad_input("unknown access level"))?;
    let wph = optional_number(&f.writes_per_hour, "writes per hour")?;
    let days = optional_number(&f.expires_in_days, "the expiry")?;
    let ip = s.ip;
    let name = agent.clone();
    let (_, token) = blocking(&app.authsvc, move |svc| {
        svc.create_agent(&name, default, wph, days, ip)
    })
    .await??;
    let body = format!(
        r#"<p>Token for agent <code>{agent}</code>. Copy it now: it is shown only once and cannot be recovered.</p>
<p><input class="secret" readonly value="{token}" aria-label="Agent token"></p>
<p>The agent sends it as <code>Authorization: Bearer &lt;token&gt;</code>. Its changes are recorded as <code>agent:{agent}</code>.</p>
<p><a href="/admin/agents">Back to agents</a></p>"#,
        agent = esc(agent.as_str()),
        token = esc(&token),
    );
    ok_page(
        "Agent token created",
        &owner_chrome(&app.authsvc, &s),
        &body,
    )
}

/// `POST /admin/agents/{id}/revoke`.
pub async fn agent_revoke(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let ip = s.ip;
    if !blocking(&app.authsvc, move |svc| svc.revoke_agent(id, ip)).await?? {
        return Err(not_found());
    }
    Ok(Redirect::to("/admin/agents").into_response())
}

pub(crate) type Structure = (
    Vec<(CollectionId, String)>,
    Vec<(ListId, Option<CollectionId>, String)>,
);

pub(crate) fn sorted_structure(state: &CoreState) -> Structure {
    let mut cols: Vec<_> = state
        .collections
        .values()
        .filter(|c| state.is_live(c.id.into()))
        .map(|c| (c.position.clone(), c.id, c.name.clone()))
        .collect();
    cols.sort();
    let mut lists: Vec<_> = state
        .lists
        .values()
        .filter(|l| state.is_live(l.id.into()))
        .map(|l| (l.collection, l.position.clone(), l.id, l.name.clone()))
        .collect();
    lists.sort();
    (
        cols.into_iter().map(|(_, id, n)| (id, n)).collect(),
        lists.into_iter().map(|(c, _, id, n)| (id, c, n)).collect(),
    )
}

/// `GET /admin/agents/{id}`: the grants editor.
pub async fn agent_grants(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<i64>,
) -> PageResult {
    let Some(a) = blocking(&app.authsvc, move |svc| svc.store().agent(id))
        .await??
        .filter(|a| a.revoked_at.is_none())
    else {
        return Err(not_found());
    };
    let state = app.engine.state();
    let (cols, lists) = sorted_structure(&state);
    let mut body = format!(
        r#"<p>Levels for agent <code>{agent}</code>: a list's own level wins, then its collection's, then the default.</p>
<form method="post" action="/admin/agents/{id}/grants">{csrf}
<p><label>Default {default}</label></p>
<table><tr><th>Collection or list</th><th>Level</th></tr>"#,
        agent = esc(a.agent.as_str()),
        id = a.id,
        csrf = csrf_field(&s),
        default = level_select("default", Some(a.grants.default), false),
    );
    let mut row = |label: String, name: String, current: Option<AccessLevel>| {
        let _ = write!(
            body,
            "<tr><td>{label}</td><td>{}</td></tr>",
            level_select(&name, current, true)
        );
    };
    for (lid, col, name) in lists.iter().filter(|(_, c, _)| c.is_none()) {
        let _ = col;
        row(
            esc(name),
            format!("l:{lid}"),
            a.grants.lists.get(lid).copied(),
        );
    }
    for (cid, cname) in &cols {
        row(
            format!("<strong>{}</strong> (collection)", esc(cname)),
            format!("c:{cid}"),
            a.grants.collections.get(cid).copied(),
        );
        for (lid, _, name) in lists.iter().filter(|(_, c, _)| *c == Some(*cid)) {
            row(
                format!("&nbsp;&nbsp;{}", esc(name)),
                format!("l:{lid}"),
                a.grants.lists.get(lid).copied(),
            );
        }
    }
    body.push_str(r#"</table><button type="submit">Save grants</button></form><p><a href="/admin/agents">Back to agents</a></p>"#);
    ok_page("Agent grants", &owner_chrome(&app.authsvc, &s), &body)
}

/// `POST /admin/agents/{id}/grants`.
pub async fn agent_grants_save(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<i64>,
    Form(fields): Form<Vec<(String, String)>>,
) -> PageResult {
    let form: BTreeMap<String, String> = fields.into_iter().collect();
    guard(&app, &s, form.get("csrf").map_or("", String::as_str), true)?;
    let state = app.engine.state();
    let mut grants = Grants::with_default(
        form.get("default")
            .and_then(|v| parse_level(v))
            .ok_or_else(|| bad_input("unknown default level"))?,
    );
    for (k, v) in &form {
        if v == "inherit" {
            continue;
        }
        let level = || parse_level(v).ok_or_else(|| bad_input("unknown access level"));
        if let Some(c) = k.strip_prefix("c:") {
            let c: CollectionId = c.parse().map_err(|_| bad_input("unknown collection"))?;
            if !state.collections.contains_key(&c) {
                return Err(bad_input("unknown collection"));
            }
            grants.collections.insert(c, level()?);
        } else if let Some(l) = k.strip_prefix("l:") {
            let l: ListId = l.parse().map_err(|_| bad_input("unknown list"))?;
            if !state.lists.contains_key(&l) {
                return Err(bad_input("unknown list"));
            }
            grants.lists.insert(l, level()?);
        }
    }
    let ip = s.ip;
    if !blocking(&app.authsvc, move |svc| svc.set_grants(id, &grants, ip)).await?? {
        return Err(not_found());
    }
    Ok(Redirect::to(&format!("/admin/agents/{id}")).into_response())
}

// ---- share links ----------------------------------------------------------------------------------

fn target_name(state: &CoreState, t: ShareTarget) -> String {
    match t {
        ShareTarget::List(l) => state
            .lists
            .get(&l)
            .map_or_else(|| "(deleted list)".into(), |l| format!("List: {}", l.name)),
        ShareTarget::Collection(c) => state.collections.get(&c).map_or_else(
            || "(deleted collection)".into(),
            |c| format!("Collection: {}", c.name),
        ),
    }
}

/// `GET /admin/shares`.
pub async fn shares(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let rows = blocking(&app.authsvc, |svc| svc.store().shares()).await??;
    let state = app.engine.state();
    let mut body = String::from(
        "<p>A share link shows one list or collection, read-only: task titles, completion, due dates and steps. Anyone with the link can see it. Links are shown once; rotate a link to get a new one.</p><table><tr><th>Shows</th><th>Created</th><th>Status</th><th></th></tr>",
    );
    for r in &rows {
        let (status, actions) = match r.revoked_at {
            Some(at) => (format!("revoked {}", time(at)), String::new()),
            None => (
                r.rotated_at.map_or_else(
                    || "active".into(),
                    |at| format!("active, rotated {}", time(at)),
                ),
                format!(
                    r#"<form method="post" action="/admin/shares/{id}/rotate" class="inline">{csrf}<button type="submit">Rotate</button></form> <form method="post" action="/admin/shares/{id}/revoke" class="inline">{csrf}<button type="submit" class="danger">Revoke</button></form>"#,
                    id = r.id,
                    csrf = csrf_field(&s),
                ),
            ),
        };
        let _ = write!(
            body,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{actions}</td></tr>",
            esc(&target_name(&state, r.target)),
            esc(&time(r.created_at)),
            esc(&status),
        );
    }
    let (cols, lists) = sorted_structure(&state);
    let mut options = String::new();
    for (id, name) in &cols {
        let _ = write!(
            options,
            r#"<option value="collection:{id}">Collection: {}</option>"#,
            esc(name)
        );
    }
    for (id, _, name) in &lists {
        let _ = write!(
            options,
            r#"<option value="list:{id}">List: {}</option>"#,
            esc(name)
        );
    }
    let _ = write!(
        body,
        r#"</table><h2>New share link</h2><form method="post" action="/admin/shares">{csrf}<label>Share <select name="target" required>{options}</select></label> <button type="submit">Create link</button></form>"#,
        csrf = csrf_field(&s),
    );
    ok_page("Share links", &owner_chrome(&app.authsvc, &s), &body)
}

/// A form or query.
#[derive(Debug, Deserialize)]
pub struct NewShareForm {
    csrf: String,
    target: String,
}

fn link_shown_once(app: &AppState, s: &OwnerSession, title: &str, token: &str) -> PageResult {
    let body = format!(
        r#"<p>Copy the link now: it is shown only once.</p><p><input class="secret" readonly value="{}" aria-label="Share link"></p><p><a href="/admin/shares">Back to share links</a></p>"#,
        esc(&app.authsvc.share_url(token))
    );
    ok_page(title, &owner_chrome(&app.authsvc, s), &body)
}

/// `POST /admin/shares`.
pub async fn share_create(
    State(app): State<AppState>,
    s: OwnerSession,
    Form(f): Form<NewShareForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let state = app.engine.state();
    let target = match f.target.split_once(':') {
        Some(("list", id)) => id
            .parse()
            .ok()
            .filter(|l| state.lists.contains_key(l))
            .map(ShareTarget::List),
        Some(("collection", id)) => id
            .parse()
            .ok()
            .filter(|c| state.collections.contains_key(c))
            .map(ShareTarget::Collection),
        _ => None,
    }
    .ok_or_else(|| bad_input("choose a list or collection"))?;
    let ip = s.ip;
    let token = blocking(&app.authsvc, move |svc| svc.create_share(target, ip)).await??;
    link_shown_once(&app, &s, "Share link created", &token)
}

/// `POST /admin/shares/{id}/rotate`.
pub async fn share_rotate(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let ip = s.ip;
    let Some(token) = blocking(&app.authsvc, move |svc| svc.rotate_share(id, ip)).await?? else {
        return Err(not_found());
    };
    link_shown_once(&app, &s, "Share link rotated", &token)
}

/// `POST /admin/shares/{id}/revoke`.
pub async fn share_revoke(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    let ip = s.ip;
    if !blocking(&app.authsvc, move |svc| svc.revoke_share(id, ip)).await?? {
        return Err(not_found());
    }
    Ok(Redirect::to("/admin/shares").into_response())
}

// ---- audit ----------------------------------------------------------------------------------------

/// `GET /admin/audit`: the newest 200 entries.
pub async fn audit(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let rows = blocking(&app.authsvc, |svc| svc.store().audit_page(200)).await??;
    let mut body =
        String::from("<table><tr><th>Time</th><th>Event</th><th>Address</th><th>Details</th></tr>");
    for r in &rows {
        let _ = write!(
            body,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td><code>{}</code></td></tr>",
            esc(&time(r.at)),
            esc(&r.kind),
            esc(r.ip.as_deref().unwrap_or("")),
            esc(&r.detail),
        );
    }
    body.push_str("</table>");
    ok_page("Audit log", &owner_chrome(&app.authsvc, &s), &body)
}
