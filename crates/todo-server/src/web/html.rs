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

/// What the page header shows.
#[derive(Debug, Clone, Default)]
pub struct Chrome<'a> {
    /// The session's CSRF token: the page is for a signed-in owner (navigation, sign-out form).
    pub csrf: Option<&'a str>,
    /// Load the passkey script.
    pub script: bool,
    /// Whether the session verified a passkey within the step-up window.
    pub fresh: Option<bool>,
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
    if let Some(csrf) = chrome.csrf {
        let _ = write!(
            nav,
            r#"<nav><a href="/admin">Overview</a><a href="/admin/devices">Devices</a><a href="/admin/agents">Agents</a><a href="/admin/shares">Share links</a><a href="/admin/audit">Audit log</a>
<form method="post" action="/auth/logout" class="inline"><input type="hidden" name="csrf" value="{}"><button type="submit" class="link">Sign out</button></form></nav>"#,
            esc(csrf)
        );
        match chrome.fresh {
            Some(true) => nav.push_str(
                r#"<p class="verified">Verified with your passkey: changes are allowed for a few minutes.</p>"#,
            ),
            Some(false) => nav.push_str(
                r#"<p class="verify">Changes need a recent passkey check. <button type="button" data-action="step-up">Verify with passkey</button></p>"#,
            ),
            None => {}
        }
    }
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<title>{title}</title>
<link rel="stylesheet" href="/assets/app.css">
{head}
</head>
<body>
<header><strong>To-do</strong>{nav}</header>
<main>
<h1>{title}</h1>
<p id="status" role="status"></p>
{body}
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
pub struct PageError(pub ApiError);

impl From<ApiError> for PageError {
    fn from(e: ApiError) -> Self {
        Self(e)
    }
}

impl From<crate::db::StoreError> for PageError {
    fn from(e: crate::db::StoreError) -> Self {
        Self(e.into())
    }
}

impl IntoResponse for PageError {
    fn into_response(self) -> Response {
        let e = self.0;
        let extra = if e.code == "step_up_required" {
            r#"<p><button type="button" data-action="step-up" data-after="back">Verify with passkey, then go back</button></p>"#
        } else {
            ""
        };
        let body = format!(
            r#"<p class="error">{}</p>{extra}<p><a href="/admin">Back to the overview</a></p>"#,
            esc(&e.message)
        );
        let chrome = Chrome {
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

/// Formats Unix milliseconds as an RFC 3339 UTC time (seconds precision).
#[must_use]
pub fn time(ms: i64) -> String {
    jiff::Timestamp::from_millisecond(ms)
        .map(|t| t.strftime("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|_| "?".into())
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
                fresh: Some(false),
            },
            "",
        );
        assert!(p.contains("<title>T&lt;</title>"));
        assert!(p.contains(r#"content="tok&quot;""#));
        assert!(p.contains("/assets/auth.js"));
        assert!(p.contains("data-action=\"step-up\""));
    }
}
