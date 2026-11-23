//! Tiny HTML helpers: escaping and the page layout. Every dynamic value goes through [`esc`].

use crate::api::error::ApiError;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use std::fmt::Write as _;

/// Escapes text for HTML element content and quoted attribute values.
#[must_use]
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The selected admin section, including forms and results within that section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Overview,
    Devices,
    Agents,
    Shares,
    Email,
    Audit,
}

const SECTIONS: [(Section, &str, &str); 6] = [
    (Section::Overview, "/admin", "Overview"),
    (Section::Devices, "/admin/devices", "Devices"),
    (Section::Agents, "/admin/agents", "Agents"),
    (Section::Shares, "/admin/shares", "Share links"),
    (Section::Email, "/admin/ingest", "Email"),
    (Section::Audit, "/admin/audit", "Audit log"),
];

/// What the page header shows.
#[derive(Debug, Clone, Default)]
pub struct Chrome<'a> {
    /// The session's CSRF token: the page is for a signed-in owner (navigation, sign-out form).
    pub csrf: Option<&'a str>,
    /// Load the passkey script.
    pub script: bool,
    /// `None` hides the banner. `Some(0)` needs step-up; otherwise minutes left at render time.
    pub(super) minutes_left: Option<i64>,
    pub(super) section: Option<Section>,
}

impl Chrome<'_> {
    pub(super) fn in_section(mut self, section: Section) -> Self {
        self.section = Some(section);
        self
    }
}

/// A full HTML page.
#[must_use]
pub fn page(title: &str, chrome: &Chrome<'_>, body: &str) -> String {
    let mut head = String::new();
    if let Some(csrf) = chrome.csrf {
        let _ = write!(head, r#"<meta name="csrf-token" content="{}">"#, esc(csrf));
    }
    if chrome.script || chrome.csrf.is_some() {
        head.push_str(r#"<script src="/assets/auth.js" defer></script>"#);
    }
    let mut nav = String::new();
    let mut banner = String::new();
    let (context, logout, layout) = if let Some(csrf) = chrome.csrf {
        nav.push_str(r#"<nav aria-label="Administration">"#);
        for (section, path, label) in SECTIONS {
            let current = if chrome.section == Some(section) {
                r#" aria-current="page""#
            } else {
                ""
            };
            let _ = write!(nav, r#"<a href="{path}"{current}>{label}</a>"#);
        }
        nav.push_str("</nav>");
        match chrome.minutes_left {
            Some(0) => banner.push_str(r#"<div class="session-note"><strong>Passkey check needed for changes</strong><button type="button" class="secondary" data-action="step-up">Verify with passkey</button></div>"#),
            Some(minutes) => {
                let unit = if minutes == 1 { "minute" } else { "minutes" };
                let _ = write!(banner, r#"<p class="session-note"><strong>Passkey verified</strong><span>About {minutes} {unit} left at page load.</span></p>"#);
            }
            None => {}
        }
        (
            r#"<span class="brand-context">Administration</span>"#,
            format!(
                r#"<form method="post" action="/auth/logout"><input type="hidden" name="csrf" value="{}"><button type="submit" class="quiet">Sign out</button></form>"#,
                esc(csrf)
            ),
            "shell",
        )
    } else {
        ("", String::new(), "shell narrow")
    };
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<title>{title}</title>
<link rel="stylesheet" href="/assets/tokens.css">
<link rel="stylesheet" href="/assets/admin.css">
{head}
</head>
<body>
<header class="site-header"><div class="{layout}"><div class="masthead"><div><strong class="brand">To-do</strong>{context}</div>{logout}</div>{nav}</div></header>
<main class="{layout}">
{banner}
<h1>{title}</h1>
<p id="status" role="status"></p>
<div class="page-body">{body}</div>
</main>
</body>
</html>
"#,
        title = esc(title),
    )
}

/// An HTML response.
#[must_use]
pub fn html(status: StatusCode, body: String) -> Response {
    let mut res = (status, body).into_response();
    res.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    res
}

/// An error shown as an HTML page.
#[derive(Debug)]
pub struct PageError {
    /// What went wrong.
    pub err: ApiError,
    /// The session's CSRF token, when the page offers an action that needs it. The "Verify with passkey"
    /// button on a `step_up_required` page posts to `/auth/step-up`, which the CSRF check refuses without it.
    pub csrf: Option<String>,
}

impl PageError {
    /// A step-up error page for a signed-in session, so its "Verify with passkey" button works.
    #[must_use]
    pub fn with_csrf(err: ApiError, csrf: &str) -> Self {
        Self {
            err,
            csrf: Some(csrf.to_owned()),
        }
    }
}

impl From<ApiError> for PageError {
    fn from(err: ApiError) -> Self {
        Self { err, csrf: None }
    }
}

impl From<crate::db::StoreError> for PageError {
    fn from(e: crate::db::StoreError) -> Self {
        Self::from(ApiError::from(e))
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        let e = self.err;
        let extra = match e.code {
            "step_up_required" => {
                r#"<p><button type="button" data-action="step-up" data-after="back">Verify with passkey, then go back</button></p>"#
            }
            "older_passkey_required" => {
                r#"<p><a href="/auth/verify?next=/admin/devices">Verify with an older passkey</a></p>"#
            }
            _ => "",
        };
        let body = format!(
            r#"<p class="error">{}</p>{extra}<p><a href="/admin">Back to the overview</a></p>"#,
            esc(&e.message)
        );
        let chrome = Chrome {
            csrf: self.csrf.as_deref(),
            script: true,
            ..Chrome::default()
        };
        let mut res = html(e.status, page("Something went wrong", &chrome, &body));
        if let Some(secs) = e.retry_after {
            res.headers_mut()
                .insert(axum::http::header::RETRY_AFTER, HeaderValue::from(secs));
        }
        res
    }
}

/// A readable date, with exact UTC timestamp and machine-readable datetime.
pub(super) fn date(ms: i64) -> String {
    time_element(ms, |t| t.strftime("%d %b %Y").to_string())
}

pub(super) fn date_time(ms: i64) -> String {
    time_element(ms, |t| t.strftime("%d %b %Y, %H:%M UTC").to_string())
}

pub(super) fn last_active(at: Option<i64>, now: i64) -> String {
    at.map_or_else(
        || "Never active".into(),
        |at| format!("Last active {}", relative_time(at, now)),
    )
}

pub(super) fn relative_time(at: i64, now: i64) -> String {
    let elapsed = now.saturating_sub(at).max(0) / 1000;
    time_element(at, |t| match elapsed {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", elapsed / 60),
        3600..=86_399 => format!("{} hr ago", elapsed / 3600),
        86_400..=604_799 => {
            let days = elapsed / 86_400;
            format!("{days} day{} ago", if days == 1 { "" } else { "s" })
        }
        _ => t.strftime("%d %b %Y").to_string(),
    })
}

fn time_element(ms: i64, label: impl FnOnce(jiff::Timestamp) -> String) -> String {
    jiff::Timestamp::from_millisecond(ms).map_or_else(
        |_| "Unknown date".into(),
        |t| {
            format!(
                r#"<time datetime="{t}" title="{}">{}</time>"#,
                t.strftime("%Y-%m-%d %H:%M:%S UTC"),
                esc(&label(t))
            )
        },
    )
}

/// Append one row to a flat list. `facts`, `extra` and `actions` are trusted HTML fragments.
pub(super) fn row(out: &mut String, title: &str, facts: &[String], extra: &str, actions: &str) {
    let _ = write!(
        out,
        r#"<li class="row"><div class="row-copy"><h3>{}</h3>"#,
        esc(title)
    );
    if !facts.is_empty() {
        out.push_str(r#"<div class="facts meta">"#);
        for (i, fact) in facts.iter().enumerate() {
            let sep = if i == 0 { "" } else { "· " };
            let _ = write!(out, "<span>{sep}{fact}</span>");
        }
        out.push_str("</div>");
    }
    out.push_str(extra);
    out.push_str("</div>");
    if !actions.is_empty() {
        let _ = write!(out, r#"<div class="row-actions actions">{actions}</div>"#);
    }
    out.push_str("</li>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_markup_and_quotes() {
        assert_eq!(
            esc(r#"<a href="x" onclick='y'>&</a>"#),
            "&lt;a href=&quot;x&quot; onclick=&#39;y&#39;&gt;&amp;&lt;/a&gt;"
        );
    }

    #[test]
    fn owner_pages_carry_the_csrf_token_and_script() {
        let p = page(
            "T<",
            &Chrome {
                csrf: Some("tok\""),
                script: false,
                minutes_left: Some(0),
                section: Some(Section::Overview),
            },
            "",
        );
        assert!(p.contains("<title>T&lt;</title>"));
        assert!(p.contains(r#"content="tok&quot;""#));
        assert!(p.contains("/assets/auth.js"));
        assert!(p.contains("data-action=\"step-up\""));
        assert_eq!(p.matches("aria-current=\"page\"").count(), 1);
    }

    #[test]
    fn dates_keep_the_exact_utc_time_out_of_the_visible_relative_label() {
        let actual = last_active(Some(0), 120_000);
        assert_eq!(
            actual,
            r#"Last active <time datetime="1970-01-01T00:00:00Z" title="1970-01-01 00:00:00 UTC">2 min ago</time>"#
        );
        assert!(date(0).contains(">01 Jan 1970</time>"));
        assert_eq!(last_active(None, 120_000), "Never active");
    }
}
