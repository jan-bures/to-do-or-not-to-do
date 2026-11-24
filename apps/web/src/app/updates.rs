//! The deployed bundle can change while this tab has a local outbox and unsent editors.

use super::{backend, ctx, icon};
use leptos::prelude::*;
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Weak},
    time::Duration,
};
use todo_design::Icon;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    AbortSignal, Request, RequestCache, RequestInit, RequestMode, RequestRedirect, Response,
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

type Dirty = dyn Fn() -> bool + Send + Sync;

/// The app-wide dirty flag combines editor-owned draft state. An editor unregisters when its
/// reactive owner is cleaned up, so a closed editor can't leave a stale reload warning behind.
#[derive(Clone, Copy)]
pub(super) struct Drafts(StoredValue<Vec<Weak<Dirty>>>);

impl Drafts {
    pub fn new() -> Self {
        Self(StoredValue::new(Vec::new()))
    }

    /// Read the editor's existing state synchronously when the user asks to reload.
    pub fn watch(self, dirty: impl Fn() -> bool + Send + Sync + 'static) {
        let dirty: Arc<Dirty> = Arc::new(dirty);
        self.0.update_value(|flags| {
            flags.retain(|weak| weak.strong_count() != 0);
            flags.push(Arc::downgrade(&dirty));
        });
        on_cleanup(move || drop(dirty));
    }

    /// For uncontrolled fields and compound forms: set on input, clear on save or cancel.
    pub fn flag(self) -> RwSignal<bool> {
        let dirty = RwSignal::new(false);
        self.watch(move || dirty.try_get_untracked().unwrap_or(false));
        dirty
    }

    fn dirty(self) -> bool {
        self.0
            .with_value(|flags| flags.iter().filter_map(Weak::upgrade).any(|dirty| dirty()))
    }
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
                            let result = backend::flush_for_reload().await;
                            if result == backend::Reload::SaveFailed {
                                c.show_reload_save_failed();
                            } else if let Some(window) = web_sys::window() {
                                let warning = if result == backend::Reload::InMemoryChanges {
                                    Some("Reload now? In-memory changes, including any draft or open edit, will be lost.")
                                } else if c.drafts.dirty() {
                                    Some("Reload now? Any draft or open edit that hasn’t been saved will be lost.")
                                } else {
                                    None
                                };
                                if warning.is_none_or(|message| window.confirm_with_message(message).unwrap_or(false)) {
                                    let _ = window.location().reload();
                                }
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
