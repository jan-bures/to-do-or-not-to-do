//! Shared web picker controls. Commands still come from the shared client intents.

use super::{Ctx, icon, platform, typing};
use leptos::html;
use leptos::prelude::*;
use todo_core::ids::TaskId;
use todo_design::{Icon, hues::SWATCHES};
use wasm_bindgen::JsCast;

#[component]
pub(super) fn Swatches(
    selected: Signal<Option<&'static str>>,
    choose: Callback<&'static str>,
    disabled: Signal<bool>,
) -> impl IntoView {
    view! {
        <div class="swatches" role="group" aria-label="Colour swatches">
            {SWATCHES.into_iter().map(|swatch| view! {
                <button type="button" data-picker-option="" data-list-hue=swatch.key
                    aria-label=swatch.key title=swatch.key disabled=move || disabled.get()
                    aria-pressed=move || (selected.get() == Some(swatch.key)).to_string()
                    on:click=move |_| choose.run(swatch.key)>
                    <span class="swatch-disc" aria-hidden="true"></span>
                    <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
                </button>
            }).collect_view()}
        </div>
    }
}

/// Arrow/Home/End move through the current page's actions, not any controls behind it.
pub(super) fn choice_key(id: &str, columns: usize, ev: &leptos::ev::KeyboardEvent) {
    if ev.is_composing() || ev.alt_key() || ev.ctrl_key() || ev.meta_key() || typing(ev) {
        return;
    }
    let key = ev.key();
    if !matches!(
        key.as_str(),
        "ArrowUp" | "ArrowDown" | "Home" | "End" | "ArrowLeft" | "ArrowRight"
    ) {
        return;
    }
    if columns == 1 && matches!(key.as_str(), "ArrowLeft" | "ArrowRight") {
        return;
    }
    let doc = document();
    let Some(panel) = doc.get_element_by_id(id) else {
        return;
    };
    let Ok(nodes) = panel.query_selector_all("[data-picker-option]:not(:disabled)") else {
        return;
    };
    let items: Vec<_> = (0..nodes.length())
        .filter_map(|i| nodes.item(i)?.dyn_into::<web_sys::HtmlElement>().ok())
        .collect();
    if items.is_empty() {
        return;
    }
    let at = doc.active_element().and_then(|el| {
        items
            .iter()
            .position(|b| b.unchecked_ref::<web_sys::Element>() == &el)
    });
    let n = items.len();
    let next = match (key.as_str(), at) {
        ("End", _) => n - 1,
        ("Home", _) | (_, None) => 0,
        ("ArrowUp", Some(i)) => (i + n - columns.min(n)) % n,
        ("ArrowDown", Some(i)) => (i + columns.min(n)) % n,
        ("ArrowLeft", Some(i)) => (i + n - 1) % n,
        (_, Some(i)) => (i + 1) % n,
    };
    ev.prevent_default();
    if !ev.repeat() {
        let _ = items[next].focus();
    }
}

#[derive(Clone, Copy)]
pub(super) struct DetailPicker {
    pub(super) node: NodeRef<html::Div>,
    pub(super) open: RwSignal<bool>,
    pub(super) anchor: &'static str,
    pub(super) id: &'static str,
}

impl DetailPicker {
    pub(super) fn new(c: Ctx, task: TaskId, anchor: &'static str, id: &'static str) -> Self {
        let this = Self {
            node: NodeRef::new(),
            open: RwSignal::new(false),
            anchor,
            id,
        };
        Effect::new(move |_| {
            if this.open.get() && (c.selected.get() != Some(task) || c.read_only()) {
                this.close(false);
            }
        });
        let resize = window_event_listener(leptos::ev::resize, move |_| this.position());
        on_cleanup(move || resize.remove());
        this
    }

    pub(super) fn show(self, first: &'static str) {
        if let Some(el) = self.node.get_untracked()
            && el.show_popover().is_ok()
        {
            self.open.set(true);
            self.focus_page(first);
        }
    }

    pub(super) fn focus_page(self, first: &'static str) {
        request_animation_frame(move || {
            self.position();
            if self.open.try_get_untracked() == Some(true) {
                platform::focus(first);
            }
        });
    }

    pub(super) fn close(self, focus: bool) {
        self.open.set(false);
        if let Some(el) = self.node.get_untracked() {
            let _ = el.hide_popover();
        }
        if focus {
            platform::focus(self.anchor);
        }
    }

    pub(super) fn toggle_event(self) {
        if let Some(el) = self.node.get_untracked() {
            self.open.set(el.matches(":popover-open").unwrap_or(false));
        }
    }

    pub(super) fn position(self) {
        if self.open.try_get_untracked() == Some(true)
            && let Some(el) = self.node.get_untracked()
            && !platform::place_room_menu(&el, self.anchor)
        {
            self.close(false);
        }
    }

    pub(super) fn key(self, c: Ctx, columns: Option<usize>, ev: &leptos::ev::KeyboardEvent) {
        ev.stop_propagation();
        if ev.is_composing() || ev.default_prevented() {
            return;
        }
        match ev.key().as_str() {
            "z" if ev.ctrl_key()
                && !ev.alt_key()
                && !ev.shift_key()
                && !typing(ev)
                && !c.read_only() =>
            {
                ev.prevent_default();
                if !ev.repeat() {
                    self.close(true);
                    c.undo();
                }
            }
            "Escape" => {
                ev.prevent_default();
                if !ev.repeat() {
                    self.close(true);
                }
            }
            "Tab" => platform::contain_tab(self.id, ev),
            "Enter" if ev.repeat() => ev.prevent_default(),
            _ => {
                if let Some(n) = columns {
                    choice_key(self.id, n, ev);
                }
            }
        }
    }
}

/// Start a new choice page on its current value, or its first enabled option.
pub(super) fn focus_choice(id: &'static str, fallback: &'static str) {
    request_animation_frame(move || {
        let Some(panel) = document()
            .get_element_by_id(id)
            .filter(|el| el.matches(":popover-open").unwrap_or(false))
        else {
            return;
        };
        let find = |selector| {
            panel
                .query_selector(selector)
                .ok()
                .flatten()
                .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        };
        if let Some(choice) = find("[data-picker-option][aria-pressed=true]:not(:disabled)")
            .or_else(|| find("[data-picker-option]:not(:disabled)"))
        {
            let _ = choice.focus();
        } else {
            platform::focus(fallback);
        }
    });
}

pub(super) fn position_detail_pickers() {
    for (id, anchor) in [
        ("repeat-picker", "edit-task-repeat"),
        ("tag-picker", "edit-task-tags"),
    ] {
        if let Some(el) = document()
            .get_element_by_id(id)
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
            .filter(|el| el.matches(":popover-open").unwrap_or(false))
            && !platform::place_room_menu(&el, anchor)
        {
            let _ = el.hide_popover();
        }
    }
}
