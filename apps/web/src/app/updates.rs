//! The deployed bundle can change while this tab has a local outbox and unsent editors.

use super::{Ctx, backend, ctx, icon};
use leptos::prelude::*;
use std::{cell::Cell, rc::Rc, time::Duration};
use todo_design::Icon;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortSignal, HtmlInputElement, Request, RequestCache, RequestInit, RequestMode,
    RequestRedirect, Response, Window,
};

fn is_version(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn running_version() -> Option<String> {
    let version = web_sys::window()?
        .document()?
        .query_selector("meta[name=todo-web-version]")
        .ok()??
        .get_attribute("content")?;
    is_version(&version).then_some(version)
}

async fn deployed_version() -> Option<String> {
    let window = web_sys::window()?;
    let init = RequestInit::new();
    init.set_cache(RequestCache::NoStore);
    init.set_mode(RequestMode::SameOrigin);
    // A signed-out tab sees a redirect to sign-in, not a new app version.
    init.set_redirect(RequestRedirect::Manual);
    init.set_signal(Some(&AbortSignal::timeout_with_u32(10_000)));
    let req = Request::new_with_str_and_init("/app/version.txt", &init).ok()?;
    let response: Response = JsFuture::from(window.fetch_with_request(&req))
        .await
        .ok()?
        .dyn_into()
        .ok()?;
    if !response.ok() {
        return None;
    }
    let text = JsFuture::from(response.text().ok()?)
        .await
        .ok()?
        .as_string()?;
    let version = text.trim();
    is_version(version).then(|| version.to_owned())
}

fn has_draft(window: &Window, c: Ctx) -> bool {
    if c.editing.get_untracked() || c.new_room.get_untracked().is_some() || c.ai.has_draft() {
        return true;
    }
    let Some(doc) = window.document() else {
        return false;
    };
    // Editors that save on blur may still be open when clicked with a pointer. A dialog, chosen
    // quick-add options, or text in the Add link field can also hold work outside the outbox.
    if doc
        .query_selector(
            "dialog[open], .title-input, .step-input, textarea.notes, .qa-chips.has-chips",
        )
        .ok()
        .flatten()
        .is_some()
    {
        return true;
    }
    doc.query_selector_all(".qa-input, #add-step, #add-task-link")
        .ok()
        .is_some_and(|fields| {
            (0..fields.length()).any(|i| {
                fields
                    .item(i)
                    .and_then(|node| node.dyn_into::<HtmlInputElement>().ok())
                    .is_some_and(|field| !field.value().is_empty())
            })
        })
}

#[component]
pub(super) fn UpdateFooter() -> impl IntoView {
    let c = ctx();
    let ready = RwSignal::new(false);
    if let Some(running) = running_version() {
        let checking = Rc::new(Cell::new(false));
        let check = move || {
            if checking.replace(true) {
                return;
            }
            let running = running.clone();
            let checking = checking.clone();
            leptos::task::spawn_local(async move {
                if let Some(deployed) = deployed_version().await {
                    let _ = ready.try_set(deployed != running);
                }
                checking.set(false);
            });
        };
        check();
        let tick = set_interval_with_handle(check.clone(), Duration::from_mins(10)).ok();
        let focus = window_event_listener(leptos::ev::focus, move |_| check());
        on_cleanup(move || {
            if let Some(tick) = tick {
                tick.clear();
            }
            focus.remove();
        });
    }
    let saving = RwSignal::new(false);
    view! {
        <Show when=move || ready.get()>
            <div class="app-update" role="status">
                <button type="button" class="button app-update-action"
                    aria-disabled=move || saving.get().then_some("true")
                    on:mousedown=move |ev| { ev.prevent_default(); }
                    on:click=move |_| {
                        if saving.get_untracked() { return; }
                        saving.set(true);
                        leptos::task::spawn_local(async move {
                            if !backend::flush_for_reload().await {
                                c.show_toast("Couldn’t save changes yet. Try reloading again once they’re saved.".into(), false);
                            } else if let Some(window) = web_sys::window()
                                && (!has_draft(&window, c) || window.confirm_with_message(
                                    "Reload now? Any draft or open edit that hasn’t been saved will be lost."
                                ).unwrap_or(false)) {
                                let _ = window.location().reload();
                            }
                            saving.set(false);
                        });
                    }>
                    {icon(Icon::ArrowUp)}
                    {move || if saving.get() { "Saving changes…" } else { "Reload to update" }}
                </button>
            </div>
        </Show>
    }
}
