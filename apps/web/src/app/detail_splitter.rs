//! Width preference and input handling for the docked task detail splitter.

use super::{ctx, html};
use crate::platform;
use leptos::prelude::*;
use todo_design::layout::{desktop, elevation};

const MIN: i32 = desktop::DETAIL_MIN as i32;
pub(super) const DEFAULT: i32 = desktop::DETAIL as i32;
const CHROME: i32 = (desktop::SIDEBAR + 2.0 * elevation::HAIRLINE) as i32;

#[derive(Clone, Copy)]
pub(super) struct Layout {
    preferred: RwSignal<i32>,
    preview: RwSignal<Option<i32>>,
    viewport: RwSignal<i32>,
    max: Memo<i32>,
    pub(super) width: Memo<i32>,
    pub(super) compact_room: Memo<bool>,
}

impl Layout {
    pub(super) fn new() -> Self {
        let viewport = RwSignal::new(platform::viewport_width());
        let preferred = RwSignal::new(platform::load_detail_width().unwrap_or(DEFAULT).max(MIN));
        let preview = RwSignal::new(None);
        let max = Memo::new(move |_| {
            (viewport.get() - CHROME - desktop::LIST_MIN_BESIDE_DETAIL as i32).max(MIN)
        });
        // Only clamp the visible width. A temporary reduction must not replace a wider preference.
        let width = Memo::new(move |_| {
            preview
                .get()
                .unwrap_or_else(|| preferred.get())
                .min(max.get())
        });
        let compact_room =
            Memo::new(move |_| viewport.get() - CHROME - width.get() < desktop::LIST_MIN as i32);
        Self {
            preferred,
            preview,
            viewport,
            max,
            width,
            compact_room,
        }
    }

    fn save(self) {
        platform::save_detail_width(self.preferred.get_untracked());
    }
}

#[derive(Clone, Copy)]
struct Grab {
    pointer: i32,
    x: i32,
    width: i32,
}

impl Grab {
    fn moved(self, x: i32, max: i32) -> i32 {
        (self.width + self.x - x).clamp(MIN, max)
    }
}

#[component]
pub(super) fn DetailSplitter(layout: Layout, docked: Memo<bool>) -> impl IntoView {
    let c = ctx();
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
        let width = platform::viewport_width();
        if width != layout.viewport.get_untracked() {
            // The grab's starting coordinates no longer match the divider. Discard its preview.
            cancel_grab();
            layout.viewport.set(width);
        }
    });
    on_cleanup(move || resize.remove());
    let reset = move || {
        cancel_grab();
        layout.preferred.set(DEFAULT);
    };
    Effect::new(move |_| {
        if !docked.get() {
            cancel_grab();
            save_keys();
        }
    });

    view! {
        <div class="detail-splitter" class:grabbing=move || grab.with(Option::is_some)
            node_ref=handle hidden=move || !docked.get()
            role="separator" tabindex="0" aria-label="Task detail width"
            aria-orientation="vertical" aria-controls="task-detail"
            aria-valuemin=MIN.to_string()
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
                    }));
                }
            }
            on:pointermove=move |ev| {
                if let Some(g) = grab.get_untracked().filter(|g| g.pointer == ev.pointer_id()) {
                    let width = g.moved(ev.client_x(), layout.max.get_untracked());
                    if layout.width.get_untracked() != width {
                        layout.preview.set(Some(width));
                    }
                }
            }
            on:pointerup=move |ev| {
                if let Some(g) = grab.get_untracked().filter(|g| g.pointer == ev.pointer_id()) {
                    let max = layout.max.get_untracked();
                    let width = g.moved(ev.client_x(), max);
                    // A click or a drag back to where it started must not overwrite a saved width.
                    if platform::viewport_width() == layout.viewport.get_untracked()
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
                            "ArrowRight" => (current - step).max(MIN),
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
