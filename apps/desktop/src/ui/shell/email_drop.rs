//! A drop on the list creates tasks. The task detail's existing file-attachment target is separate.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use gpui::{AsyncApp, Context, Div, ExternalPaths, div, prelude::*, px, transparent_black};
use todo_client::intent::email::{
    self, DroppedEmail, Feedback, ImportError, MAX_FILE_BYTES, MAX_FILES,
};
use todo_core::Command;
use todo_core::time::{Date, Timestamp};

use super::{RoomKey, ShellView, elevation, undo};

fn is_eml(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(email::is_eml_extension)
}

fn read(path: &Path, at: Timestamp) -> Result<DroppedEmail, ImportError> {
    let meta = path.metadata().map_err(|_| ImportError::Read)?;
    if !meta.is_file() {
        return Err(ImportError::Read);
    }
    if meta.len() > MAX_FILE_BYTES as u64 {
        return Err(ImportError::TooLarge);
    }
    let file = File::open(path).map_err(|_| ImportError::Read)?;
    let mut raw = Vec::new();
    // Metadata is only a fast rejection. A growing file never makes us read past the bound.
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|_| ImportError::Read)?;
    email::parse(&raw, at)
}

impl ShellView {
    pub fn preview_email_drop(&mut self, active: bool) {
        self.preview_email_drop = active;
    }

    pub(super) fn email_drop_target(&self, cx: &mut Context<Self>) -> Option<Div> {
        let room = self.shell.room.key;
        if room.is_history() || self.email_importing || self.pairing_panel.is_some() {
            return None;
        }
        let accent = self.palette.accent_strong;
        Some(
            div()
                .absolute()
                .inset_0()
                .border(px(elevation::FOCUS_RING))
                .border_color(if self.preview_email_drop {
                    accent
                } else {
                    transparent_black()
                })
                .can_drop(|value, _, _| {
                    value
                        .downcast_ref::<ExternalPaths>()
                        .is_some_and(|paths| paths.0.iter().any(|p| is_eml(p)))
                })
                .drag_over::<ExternalPaths>(move |style, _, _, _| style.border_color(accent))
                .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                    this.import_emails(room, &paths.0, cx);
                })),
        )
    }

    fn import_emails(&mut self, room: RoomKey, paths: &[PathBuf], cx: &mut Context<Self>) {
        if self.email_importing || self.store.sync().unauthorized {
            return;
        }
        let mut candidates = paths.iter().filter(|p| is_eml(p));
        let paths: Vec<_> = candidates.by_ref().take(MAX_FILES).cloned().collect();
        let overflow = candidates.next().is_some();
        if paths.is_empty() {
            return;
        }
        let now = self.store.now();
        let today = now.today();
        self.email_importing = true;
        cx.notify();
        let reading = cx.background_spawn(async move {
            paths
                .iter()
                .map(|path| read(path, now.at))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let messages = reading.await;
            let _ = this.update(cx, |this, cx| {
                this.email_importing = false;
                if !this.store.sync().unauthorized {
                    this.finish_email_drop(room, today, messages, overflow, cx);
                }
            });
        })
        .detach();
    }

    fn finish_email_drop(
        &mut self,
        room: RoomKey,
        today: Date,
        messages: Vec<Result<DroppedEmail, ImportError>>,
        overflow: bool,
        cx: &mut Context<Self>,
    ) {
        let mut feedback = Feedback::default();
        if overflow {
            feedback.failed(ImportError::TooMany);
        }
        let mut targets = Vec::new();
        let mut last_task = None;
        for message in messages {
            let message = match message {
                Ok(message) => message,
                Err(error) => {
                    feedback.failed(error);
                    continue;
                }
            };
            let state = self.store.view();
            if !email::destination_available(&state, room.into()) {
                feedback.failed(ImportError::Destination);
                break;
            }
            let commands = message.commands(&state, room, today, || self.store.new_uuid());
            email::submit_message(commands, &mut feedback, &mut targets, |cmd| {
                let task = match &cmd {
                    Command::CreateTask(task) => Some(task.id),
                    _ => None,
                };
                let target = self.store.submit_undoable(cmd).map_err(|_| ())?;
                last_task = task.or(last_task);
                Ok(target)
            });
        }
        if let Some(label) = feedback.message() {
            if let Some(task) = last_task {
                self.remember_undo(
                    undo::Action {
                        entity: task.into(),
                        kind: undo::EditKind::ImportEmail,
                        targets,
                        room: self.shell.room.key,
                    },
                    label.into(),
                );
            } else {
                self.toast_without_undo = true;
                self.toast.show(label.into(), self.now(), self.pace());
            }
        }
        self.refresh(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, store::Store};

    #[test]
    fn only_real_eml_extensions_are_candidates() {
        assert!(is_eml(Path::new("message.EML")));
        for name in ["eml", "message.eml.txt", "message.msg", "message.emlx"] {
            assert!(!is_eml(Path::new(name)));
        }
    }

    #[test]
    fn imported_task_and_message_link_undo_in_reverse_submission_order() {
        let fixture = fixtures::my_day();
        let store = Store::fixture(&fixture);
        let mut feedback = Feedback::default();
        let mut undo = Vec::new();
        let raw = include_bytes!("../../../../../crates/todo-client/tests/fixtures/eml/plain.eml");
        let email = email::parse(raw, fixture.now.at).expect("message");
        let commands = email.commands(&store.view(), fixture.ui.room, fixture.now.today(), || {
            store.new_uuid()
        });
        email::submit_message(commands, &mut feedback, &mut undo, |cmd| {
            store.submit_undoable(cmd).map_err(|_| ())
        });
        assert_eq!(feedback.created, 1);
        assert_eq!(undo.len(), 2);
        assert_eq!(store.view().tasks.len(), fixture.state.tasks.len() + 1);
        assert_eq!(store.view().links.len(), fixture.state.links.len() + 1);
        for target in undo.iter().rev() {
            store.undo(target).expect("undo");
        }
        assert_eq!(store.view().tasks, fixture.state.tasks);
        assert_eq!(store.view().links, fixture.state.links);
    }
}
