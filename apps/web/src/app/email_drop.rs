//! The room's .eml drop target. Attachment drops on task detail use their own target.

use leptos::prelude::*;
use todo_client::intent::{
    TaskRoom,
    email::{self, DroppedEmail, Feedback, ImportError, MAX_FILE_BYTES, MAX_FILES},
};
use wasm_bindgen_futures::JsFuture;
use web_sys::{DataTransfer, DragEvent, File};

use super::{Ctx, RoomId, backend, platform};

#[derive(Clone, Copy)]
pub(super) struct Target {
    depth: RwSignal<u32>,
    over: RwSignal<bool>,
    reading: RwSignal<bool>,
}

impl Target {
    pub fn new() -> Self {
        Self {
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
