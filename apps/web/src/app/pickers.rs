//! Shared web picker controls. Commands still come from the shared client intents.

use super::{Ctx, icon, platform, typing};
use leptos::html;
use leptos::prelude::*;
use todo_client::intent::organize::swatch_name;
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
        <div class="swatches" role="listbox" aria-label="Colour swatches" on:focusin=choice_focus>
            {SWATCHES.into_iter().map(|swatch| view! {
                <button type="button" data-picker-option="" data-list-hue=swatch.key role="option" tabindex="-1"
                    aria-label=swatch_name(swatch.key) title=swatch_name(swatch.key) disabled=move || disabled.get()
                    aria-selected=move || (selected.get() == Some(swatch.key)).to_string()
                    on:click=move |_| choose.run(swatch.key)>
                    <span class="swatch-disc" aria-hidden="true"></span>
                    <span class="picker-tick" aria-hidden="true">{icon(Icon::Check)}</span>
                </button>
            }).collect_view()}
        </div>
    }
}

/// Arrow keys move focus without committing a choice; only click, Enter or Space selects it.
/// Keep one Tab stop in each listbox, without changing which value is selected.
pub(super) fn choice_focus(ev: leptos::ev::FocusEvent) {
    let Some(target) = ev
        .target()
        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
    else {
        return;
    };
    if target
        .matches("[role=option]:not(:disabled)")
        .unwrap_or(false)
        && let Ok(Some(group)) = target.closest("[role=listbox]")
    {
        if let Ok(Some(previous)) = group.query_selector("[role=option][tabindex='0']") {
            let _ = previous.set_attribute("tabindex", "-1");
        }
        target.set_tab_index(0);
    }
}

/// Arrow/Home/End move through the current page's actions, not any controls behind it.
pub(super) fn choice_key(id: &str, columns: usize, ev: &leptos::ev::KeyboardEvent) {
    if !typing(ev) {
        move_choice_focus(id, columns, ev);
    }
}

/// In a searchable picker, Up and Down leave the filter; the other keys still edit its text.
pub(super) fn choice_key_from_filter(id: &str, ev: &leptos::ev::KeyboardEvent) {
    if !typing(ev) || matches!(ev.key().as_str(), "ArrowUp" | "ArrowDown") {
        move_choice_focus(id, 1, ev);
    }
}

fn move_choice_focus(id: &str, columns: usize, ev: &leptos::ev::KeyboardEvent) {
    if ev.is_composing() || ev.alt_key() || ev.ctrl_key() || ev.meta_key() {
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
        ("End", _) | ("ArrowUp", None) => n - 1,
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
        let this = Self::anchored(anchor, id);
        Effect::new(move |_| {
            if this.open.get() && (c.selected.get() != Some(task) || c.read_only()) {
                this.close(false);
            }
        });
        this
    }

    pub(super) fn anchored(anchor: &'static str, id: &'static str) -> Self {
        let this = Self {
            node: NodeRef::new(),
            open: RwSignal::new(false),
            anchor,
            id,
        };
        let resize = window_event_listener(leptos::ev::resize, move |_| this.position());
        on_cleanup(move || resize.remove());
        this
    }

    fn open_popover(self) -> bool {
        if let Some(el) = self.node.get_untracked()
            && el.show_popover().is_ok()
        {
            self.open.set(true);
            return true;
        }
        false
    }

    pub(super) fn show(self, first: &'static str) {
        if self.open_popover() {
            self.focus_page(first);
        }
    }

    pub(super) fn show_choice(self, fallback: &'static str) {
        if self.open_popover() {
            self.focus_choices(fallback);
        }
    }

    pub(super) fn focus_choices(self, fallback: &'static str) {
        request_animation_frame(move || self.position());
        focus_choice(self.id, fallback);
    }

    pub(super) fn focus_page(self, first: &'static str) {
        let before = focused();
        request_animation_frame(move || {
            self.position();
            if self.open.try_get_untracked() == Some(true) && focus_unmoved(before.as_ref()) {
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
    let before = focused();
    request_animation_frame(move || {
        if !focus_unmoved(before.as_ref()) {
            return;
        }
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
        if let Some(choice) = find("[role=option][aria-selected=true]:not(:disabled)")
            .or_else(|| find("[data-picker-option]:not(:disabled)"))
        {
            let _ = choice.focus();
        } else {
            platform::focus(fallback);
        }
    });
}

pub(super) fn focused() -> Option<web_sys::Element> {
    document().active_element()
}

/// Focus put off to a later frame (a page's first choice, or back to an opener) goes ahead only if
/// nothing moved the keyboard meanwhile: it never pulls back focus from a control reached in the
/// meantime. A removed element leaves focus on the body, which still counts as unmoved.
pub(super) fn focus_unmoved(before: Option<&web_sys::Element>) -> bool {
    focused().is_none_or(|now| before == Some(&now) || now.tag_name() == "BODY")
}

pub(super) fn position_detail_pickers() {
    for (id, anchor) in [
        ("repeat-picker", "edit-task-repeat"),
        ("tag-picker", "edit-task-tags"),
        (super::reminder::PICKER, super::reminder::ANCHOR),
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
