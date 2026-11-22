//! Small browser helpers: the clock, the theme, the address bar, the CSRF token and downloads.

use crate::model::Now;
use std::collections::BTreeSet;
use todo_core::ids::CollectionId;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::{JsCast, JsValue};

fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

fn document() -> Option<web_sys::Document> {
    window().and_then(|w| w.document())
}

const FOLDED_COLLECTIONS: &str = "todo-folded-collections";

/// A browser may throw even when merely accessing localStorage. Folding still works in memory.
pub fn load_folded() -> BTreeSet<CollectionId> {
    window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item(FOLDED_COLLECTIONS).ok().flatten())
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default()
}

pub fn save_folded(ids: &BTreeSet<CollectionId>) {
    if let Some(storage) = window().and_then(|w| w.local_storage().ok().flatten())
        && let Ok(value) = serde_json::to_string(ids)
    {
        let _ = storage.set_item(FOLDED_COLLECTIONS, &value);
    }
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

/// Whether the detail overlays the room (see `app.css`).
pub fn detail_overlays() -> bool {
    media("(max-width: 1199px)").is_some_and(|m| m.matches())
}

/// Calls `f` whenever the media query changes. Lives as long as the page.
pub fn on_media_change(query: &str, f: impl Fn(bool) + 'static) {
    if let Some(m) = media(query) {
        let cb = Closure::<dyn Fn(web_sys::MediaQueryListEvent)>::new(
            move |e: web_sys::MediaQueryListEvent| f(e.matches()),
        );
        let _ = m.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        cb.forget();
    }
}

/// Focuses a visible control or a programmatic focus target.
pub fn focus(id: &str) -> bool {
    document()
        .and_then(|d| d.get_element_by_id(id))
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        .is_some_and(|el| {
            if el.get_bounding_client_rect().width() == 0.0 {
                return false;
            }
            let _ = el.focus();
            document().and_then(|d| d.active_element()).as_ref() == Some(el.as_ref())
        })
}

/// Focus and select the existing name in a rename dialog.
pub fn focus_select_text(id: &str) {
    if focus(id)
        && let Some(input) = document()
            .and_then(|d| d.get_element_by_id(id))
            .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        input.select();
    }
}

/// Restores focus after an optimistic update replaced a row, but not if focus has moved elsewhere.
pub fn refocus_row(id: &str) {
    if document()
        .and_then(|doc| doc.active_element())
        .is_none_or(|el| el.tag_name() == "BODY" || el.id() == id)
    {
        focus(id);
    }
}

fn method(el: &web_sys::Element, name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(el, &JsValue::from_str(name))
        .ok()?
        .dyn_into()
        .ok()
}

/// Opens the native priority picker if supported; otherwise its normal keys work from focus.
pub fn focus_select(id: &str) {
    if focus(id)
        && let Some(el) = document().and_then(|doc| doc.get_element_by_id(id))
        && let Some(picker) = method(&el, "showPicker")
    {
        let _ = picker.call0(&el);
    }
}

/// Captures current row positions, including in-flight nudges, before the command moves them.
pub fn row_positions() -> Vec<(web_sys::Element, f64)> {
    if reduced_motion() {
        return Vec::new();
    }
    let Some(rows) = document().and_then(|doc| {
        doc.query_selector_all(".rows:not(.completed-rows) > .row:not(.done)")
            .ok()
    }) else {
        return Vec::new();
    };
    (0..rows.length())
        .filter_map(|i| rows.item(i)?.dyn_into::<web_sys::Element>().ok())
        .map(|row| {
            let y = row.get_bounding_client_rect().top();
            (row, y)
        })
        .collect()
}

/// Slides the moved rows to their new slots without changing the DOM order or inline styles.
pub fn animate_reorder(before: Vec<(web_sys::Element, f64)>) {
    use todo_design::motion;
    const NUDGE: &str = "task-nudge";
    if before.is_empty() || reduced_motion() {
        return;
    }
    let options = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &options,
        &"duration".into(),
        &f64::from(motion::ROW_ENTER).into(),
    );
    let _ = js_sys::Reflect::set(&options, &"easing".into(), &motion::STANDARD.css().into());
    for (row, y) in before.into_iter().filter(|(r, _)| r.is_connected()) {
        let (Some(animate), Some(animations)) =
            (method(&row, "animate"), method(&row, "getAnimations"))
        else {
            continue;
        };
        if let Ok(animations) = animations
            .call0(&row)
            .and_then(JsCast::dyn_into::<js_sys::Array>)
        {
            for animation in animations
                .iter()
                .filter_map(|a| a.dyn_into::<web_sys::Animation>().ok())
            {
                if animation.id() == NUDGE {
                    animation.cancel();
                }
            }
        }
        let dy = y - row.get_bounding_client_rect().top();
        if dy.abs() >= 1.0 {
            let frames = js_sys::Object::new();
            let transform =
                js_sys::Array::of2(&format!("translateY({dy}px)").into(), &"none".into());
            let _ = js_sys::Reflect::set(&frames, &"transform".into(), &transform);
            if let Ok(animation) = animate
                .call2(&row, &frames, &options)
                .and_then(JsCast::dyn_into::<web_sys::Animation>)
            {
                animation.set_id(NUDGE);
            }
        }
    }
}

/// Wraps Tab within the active overlay. The rest of the shell is inert.
pub fn contain_tab(id: &str, ev: &web_sys::KeyboardEvent) {
    let Some(doc) = document() else { return };
    let Some(panel) = doc.get_element_by_id(id) else {
        return;
    };
    let Ok(nodes) = panel.query_selector_all(
        ":is(a[href], button, input, textarea, select, summary, [tabindex]:not([tabindex=\"-1\"]), \
             [contenteditable]:not([contenteditable=false])):not(:disabled, [type=hidden], [inert], [inert] *)",
    ) else { return };
    let mut controls = (0..nodes.length())
        .filter_map(|i| nodes.item(i)?.dyn_into::<web_sys::HtmlElement>().ok())
        .filter(|el| {
            el.get_bounding_client_rect().width() > 0.0
                && el
                    .get_attribute("tabindex")
                    .is_none_or(|_| el.tab_index() >= 0)
        });
    let Some(first) = controls.next() else { return };
    let last = controls.next_back().unwrap_or_else(|| first.clone());
    let active = doc.active_element();
    let active = active.as_ref();
    let at_boundary = if ev.shift_key() {
        active == Some(first.as_ref()) || active == Some(&panel)
    } else {
        active == Some(last.as_ref())
    };
    if at_boundary || !panel.contains(active.map(AsRef::as_ref)) {
        ev.prevent_default();
        let _ = if ev.shift_key() {
            last.focus()
        } else {
            first.focus()
        };
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
