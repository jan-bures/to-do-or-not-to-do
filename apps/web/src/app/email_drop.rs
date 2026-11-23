//! The task list's .eml drop target. Attachment drops on task detail use their own target.

use leptos::{html, prelude::*};
use todo_client::intent::{
    TaskRoom,
    email::{self, DroppedEmail, Feedback, ImportError, MAX_FILE_BYTES, MAX_FILES},
};
use wasm_bindgen_futures::JsFuture;
use web_sys::{DataTransfer, DragEvent, File};

use super::{Ctx, Icon, RoomId, backend, icon, platform};

#[derive(Clone, Copy)]
pub(super) struct Target {
    room: NodeRef<html::Div>,
    pub list: NodeRef<html::Div>,
    feedback: NodeRef<html::Div>,
    depth: RwSignal<u32>,
    over: RwSignal<bool>,
    reading: RwSignal<bool>,
}

impl Target {
    pub fn new(room: NodeRef<html::Div>) -> Self {
        Self {
            room,
            list: NodeRef::new(),
            feedback: NodeRef::new(),
            depth: RwSignal::new(0),
            over: RwSignal::new(false),
            reading: RwSignal::new(false),
        }
    }

    fn enabled(self, c: Ctx) -> bool {
        !self.reading.get()
            && !c.read_only()
            && !c.drawer.get()
            && !c.detail_modal.get()
            && !c.room.with(RoomId::is_history)
    }

    pub fn highlight(self, c: Ctx) -> bool {
        self.over.get() && self.enabled(c)
    }

    /// The room stays a single scroll surface, including on phones. Centre feedback on the
    /// visible list viewport, not the full height of its rows or the header above it.
    pub fn position(self) {
        if !self.over.get_untracked() {
            return;
        }
        let (Some(room), Some(list), Some(feedback)) = (
            self.room.get_untracked(),
            self.list.get_untracked(),
            self.feedback.get_untracked(),
        ) else {
            return;
        };
        let top = list.get_bounding_client_rect().top() - room.get_bounding_client_rect().top();
        let top = top.clamp(0., f64::from(room.client_height()));
        let scrollbar = room.offset_width() - room.client_width();
        let style = web_sys::HtmlElement::style(&feedback);
        let _ = style.set_property("--email-drop-top", &format!("{top}px"));
        let _ = style.set_property("--email-drop-scrollbar", &format!("{scrollbar}px"));
    }

    pub fn clear(self) {
        self.depth.set(0);
        self.over.set(false);
    }

    pub fn over(self, c: Ctx, event: DragEvent, entering: bool) {
        let Some(data) = file_data(&event) else {
            return;
        };
        // Even a file we can't import must not replace the app in this tab.
        event.prevent_default();
        if entering {
            self.depth.update(|depth| *depth = depth.saturating_add(1));
        }
        let over = untrack(|| self.enabled(c)) && might_be_email(&data);
        if over != self.over.get_untracked() {
            self.over.set(over);
        }
        data.set_drop_effect(if over { "copy" } else { "none" });
        self.position();
    }

    pub fn leave(self) {
        // Entering a child is followed by a leave on the old target; neither leaves the room.
        let depth = self.depth.get_untracked().saturating_sub(1);
        self.depth.set(depth);
        if depth == 0 {
            self.over.set(false);
        }
    }

    pub fn receive(self, c: Ctx, event: DragEvent) {
        self.clear();
        let Some(data) = file_data(&event) else {
            return;
        };
        event.prevent_default();
        if !untrack(|| self.enabled(c)) {
            return;
        }
        let mut files = email_files(&data);
        let overflow = files.len() > MAX_FILES;
        files.truncate(MAX_FILES);
        if files.is_empty() {
            return;
        }
        let room = c.room.get_untracked();
        let now = if c.live {
            platform::now()
        } else {
            c.now.get_untracked()
        };
        self.reading.set(true);
        leptos::task::spawn_local(async move {
            let mut messages = Vec::with_capacity(files.len());
            for file in files {
                messages.push(read(&file, now.at).await);
            }
            self.reading.set(false);
            let status = backend::status();
            if status.signed_out || status.read_only {
                c.refresh();
                return;
            }
            let mut feedback = Feedback::default();
            if overflow {
                feedback.failed(ImportError::TooMany);
            }
            let mut targets = Vec::new();
            let room = TaskRoom::from(&room);
            for message in messages {
                let message = match message {
                    Ok(message) => message,
                    Err(error) => {
                        feedback.failed(error);
                        continue;
                    }
                };
                let state = backend::view();
                if !email::destination_available(&state, room) {
                    feedback.failed(ImportError::Destination);
                    break;
                }
                let commands = message.commands(&state, room, now.today(), backend::new_uuid);
                email::submit_message(commands, &mut feedback, &mut targets, |cmd| {
                    backend::submit_tracked(cmd).map_err(|_| ())
                });
            }
            c.refresh();
            if let Some(message) = feedback.message() {
                if targets.is_empty() {
                    c.show_toast(message, false);
                } else {
                    c.remember_undo(targets, message, None);
                }
            }
        });
    }
}

#[component]
pub(super) fn Affordance(target: Target, c: Ctx) -> impl IntoView {
    let destination = Memo::new(move |_| {
        c.room.with(|room| {
            c.state
                .with(|state| email::drop_destination(state, room.into()))
        })
    });
    view! {
        <div class="email-drop-feedback" class:active=move || target.highlight(c)
             node_ref=target.feedback aria-hidden="true">
            <div class="email-drop-card">
                <span class="email-drop-icon">{icon(Icon::Mail)}</span>
                <div class="email-drop-text">
                    <div class="email-drop-label">{email::DROP_LABEL}</div>
                    <div class="email-drop-destination">{move || destination.get()}</div>
                </div>
            </div>
        </div>
        <span class="email-drop-announcement" role="status" aria-atomic="true">
            {move || target.highlight(c).then(|| format!("{} {}", email::DROP_LABEL, destination.get()))}
        </span>
    }
}

/// Header and composer are not import targets, but a file must not navigate the tab away.
pub(super) fn ignore_files(event: DragEvent) {
    if !event.default_prevented()
        && let Some(data) = file_data(&event)
    {
        event.prevent_default();
        data.set_drop_effect("none");
    }
}

fn file_data(event: &DragEvent) -> Option<DataTransfer> {
    event
        .data_transfer()
        .filter(|data| data.types().includes(&"Files".into(), 0))
}

fn is_eml(file: &File) -> bool {
    file.name()
        .rsplit_once('.')
        .is_some_and(|(name, ext)| !name.is_empty() && email::is_eml_extension(ext))
}

fn might_be_email(data: &DataTransfer) -> bool {
    let items = data.items();
    // In protected drag-over mode, browsers expose MIME types but not filenames. An undeclared
    // type can be an .eml, so show the target and check real filenames only on drop.
    items.length() == 0
        || (0..items.length())
            .filter_map(|i| items.get(i))
            .any(|item| {
                item.kind() == "file"
                    && item.get_as_file().ok().flatten().map_or_else(
                        || {
                            matches!(
                                item.type_().as_str(),
                                "message/rfc822"
                                    | "application/eml"
                                    | "application/x-eml"
                                    | "application/octet-stream"
                                    | ""
                            )
                        },
                        |file| is_eml(&file),
                    )
            })
}

fn email_files(data: &DataTransfer) -> Vec<File> {
    let items = data.items();
    if items.length() == 0 {
        return data.files().map_or_else(Vec::new, |files| {
            (0..files.length())
                .filter_map(|i| files.get(i))
                .filter(is_eml)
                .take(MAX_FILES + 1)
                .collect()
        });
    }
    (0..items.length())
        .filter_map(|i| {
            let item = items.get(i).filter(|item| item.kind() == "file")?;
            if item
                .webkit_get_as_entry()
                .ok()
                .flatten()
                .is_some_and(|entry| entry.is_directory())
            {
                return None;
            }
            item.get_as_file().ok().flatten().filter(is_eml)
        })
        .take(MAX_FILES + 1)
        .collect()
}

async fn read(file: &File, at: todo_core::time::Timestamp) -> Result<DroppedEmail, ImportError> {
    if file.size() > MAX_FILE_BYTES as f64 {
        return Err(ImportError::TooLarge);
    }
    let buffer = JsFuture::from(file.array_buffer())
        .await
        .map_err(|_| ImportError::Read)?;
    email::parse(&js_sys::Uint8Array::new(&buffer).to_vec(), at)
}
