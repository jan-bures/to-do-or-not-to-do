//! Small browser helpers: the clock, the theme, the address bar, the CSRF token and downloads.

use crate::model::Now;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::Closure;

fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

fn document() -> Option<web_sys::Document> {
    window().and_then(|w| w.document())
}

/// The real "now": the instant and the browser's current UTC offset.
#[must_use]
pub fn now() -> Now {
    #[allow(clippy::cast_possible_truncation)]
    let offset_s = (-js_sys::Date::new_0().get_timezone_offset() * 60.0) as i32;
    Now {
        at: todo_client::web::now(),
        offset_s,
    }
}

/// Milliseconds since the page loaded, for animations.
#[must_use]
pub fn clock_ms() -> f64 {
    window()
        .and_then(|w| w.performance())
        .map_or_else(js_sys::Date::now, |p| p.now())
}

/// The CSRF token the server put in the page.
#[must_use]
pub fn csrf() -> Option<String> {
    let meta = document()?
        .query_selector(r#"meta[name="csrf-token"]"#)
        .ok()??;
    let token = meta.get_attribute("content")?;
    (!token.is_empty()).then_some(token)
}

fn media(query: &str) -> Option<web_sys::MediaQueryList> {
    window()?.match_media(query).ok()?
}

/// Whether the system prefers the dark theme.
#[must_use]
pub fn prefers_dark() -> bool {
    media("(prefers-color-scheme: dark)").is_some_and(|m| m.matches())
}

/// Whether the window has the phone layout (see `app.css`).
#[must_use]
pub fn is_phone() -> bool {
    media("(max-width: 759px)").is_some_and(|m| m.matches())
}

/// Whether the system asks for reduced motion.
#[must_use]
pub fn reduced_motion() -> bool {
    media("(prefers-reduced-motion: reduce)").is_some_and(|m| m.matches())
}

/// Calls `f` whenever the colour scheme preference changes. Lives as long as the page.
pub fn on_theme_change(f: impl Fn(bool) + 'static) {
    if let Some(m) = media("(prefers-color-scheme: dark)") {
        let cb = Closure::<dyn Fn(web_sys::MediaQueryListEvent)>::new(
            move |e: web_sys::MediaQueryListEvent| f(e.matches()),
        );
        let _ = m.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

/// Sets an attribute on `<html>`.
pub fn set_root_attr(name: &str, value: &str) {
    if let Some(el) = document().and_then(|d| d.document_element()) {
        let _ = el.set_attribute(name, value);
    }
}

/// The current path.
#[must_use]
pub fn path() -> String {
    window()
        .and_then(|w| w.location().pathname().ok())
        .unwrap_or_default()
}

/// The current query string (`?fixture=...`).
#[must_use]
pub fn query() -> String {
    window()
        .and_then(|w| w.location().search().ok())
        .unwrap_or_default()
}

/// Puts `path` in the address bar (keeping the query), without reloading.
pub fn push_path(path: &str) {
    if self::path() == path {
        return;
    }
    if let Some(w) = window() {
        let url = format!("{path}{}", query());
        let _ = w
            .history()
            .and_then(|h| h.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url)));
    }
}

/// Calls `f` when the user goes back or forward.
pub fn on_pop_state(f: impl Fn() + 'static) {
    if let Some(w) = window() {
        let cb = Closure::<dyn Fn()>::new(f);
        let _ = w.add_event_listener_with_callback("popstate", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

/// Calls `f` when the tab becomes visible again or the network comes back.
pub fn on_wake(f: impl Fn() + 'static) {
    let f = std::rc::Rc::new(f);
    if let Some(d) = document() {
        let g = f.clone();
        let cb = Closure::<dyn Fn()>::new(move || {
            if document().is_some_and(|d| d.visibility_state() == web_sys::VisibilityState::Visible)
            {
                g();
            }
        });
        let _ = d.add_event_listener_with_callback("visibilitychange", cb.as_ref().unchecked_ref());
        cb.forget();
    }
    if let Some(w) = window() {
        let cb = Closure::<dyn Fn()>::new(move || f());
        let _ = w.add_event_listener_with_callback("online", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

/// While the page is open: asks before it closes if `unsaved()` (a change not stored in this browser
/// yet), and calls `hidden` when it is hidden or put away (to write what is pending).
#[cfg_attr(feature = "fixtures", allow(dead_code))]
pub fn on_leave(unsaved: impl Fn() -> bool + 'static, hidden: impl Fn() + 'static) {
    let Some(w) = window() else { return };
    let ask = Closure::<dyn Fn(web_sys::Event)>::new(move |e: web_sys::Event| {
        if unsaved() {
            e.prevent_default();
            // Older browsers want `returnValue` set as well.
            let _ = js_sys::Reflect::set(&e, &"returnValue".into(), &"".into());
        }
    });
    let _ = w.add_event_listener_with_callback("beforeunload", ask.as_ref().unchecked_ref());
    ask.forget();
    let hide = Closure::<dyn Fn()>::new(hidden);
    let _ = w.add_event_listener_with_callback("pagehide", hide.as_ref().unchecked_ref());
    hide.forget();
}

/// Shows a notice while the app is starting (before it mounts), or removes it (`None`).
#[cfg_attr(feature = "fixtures", allow(dead_code))]
pub fn boot_notice(text: Option<&str>) {
    let Some(d) = document() else { return };
    if let Some(old) = d.get_element_by_id("boot-notice") {
        old.remove();
    }
    let (Some(text), Some(body)) = (text, d.body()) else {
        return;
    };
    if let Ok(p) = d.create_element("p") {
        p.set_id("boot-notice");
        p.set_class_name("fatal boot-notice");
        let _ = p.set_attribute("role", "status");
        p.set_text_content(Some(text));
        let _ = body.append_child(&p);
    }
}

/// Reloads the page.
#[cfg_attr(feature = "fixtures", allow(dead_code))]
pub fn reload() {
    if let Some(w) = window() {
        let _ = w.location().reload();
    }
}

/// Sends the browser to the sign-in page, back to `next` afterwards.
pub fn sign_in(next: &str) {
    if let Some(w) = window() {
        let next = js_sys::encode_uri_component(next);
        let _ = w.location().set_href(&format!("/login?next={next}"));
    }
}

/// Downloads an attachment: `fetch` with the session and CSRF header, then a blob link. (A plain link
/// can't carry the CSRF header the API requires.)
pub async fn download(id: String, file_name: String) {
    use wasm_bindgen_futures::JsFuture;
    let Some(w) = window() else { return };
    let Some(token) = crate::backend::csrf() else {
        return;
    };
    let init = web_sys::RequestInit::new();
    init.set_credentials(web_sys::RequestCredentials::SameOrigin);
    let headers = web_sys::Headers::new().ok();
    if let Some(h) = &headers {
        let _ = h.set(todo_client::web::CSRF_HEADER, &token);
        init.set_headers(h);
    }
    let url = format!("/api/v1/attachments/{id}");
    let Ok(resp) = JsFuture::from(w.fetch_with_str_and_init(&url, &init)).await else {
        return;
    };
    let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
        return;
    };
    if !resp.ok() {
        tracing::warn!("attachment download failed: {}", resp.status());
        return;
    }
    let Ok(blob) = resp.blob() else { return };
    let Ok(blob) = JsFuture::from(blob).await else {
        return;
    };
    let Ok(blob) = blob.dyn_into::<web_sys::Blob>() else {
        return;
    };
    let Ok(href) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return;
    };
    if let Some(a) = document()
        .and_then(|d| d.create_element("a").ok())
        .and_then(|a| a.dyn_into::<web_sys::HtmlAnchorElement>().ok())
    {
        a.set_href(&href);
        a.set_download(&file_name);
        a.click();
    }
    let _ = web_sys::Url::revoke_object_url(&href);
}

/// Watches an element's width.
pub fn observe_width(el: &web_sys::Element, f: impl Fn(f64) + 'static) {
    let cb = Closure::<dyn Fn(js_sys::Array)>::new(move |entries: js_sys::Array| {
        if let Ok(e) = entries.get(0).dyn_into::<web_sys::ResizeObserverEntry>() {
            f(e.content_rect().width());
        }
    });
    if let Ok(obs) = web_sys::ResizeObserver::new(cb.as_ref().unchecked_ref()) {
        obs.observe(el);
        // The observer lives as long as the page; the header element is never replaced.
        std::mem::forget(obs);
    }
    cb.forget();
}
