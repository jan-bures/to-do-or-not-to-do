//! Server-rendered pages: sign-in, setup, device pairing, and the owner's admin sections.
//!
//! Admin pages need a signed-in session. Every form posts the session's CSRF token, and every
//! owner-only change (devices, agent tokens and grants, share links, pairing approval, the email
//! allowlist and list keys in [`super::ingest`]) also needs a passkey verification within the last
//! five minutes (step-up).

use super::html::{
    Chrome, PageError, Section, date, date_time, esc, html, last_active, page, relative_time, row,
};
use super::safe_next;
use crate::api::error::ApiError;
use crate::app::AppState;
use crate::auth::invite::Issuer;
use crate::auth::pairing::format_code;
use crate::auth::service::{AuthService, STEP_UP_MS};
use crate::auth::session::{MaybeSession, OwnerSession, blocking, check_csrf, clear_cookie};
use crate::auth::store::{AgentRow, CredentialRoot, RevocationImpact, ShareTarget};
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
    let minutes = if svc.is_fresh(&s.row) {
        let remaining = (STEP_UP_MS - (svc.now() - s.row.verified_at)).clamp(0, STEP_UP_MS);
        (remaining + 59_999) / 60_000
    } else {
        0
    };
    Chrome {
        csrf: Some(&s.row.csrf),
        script: true,
        minutes_left: Some(minutes),
        ..Chrome::default()
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
        r#"<p class="lead">Use your passkey to open your to-do lists and manage access.</p>
<p><button type="button" data-action="login" data-next="{}">Sign in with passkey</button></p>"#,
        esc(&next)
    );
    if setup {
        body.push_str(
            r#"<p>No passkey is registered yet. <a href="/setup">Set up this server</a> with the one-time code from its data directory.</p>"#,
        );
    }
    let chrome = Chrome {
        script: true,
        ..Chrome::default()
    };
    ok_page("Sign in", &chrome, &body)
}

/// `GET /auth/verify`: verify a signed-in session before issuing or revoking credentials.
pub async fn verify(
    State(app): State<AppState>,
    s: OwnerSession,
    Query(q): Query<NextQuery>,
) -> PageResult {
    let next = safe_next(q.next.as_deref());
    let (title, message) = if next == "/admin/devices" {
        (
            "Verify to revoke",
            "Verify with a passkey registered before the passkeys this revocation would remove.",
        )
    } else {
        (
            "Verify to pair",
            "Verify with your passkey before pairing or approving a new device.",
        )
    };
    let body = format!(
        r#"<p class="lead">{message}</p>
<p><button type="button" data-action="step-up" data-next="{}">Verify with passkey</button></p>"#,
        esc(&next)
    );
    let mut chrome = owner_chrome(&app.authsvc, &s);
    // This page already has a verification button with the correct follow-up destination.
    chrome.minutes_left = None;
    if next == "/admin/devices" {
        chrome = chrome.in_section(Section::Devices);
    }
    ok_page(title, &chrome, &body)
}

/// `GET /setup`: exists until the first passkey is registered, then 404 for good.
pub async fn setup(State(app): State<AppState>) -> PageResult {
    if !blocking(&app.authsvc, AuthService::setup_open).await?? {
        return Err(not_found());
    }
    let body = r#"<p>Enter the one-time setup code from the file <code>setup-code.txt</code> in the server's data directory (the server's log says where it is), then register your first passkey.</p>
<form id="setup-form" class="stack" data-action="setup">
<label>Setup code <input class="code-input" name="code" autocomplete="one-time-code" required spellcheck="false" autocapitalize="characters"></label>
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
<form class="stack" data-action="redeem-invite" method="post" action="/pair/invite">
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
        let body = r#"<p class="lead">Start pairing in the app on the device you want to connect. Enter the code it shows.</p>
<form class="fields" method="get" action="/pair"><label>Pairing code <input class="code-input" name="code" required spellcheck="false" autocapitalize="characters"></label><button type="submit">Continue</button></form>"#;
        return ok_page("Pair a device", &chrome, body);
    }
    let issuer = Issuer::Session(s.row.id_hash.clone());
    let Some(p) = svc.look_up_pairing_code(issuer, || svc.pairings.pending(code, svc.now()))?
    else {
        return ok_page(
            "Pair a device",
            &chrome,
            "<p class=\"error\">There is no pending request with this code. It may have expired (codes last five minutes); start again on the device.</p>",
        );
    };
    let fresh = svc.is_fresh(&s.row);
    let code = format_code(&p.user_code);
    let mut body = format!(
        r#"<section aria-labelledby="pair-device"><h2 id="pair-device">{name}</h2>
<p class="meta">Requested {at} · Source address: {source}</p>
<p>Only approve if you started this on your own device and its code matches <code>{code}</code>.</p></section>"#,
        name = esc(&p.device_name),
        at = date_time(p.created_at),
        code = esc(&code),
        source = p
            .source
            .map_or_else(|| "Unknown".into(), |ip| ip.to_string()),
    );
    if fresh {
        let _ = write!(
            body,
            r#"<div class="actions"><form method="post" action="/pair/approve">{csrf}<input type="hidden" name="code" value="{code}"><button type="submit">Approve device</button></form>
<form method="post" action="/pair/deny">{csrf}<input type="hidden" name="code" value="{code}"><button type="submit" class="danger">Deny</button></form></div>"#,
            csrf = csrf_field(&s),
            code = esc(&code),
        );
    } else {
        body.push_str(r#"<p class="hint">Verify with your passkey to review this request.</p>"#);
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
    let issuer = Issuer::Session(s.row.id_hash.clone());
    let Some(p) = svc.look_up_pairing_code(issuer.clone(), || {
        if approve {
            svc.pairings
                .approve_reviewed(&f.code, None, &issuer, svc.now())
        } else {
            svc.pairings.deny(&f.code, svc.now())
        }
    })?
    else {
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
    let hash = s.row.id_hash.clone();
    let (passkeys, current) = blocking(&app.authsvc, move |svc| {
        let store = svc.store();
        Ok::<_, crate::db::StoreError>((
            store.passkeys()?,
            store.session_origin(&hash)?.and_then(|o| o.passkey),
        ))
    })
    .await??;
    let now = app.authsvc.now();
    let mut body = String::from(
        r#"<p class="lead">Manage the devices, agents and share links that can reach your to-do lists.</p>
<section aria-labelledby="passkeys"><h2 id="passkeys">Passkeys</h2>"#,
    );
    if passkeys.is_empty() {
        body.push_str(r#"<p class="empty">No passkeys listed. Add one below to sign in from this browser.</p>"#);
    } else {
        body.push_str(r#"<ul class="rows" role="list">"#);
        for p in &passkeys {
            let mut facts = vec![format!("Registered {}", date(p.created_at))];
            if let Some(at) = p.last_used_at {
                facts.push(format!("Last used {}", relative_time(at, now)));
            } else if current != Some(p.id) {
                facts.push("Not used yet".into());
            }
            if current == Some(p.id) {
                facts.push("Used to sign in here".into());
            }
            row(&mut body, &p.name, &facts, "", "");
        }
        body.push_str("</ul>");
    }
    body.push_str(r#"<p class="hint">Register a backup passkey on another device or security key so you can still sign in if you lose this one.</p>
<div class="fields"><label>Passkey name <input id="passkey-name" value="Another passkey" maxlength="64"></label><button type="button" data-action="add-passkey">Add a passkey</button></div></section>"#);
    ok_page(
        "Overview",
        &owner_chrome(&app.authsvc, &s).in_section(Section::Overview),
        &body,
    )
}

// ---- devices --------------------------------------------------------------------------------------

fn revoke_warning(impact: RevocationImpact) -> String {
    let counts = [
        (impact.devices, "device"),
        (impact.passkeys, "passkey"),
        (impact.sessions, "browser session"),
    ]
    .into_iter()
    .filter(|(count, _)| *count > 0)
    .map(|(count, name)| format!("{count} {name}{}", if count == 1 { "" } else { "s" }))
    .collect::<Vec<_>>();
    if counts.is_empty() {
        String::new()
    } else {
        format!(
            r#"<p class="hint">Also removes: {}.</p>"#,
            counts.join(", ")
        )
    }
}

fn issuer_label(value: Option<&str>, session: &str) -> &'static str {
    match value {
        Some(value) if value.strip_prefix("Browser session ") == Some(session) => {
            "Approved from this browser session (passkey)"
        }
        Some(value) if value.starts_with("Browser session ") => {
            "Approved from a browser session (passkey)"
        }
        Some(value) if value.starts_with("Device ") => "Approved from a paired device",
        _ => "Approval source not recorded",
    }
}

fn short_id(id: &str) -> String {
    format!("{}…", id.chars().take(8).collect::<String>())
}

/// Keep the full ID selectable in a closed disclosure, with soft breaks between groups. The
/// same identifier goes to the existing POST route as an encoded path segment, not visible text.
fn device_identity(id: &str) -> (String, String) {
    let mut path = String::new();
    for b in id.bytes() {
        let _ = write!(path, "%{b:02X}");
    }
    let mut detail = String::new();
    for (i, c) in id.chars().enumerate() {
        if i > 0 && i % 8 == 0 {
            detail.push_str("<wbr>");
        }
        detail.push_str(&esc(&c.to_string()));
    }
    (path, detail)
}

/// `GET /admin/devices`.
pub async fn devices(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let rows = blocking(&app.authsvc, |svc| {
        svc.store()
            .devices()?
            .into_iter()
            .map(|d| {
                let count = if d.revoked_at.is_none() {
                    svc.store()
                        .revocation_impact(CredentialRoot::Device(d.id.as_str()))?
                } else {
                    RevocationImpact::default()
                };
                Ok((d, count))
            })
            .collect::<Result<Vec<_>, crate::db::StoreError>>()
    })
    .await??;
    let now = app.authsvc.now();
    let mut body = String::from(
        r#"<p class="lead">Manage paired apps and browser passkeys. To connect another device, start pairing in its app.</p>
<section aria-labelledby="paired-apps"><h2 id="paired-apps">Paired apps</h2><a class="button secondary" href="/pair">Enter a pairing code</a>"#,
    );
    if rows.is_empty() {
        body.push_str(r#"<p class="empty">No paired apps yet. When an app connects, it will appear here.</p>"#);
    } else {
        body.push_str(r#"<ul class="rows" role="list">"#);
        for (d, count) in rows {
            let id = d.id.as_str();
            let (path, detail) = device_identity(id);
            let mut facts = vec![
                format!("Paired {}", date(d.created_at)),
                last_active(d.last_used_at, now),
            ];
            let action = if let Some(at) = d.revoked_at {
                facts.push(format!("Revoked {}", date(at)));
                String::new()
            } else {
                let warning = revoke_warning(count);
                format!(
                    r#"<details class="confirm"><summary class="danger"><span class="confirm-open">Revoke</span><span class="confirm-cancel">Cancel</span></summary>
<form class="stack" method="post" action="/admin/devices/{path}/revoke">{csrf}<p>Disconnect this device?</p>{warning}<button type="submit" class="danger">Confirm revoke</button></form></details>"#,
                    csrf = csrf_field(&s)
                )
            };
            let extra = format!(
                r#"<p class="meta">{issuer}</p><details><summary>Device ID <code class="short-id">{prefix}</code></summary><code>{detail}</code></details>"#,
                issuer = issuer_label(d.issued_by.as_deref(), &s.row.id_hash),
                prefix = esc(&short_id(id))
            );
            row(&mut body, &d.name, &facts, &extra, &action);
        }
        body.push_str("</ul>");
    }
    body.push_str(r#"</section><section aria-labelledby="browser-passkeys"><h2 id="browser-passkeys">Browser passkeys</h2>"#);
    let fresh = app.authsvc.is_fresh(&s.row);
    let hash = s.row.id_hash.clone();
    let (passkeys, current) = blocking(&app.authsvc, move |svc| {
        let store = svc.store();
        let current = store.session_origin(&hash)?.and_then(|o| o.passkey);
        let verified_by = if fresh {
            store.step_up_passkey(&hash)?
        } else {
            None
        };
        let passkeys = store
            .passkeys()?
            .into_iter()
            .map(|p| {
                let impact = if verified_by.is_some_and(|id| id < p.id) {
                    Some(store.revocation_impact(CredentialRoot::Passkey(p.id))?)
                } else {
                    None
                };
                Ok((p, impact))
            })
            .collect::<Result<Vec<_>, crate::db::StoreError>>()?;
        Ok::<_, crate::db::StoreError>((passkeys, current))
    })
    .await??;
    if passkeys.is_empty() {
        body.push_str(r#"<p class="empty">No browser passkeys listed. <a href="/admin">Add one from Overview</a>.</p>"#);
    } else {
        body.push_str(r#"<ul class="rows" role="list">"#);
    }
    let has_passkeys = !passkeys.is_empty();
    for (p, impact) in passkeys {
        let action = if let Some(impact) = impact {
            format!(
                r#"<form class="stack" method="post" action="/auth/passkeys/{}/revoke">{}<button type="submit" class="danger">Revoke</button>{}</form>"#,
                p.id,
                csrf_field(&s),
                revoke_warning(impact),
            )
        } else {
            String::new()
        };
        let mut facts = vec![format!("Registered {}", date(p.created_at))];
        if current == Some(p.id) {
            facts.push("Used to sign in here".into());
        }
        if p.issued_by.is_some() {
            facts.push(issuer_label(p.issued_by.as_deref(), &s.row.id_hash).into());
        }
        row(&mut body, &p.name, &facts, "", &action);
    }
    if has_passkeys {
        body.push_str("</ul>");
    }
    body.push_str(r#"<p class="hint">Revoking a device or passkey also signs out the devices and browsers it added, including those added through them. It invalidates any outstanding invites from those sign-ins.</p>
<p class="hint">A newer passkey cannot remove an older one, even by revoking a device. If a revocation is blocked, <a href="/auth/verify?next=/admin/devices">verify with an older passkey</a>. You cannot revoke the oldest passkey here.</p></section>"#);
    ok_page(
        "Devices",
        &owner_chrome(&app.authsvc, &s).in_section(Section::Devices),
        &body,
    )
}

/// `POST /admin/devices/{id}/revoke`.
pub async fn device_revoke(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<String>,
    Form(f): Form<CsrfForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    if !blocking(&app.authsvc, move |svc| {
        svc.revoke_device_from_session(&s.row, &id, s.ip)
    })
    .await??
    {
        return Err(not_found());
    }
    Ok(Redirect::to("/admin/devices").into_response())
}

/// `POST /auth/passkeys/{id}/revoke`.
pub async fn passkey_revoke(
    State(app): State<AppState>,
    s: OwnerSession,
    Path(id): Path<i64>,
    Form(f): Form<CsrfForm>,
) -> PageResult {
    guard(&app, &s, &f.csrf, true)?;
    blocking(&app.authsvc, move |svc| {
        svc.revoke_passkey(&s.row, id, s.ip)
    })
    .await??;
    Ok(Redirect::to("/admin/devices").into_response())
}

// ---- agents ---------------------------------------------------------------------------------------

fn agent_status(a: &AgentRow, now: i64) -> String {
    if let Some(at) = a.revoked_at {
        format!("Revoked {}", date(at))
    } else if let Some(x) = a.expires_at {
        format!(
            "{} {}",
            if x <= now { "Expired" } else { "Expires" },
            date(x)
        )
    } else {
        "Active".into()
    }
}

/// `GET /admin/agents`.
pub async fn agents(State(app): State<AppState>, s: OwnerSession) -> PageResult {
    let rows = blocking(&app.authsvc, AuthService::agents).await??;
    let now = app.authsvc.now();
    let mut body = String::from(
        r#"<p class="lead">Give each agent its own token. Its default access applies unless a collection or list has a grant of its own.</p>
<section aria-labelledby="agent-access"><h2 id="agent-access">Agent access</h2>"#,
    );
    if rows.is_empty() {
        body.push_str(
            r#"<p class="empty">No agents yet. Create a token below to give an agent access.</p>"#,
        );
    } else {
        body.push_str(r#"<ul class="rows" role="list">"#);
    }
    for a in &rows {
        let active = a.revoked_at.is_none();
        let actions = if active {
            format!(
                r#"<a class="button secondary" href="/admin/agents/{id}">Edit grants</a><form method="post" action="/admin/agents/{id}/revoke">{csrf}<button type="submit" class="danger">Revoke</button></form>"#,
                id = a.id,
                csrf = csrf_field(&s),
            )
        } else {
            String::new()
        };
        row(
            &mut body,
            a.agent.as_str(),
            &[
                format!("Default: {}", level_name(a.grants.default)),
                format!("{} writes/hour", a.writes_per_hour),
                agent_status(a, now),
                last_active(a.last_used_at, now),
            ],
            "",
            &actions,
        );
    }
    if !rows.is_empty() {
        body.push_str("</ul>");
    }
    let _ = write!(
        body,
        r#"</section><section aria-labelledby="new-agent">
<h2 id="new-agent">New agent token</h2>
<form class="stack" method="post" action="/admin/agents">{csrf}
<div class="form-grid"><label>Agent name <input name="name" required pattern="[a-z0-9_\-]{{1,64}}" title="lowercase letters, digits, - and _"></label>
<label>Default access {levels}</label>
<label>Writes per hour <input name="writes_per_hour" inputmode="numeric" placeholder="{wph}"></label>
<label>Expires after (days) <input name="expires_in_days" inputmode="numeric" placeholder="Never"></label></div>
<button type="submit">Create token</button>
</form></section>"#,
        csrf = csrf_field(&s),
        levels = level_select("default", Some(AccessLevel::Read), false),
        wph = app.authsvc.config().agent_writes_per_hour,
    );
    ok_page(
        "Agents",
        &owner_chrome(&app.authsvc, &s).in_section(Section::Agents),
        &body,
    )
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
<label>Agent token <input class="secret" readonly value="{token}"></label>
<p>The agent sends it as <code>Authorization: Bearer &lt;token&gt;</code>. Its changes are recorded as <code>agent:{agent}</code>.</p>
<p><a href="/admin/agents">Back to agents</a></p>"#,
        agent = esc(agent.as_str()),
        token = esc(&token),
    );
    ok_page(
        "Agent token created",
        &owner_chrome(&app.authsvc, &s).in_section(Section::Agents),
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
        r#"<p class="lead">Access for <strong>{agent}</strong>. A list's own level wins, then its collection's, then the default.</p>
<form class="stack grants" method="post" action="/admin/agents/{id}/grants">{csrf}
<label>Default access {default}</label>"#,
        agent = esc(a.agent.as_str()),
        id = a.id,
        csrf = csrf_field(&s),
        default = level_select("default", Some(a.grants.default), false),
    );
    if cols.is_empty() && lists.is_empty() {
        body.push_str(r#"<p class="empty">Create a list or collection in the app to set its access here.</p>"#);
    } else {
        body.push_str("<h2>Collection and list overrides</h2>");
    }
    let mut row = |label: String, name: String, current: Option<AccessLevel>, class: &str| {
        let _ = write!(
            body,
            r#"<label class="grant {class}"><span>{label}</span>{}</label>"#,
            level_select(&name, current, true)
        );
    };
    for (lid, col, name) in lists.iter().filter(|(_, c, _)| c.is_none()) {
        let _ = col;
        row(
            esc(name),
            format!("l:{lid}"),
            a.grants.lists.get(lid).copied(),
            "",
        );
    }
    for (cid, cname) in &cols {
        row(
            format!("<strong>{}</strong> (collection)", esc(cname)),
            format!("c:{cid}"),
            a.grants.collections.get(cid).copied(),
            "",
        );
        for (lid, _, name) in lists.iter().filter(|(_, c, _)| *c == Some(*cid)) {
            row(
                esc(name),
                format!("l:{lid}"),
                a.grants.lists.get(lid).copied(),
                "grant-list",
            );
        }
    }
    body.push_str(r#"<div class="actions"><button type="submit">Save grants</button><a class="button quiet" href="/admin/agents">Back to agents</a></div></form>"#);
    ok_page(
        "Agent grants",
        &owner_chrome(&app.authsvc, &s).in_section(Section::Agents),
        &body,
    )
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
        r#"<p class="lead">Anyone with a share link can see task titles, completion, due dates and steps in one list or collection. They cannot make changes.</p>
<section aria-labelledby="shared"><h2 id="shared">Shared lists and collections</h2>"#,
    );
    if rows.is_empty() {
        body.push_str(
            r#"<p class="empty">Nothing is shared yet. Create a read-only link below.</p>"#,
        );
    } else {
        body.push_str(r#"<ul class="rows" role="list">"#);
    }
    for r in &rows {
        let (status, actions) = match r.revoked_at {
            Some(at) => (format!("Revoked {}", date(at)), String::new()),
            None => (
                r.rotated_at
                    .map_or_else(|| "Active".into(), |at| format!("Rotated {}", date(at))),
                format!(
                    r#"<form method="post" action="/admin/shares/{id}/rotate">{csrf}<button type="submit" class="secondary">Rotate</button></form><form method="post" action="/admin/shares/{id}/revoke">{csrf}<button type="submit" class="danger">Revoke</button></form>"#,
                    id = r.id,
                    csrf = csrf_field(&s),
                ),
            ),
        };
        row(
            &mut body,
            &target_name(&state, r.target),
            &[format!("Created {}", date(r.created_at)), status],
            "",
            &actions,
        );
    }
    if !rows.is_empty() {
        body.push_str("</ul>");
    }
    body.push_str(r#"<p class="hint">Links are shown once. Rotating a link replaces it and invalidates the old one.</p></section>"#);
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
    body.push_str(r#"<section aria-labelledby="new-share"><h2 id="new-share">New share link</h2>"#);
    if options.is_empty() {
        body.push_str(
            r#"<p class="empty">Create a list or collection in the app before sharing it.</p>"#,
        );
    } else {
        let _ = write!(
            body,
            r#"<form class="fields" method="post" action="/admin/shares">{csrf}<label>List or collection <select name="target" required><option value="">Choose an item</option>{options}</select></label><button type="submit">Create link</button></form>"#,
            csrf = csrf_field(&s)
        );
    }
    body.push_str("</section>");
    ok_page(
        "Share links",
        &owner_chrome(&app.authsvc, &s).in_section(Section::Shares),
        &body,
    )
}

/// A form or query.
#[derive(Debug, Deserialize)]
pub struct NewShareForm {
    csrf: String,
    target: String,
}

fn link_shown_once(app: &AppState, s: &OwnerSession, title: &str, token: &str) -> PageResult {
    let body = format!(
        r#"<p>Copy the link now: it is shown only once.</p><label>Share link <input class="secret" readonly value="{}"></label><p><a href="/admin/shares">Back to share links</a></p>"#,
        esc(&app.authsvc.share_url(token))
    );
    ok_page(
        title,
        &owner_chrome(&app.authsvc, s).in_section(Section::Shares),
        &body,
    )
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
    let mut body = String::from(
        r#"<p class="lead">The latest 200 sign-ins, access changes and security events.</p>"#,
    );
    if rows.is_empty() {
        body.push_str(r#"<p class="empty">No events recorded yet. Sign-ins and access changes will appear here.</p>"#);
    } else {
        body.push_str(r#"<ul class="rows" role="list">"#);
        for r in &rows {
            let detail = audit_detail(&r.detail);
            let extra = format!(
                r#"<details><summary>Event details</summary><code class="audit-detail">{kind}{detail}</code></details>"#,
                kind = esc(&r.kind),
                detail = detail.map_or_else(String::new, |d| format!("\n{}", esc(&d)))
            );
            let mut facts = vec![date_time(r.at)];
            if let Some(ip) = &r.ip {
                facts.push(format!("From {}", esc(ip)));
            }
            let mut title = r.kind.replace('_', " ");
            if let Some(first) = title.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            row(&mut body, &title, &facts, &extra, "");
        }
        body.push_str("</ul>");
    }
    ok_page(
        "Audit log",
        &owner_chrome(&app.authsvc, &s).in_section(Section::Audit),
        &body,
    )
}

fn audit_detail(raw: &str) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let fields = value.as_object_mut()?;
    if let Some(issuer) = fields.get_mut("issuer") {
        *issuer = match issuer.get("kind").and_then(serde_json::Value::as_str) {
            Some("session") => "Browser session (passkey)".into(),
            Some("device") => "Paired device".into(),
            _ => "Not recorded".into(),
        };
    }
    if let Some(device) = fields.get_mut("device")
        && let Some(id) = device.as_str()
    {
        *device = short_id(id).into();
    }
    if let Some(devices) = fields
        .get_mut("descendant_devices")
        .and_then(serde_json::Value::as_array_mut)
    {
        for device in devices {
            if let Some(id) = device.as_str() {
                *device = short_id(id).into();
            }
        }
    }
    if let Some(actor) = fields.get_mut("actor")
        && actor
            .as_str()
            .is_some_and(|a| a.starts_with("Browser session ") || a.starts_with("Device "))
    {
        *actor = "Authenticated client".into();
    }
    if fields.is_empty() {
        return None;
    }
    serde_json::to_string_pretty(&value).ok()
}
