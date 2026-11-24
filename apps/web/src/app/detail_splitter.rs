//! Width preference and input handling for the docked task detail splitter.

use super::{ctx, html};
use crate::platform;
use leptos::prelude::*;
use todo_design::layout::{desktop, elevation};

pub(super) const CHROME: i32 = (desktop::SIDEBAR + 2.0 * elevation::HAIRLINE) as i32;
const ASSISTANT_WIDTH_KEY: &str = "todo.assistant.width";

#[derive(Clone, Copy)]
pub(super) enum Pane {
    Detail,
    Assistant,
}

impl Pane {
    fn min(self) -> i32 {
        (match self {
            Self::Detail => desktop::DETAIL_MIN,
            Self::Assistant => desktop::AI_PANEL_MIN,
        }) as i32
    }

    pub(super) fn default_width(self) -> i32 {
        (match self {
            Self::Detail => desktop::DETAIL,
            Self::Assistant => desktop::AI_PANEL,
        }) as i32
    }

    fn cap(self) -> i32 {
        match self {
            Self::Detail => i32::MAX,
            Self::Assistant => desktop::AI_PANEL_MAX as i32,
        }
    }

    fn load(self) -> Option<i32> {
        match self {
            Self::Detail => platform::load_detail_width(),
            Self::Assistant => platform::local_storage()?
                .get_item(ASSISTANT_WIDTH_KEY)
                .ok()??
                .parse()
                .ok(),
        }
    }

    fn save(self, width: i32) {
        match self {
            Self::Detail => platform::save_detail_width(width),
            Self::Assistant => {
                if let Some(storage) = platform::local_storage() {
                    let _ = storage.set_item(ASSISTANT_WIDTH_KEY, &width.to_string());
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pane: Pane,
    pub(super) preferred: RwSignal<i32>,
    preview: RwSignal<Option<i32>>,
    max: Memo<i32>,
    pub(super) width: Memo<i32>,
}

impl Layout {
    pub(super) fn preferred(pane: Pane) -> RwSignal<i32> {
        RwSignal::new(
            pane.load()
                .unwrap_or_else(|| pane.default_width())
                .clamp(pane.min(), pane.cap()),
        )
    }

    pub(super) fn new(
        pane: Pane,
        preferred: RwSignal<i32>,
        viewport: RwSignal<i32>,
        reserved: impl Fn() -> i32 + Send + Sync + 'static,
    ) -> Self {
        let preview = RwSignal::new(None);
        let max = Memo::new(move |_| {
            (viewport.get() - CHROME - reserved() - desktop::LIST_MIN_BESIDE_DETAIL as i32)
                .clamp(pane.min(), pane.cap())
        });
        // Only clamp the visible width. A temporary reduction must not replace a wider preference.
        let width = Memo::new(move |_| {
            preview
                .get()
                .unwrap_or_else(|| preferred.get())
                .min(max.get())
        });
        Self {
            pane,
            preferred,
            preview,
            max,
            width,
        }
    }

    fn save(self) {
        self.pane.save(self.preferred.get_untracked());
    }
}

#[derive(Clone, Copy)]
struct Grab {
    pointer: i32,
    x: i32,
    width: i32,
    viewport: i32,
}

impl Grab {
    fn moved(self, x: i32, min: i32, max: i32) -> i32 {
        (self.width + self.x - x).clamp(min, max)
    }
}

#[component]
pub(super) fn Splitter(layout: Layout, docked: Memo<bool>) -> impl IntoView {
    let c = ctx();
    let min = layout.pane.min();
    let (class, label, controls) = match layout.pane {
        Pane::Detail => ("detail-splitter", "Task detail width", "task-detail"),
        Pane::Assistant => (
            "detail-splitter ai-splitter",
            "Resize AI assistant",
            "assistant-panel",
        ),
    };
    let handle = NodeRef::<html::Div>::new();
    let grab = RwSignal::new(None::<Grab>);
    let keyboard_dirty = StoredValue::new(false);
    let save_keys = move || {
        if keyboard_dirty.get_value() {
            keyboard_dirty.set_value(false);
            layout.save();
        }
    };
    let cancel_grab = move || {
        if let Some(g) = grab.get_untracked() {
            grab.set(None);
            layout.preview.set(None);
            if let Some(el) = handle.get_untracked()
                && el.has_pointer_capture(g.pointer)
            {
                let _ = el.release_pointer_capture(g.pointer);
            }
        }
    };
    let resize = window_event_listener(leptos::ev::resize, move |_| {
        // A viewport resize invalidates the drag's starting geometry, not the saved width.
        cancel_grab();
    });
    on_cleanup(move || resize.remove());
    let reset = move || {
        cancel_grab();
        layout.preferred.set(layout.pane.default_width());
    };
    Effect::new(move |_| {
        if !docked.get() {
            cancel_grab();
            save_keys();
        }
    });

    view! {
        <div class=class class:grabbing=move || grab.with(Option::is_some)
            node_ref=handle hidden=move || !docked.get()
            role="separator" tabindex="0" aria-label=label
            aria-orientation="vertical" aria-controls=controls
            aria-valuemin=min.to_string()
            aria-valuemax=move || layout.max.get().to_string()
            aria-valuenow=move || layout.width.get().to_string()
            on:pointerdown=move |ev| {
                if ev.button() == 0 && ev.is_primary() && grab.get_untracked().is_none()
                    && let Some(el) = handle.get_untracked()
                    && el.set_pointer_capture(ev.pointer_id()).is_ok()
                {
                    ev.prevent_default();
                    save_keys();
                    let _ = el.focus();
                    grab.set(Some(Grab {
                        pointer: ev.pointer_id(),
                        x: ev.client_x(),
                        width: layout.width.get_untracked(),
                        viewport: platform::viewport_width(),
                    }));
                }
            }
            on:pointermove=move |ev| {
                if let Some(g) = grab.get_untracked().filter(|g| g.pointer == ev.pointer_id()) {
                    let width = g.moved(ev.client_x(), min, layout.max.get_untracked());
                    if layout.width.get_untracked() != width {
                        layout.preview.set(Some(width));
                    }
                }
            }
            on:pointerup=move |ev| {
                if let Some(g) = grab.get_untracked().filter(|g| g.pointer == ev.pointer_id()) {
                    let max = layout.max.get_untracked();
                    let width = g.moved(ev.client_x(), min, max);
                    // A click or a drag back to where it started must not overwrite a saved width.
                    if platform::viewport_width() == g.viewport
                        && ev.client_x() != g.x && width != g.width.min(max)
                    {
                        layout.preferred.set(width);
                        layout.save();
                    }
                    cancel_grab();
                }
            }
            on:pointercancel=move |ev| {
                if grab.get_untracked().is_some_and(|g| g.pointer == ev.pointer_id()) {
                    cancel_grab();
                }
            }
            on:lostpointercapture=move |ev| {
                if grab.get_untracked().is_some_and(|g| g.pointer == ev.pointer_id()) {
                    cancel_grab();
                }
            }
            on:dblclick=move |_| {
                reset();
                layout.save();
            }
            on:keydown=move |ev| {
                if ev.is_composing() || ev.alt_key() || ev.ctrl_key() || ev.meta_key() {
                    return;
                }
                let key = ev.key();
                match key.as_str() {
                    "Escape" => {
                        cancel_grab();
                        if !c.selected.get_untracked().is_some_and(|id| platform::focus(&format!("row-{id}"))) {
                            platform::focus("room-title");
                        }
                    }
                    "Home" | "Enter" => {
                        reset();
                        keyboard_dirty.set_value(true);
                    }
                    "ArrowLeft" | "ArrowRight" | "End" => {
                        cancel_grab();
                        let max = layout.max.get_untracked();
                        let current = layout.width.get_untracked();
                        let step = if ev.shift_key() { desktop::PANE_KEY_STEP_LARGE } else { desktop::PANE_KEY_STEP } as i32;
                        let width = match key.as_str() {
                            "ArrowLeft" => (current + step).min(max),
                            "ArrowRight" => (current - step).max(min),
                            _ => max,
                        };
                        if width != current {
                            layout.preferred.set(width);
                            keyboard_dirty.set_value(true);
                        }
                    }
                    _ => return,
                }
                ev.prevent_default();
                ev.stop_propagation();
            }
            on:keyup=move |ev| {
                if matches!(ev.key().as_str(), "ArrowLeft" | "ArrowRight" | "Home" | "End" | "Enter") {
                    save_keys();
                }
            }
            on:blur=move |_| save_keys()
        ></div>
    }
}
