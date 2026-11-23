//! A drop on the list creates tasks. The task detail's existing file-attachment target is separate.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use gpui::{AsyncApp, Context, Div, DragMoveEvent, ExternalPaths, Window, div, prelude::*, px};
use todo_client::intent::email::{
    self, DroppedEmail, Feedback, ImportError, MAX_FILE_BYTES, MAX_FILES,
};
use todo_core::Command;
use todo_core::time::{Date, Timestamp};

use super::{Icon, RoomKey, ShellView, Width, component, elevation, kit, radius, space, ty, undo};
use crate::theme::{self, TextStyled, motion};

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

    fn can_import_emails(&self) -> bool {
        !self.shell.room.key.is_history()
            && !self.email_importing
            && self.pairing_panel.is_none()
            && self.menu.is_none()
            && !(self.width == Width::Narrow && self.motion.detail.look(self.frame).task > 0.)
    }

    fn aim_email_drop(&mut self, over: bool) {
        let target = f32::from(over);
        if target != self.email_drop_fade.target() {
            let duration = motion::HOVER.min(motion::REDUCED_FADE);
            self.email_drop_fade.aim(
                target,
                self.now(),
                std::time::Duration::from_millis(duration.into()),
                motion::STANDARD,
            );
        }
    }

    pub(super) fn drive_email_drop(&mut self, window: &mut Window, cx: &Context<Self>) {
        // GPUI refreshes the window when a file is dropped or dragged out of the window.
        if !cx.has_active_drag() || !self.can_import_emails() {
            self.aim_email_drop(false);
        }
        if self.email_drop_fade.moving(self.frame) {
            window.request_animation_frame();
        }
    }

    pub(super) fn email_drop_target(&self, cx: &mut Context<Self>) -> Option<Div> {
        let enabled = self.can_import_emails();
        let fade = if self.preview_email_drop {
            1.
        } else {
            self.email_drop_fade.at(self.frame)
        };
        let room = self.shell.room.key;
        (enabled || fade > 0.).then(|| {
            div()
                .absolute()
                .inset_0()
                .when(enabled, |d| {
                    d.on_drag_move(cx.listener(
                        |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                            let over = event.bounds.contains(&event.event.position)
                                && event.drag(cx).0.iter().any(|p| is_eml(p));
                            if f32::from(over) != this.email_drop_fade.target() {
                                this.aim_email_drop(over);
                                cx.notify();
                            }
                        },
                    ))
                    .can_drop(|value, _, _| {
                        value
                            .downcast_ref::<ExternalPaths>()
                            .is_some_and(|paths| paths.0.iter().any(|p| is_eml(p)))
                    })
                    .on_drop(cx.listener(
                        move |this, paths: &ExternalPaths, _, cx| {
                            this.aim_email_drop(false);
                            this.import_emails(room, &paths.0, cx);
                        },
                    ))
                })
                .when(fade > 0., |d| d.child(self.email_drop_feedback(room, fade)))
        })
    }

    fn email_drop_feedback(&self, room: RoomKey, fade: f32) -> Div {
        let p = self.palette;
        let destination = email::drop_destination(&self.store.view(), room.into());
        div()
            .absolute()
            .inset_0()
            .opacity(fade)
            .bg(theme::with_alpha(p.wash, elevation::DROP_WASH_ALPHA))
            .border(px(elevation::FOCUS_RING))
            .border_color(p.accent_strong)
            .flex()
            .items_center()
            .justify_center()
            .p(px(space::S4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(space::S4))
                    .py(px(space::S3))
                    .max_w_full()
                    .rounded(px(radius::LG))
                    .bg(p.pane)
                    .border(px(elevation::HAIRLINE))
                    .border_color(p.line_strong)
                    .child(
                        div()
                            .size(px(space::S8))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(p.wash)
                            .rounded(px(radius::MD))
                            .child(kit::icon(Icon::Mail, component::ICON, p.accent_strong)),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .child(
                                div()
                                    .style(ty::TASK_TITLE)
                                    .text_color(p.ink)
                                    .child(email::DROP_LABEL),
                            )
                            .child(
                                div()
                                    .style(ty::BODY)
                                    .text_color(p.ink_3)
                                    .truncate()
                                    .child(destination),
                            ),
                    ),
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
