//! Native picker, Explorer drops, and task-scoped feedback for direct attachment uploads.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use accesskit::{Live, Node};
use gpui::{
    App, AsyncApp, Bounds, Context, Div, Element, ElementId, FocusHandle, GlobalElementId,
    InspectorElementId, LayoutId, PathPromptOptions, Pixels, Role, SharedString, Window, div,
    prelude::*, px,
};
use todo_client::{Connection, failure, intent};
use todo_core::ids::TaskId;

use super::{ShellView, kit};
use crate::model::attachments::{Control, Upload, Uploads, file_controls};
use crate::model::{Detail, files};
use crate::theme::{Icon, TextStyled, component, desktop, elevation, radius, space, ty};

pub(super) struct AttachmentUi {
    pub uploads: Uploads,
    pub add: FocusHandle,
    pub rows: BTreeMap<Control, FocusHandle>,
    bounds: Rc<RefCell<BTreeMap<Control, Bounds<Pixels>>>>,
}

impl AttachmentUi {
    pub fn new(cx: &mut Context<ShellView>) -> Self {
        Self {
            uploads: Uploads::default(),
            add: cx.focus_handle(),
            rows: BTreeMap::new(),
            bounds: Rc::default(),
        }
    }

    fn probe(&self, control: Control) -> impl IntoElement {
        let bounds = self.bounds.clone();
        gpui::canvas(
            move |b, _, _| {
                bounds.borrow_mut().insert(control, b);
            },
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0()
    }
}

impl ShellView {
    fn upload_block(&self, task: TaskId) -> Option<&'static str> {
        match self.attachment_ui.uploads.get(task) {
            Some(Upload::Picking) => return Some("Choosing files…"),
            Some(Upload::Transferring) => return Some("Uploading…"),
            _ => {}
        }
        let session = self.store.session()?;
        let sync = self.store.sync();
        if sync.unauthorized {
            return Some("This device is signed out. Pair again to upload attachments.");
        }
        if sync.connection == Connection::Offline {
            return Some(failure::ATTACHMENT_OFFLINE);
        }
        (!session.client().with_confirmed(|s| s.is_live(task.into())))
            .then_some(failure::ATTACHMENT_TASK_UNSYNCED)
    }

    /// The same gate for the picker and GPUI's ExternalPaths drop target. Remove is a tracked edit,
    /// so it doesn't depend on connectivity or the state of a transfer.
    pub(super) fn can_add_files(&self, task: TaskId) -> bool {
        self.repeat_target() == Some(task)
            && self.store.session().is_some()
            && self.upload_block(task).is_none()
    }

    fn pick_files(&mut self, task: TaskId, cx: &mut Context<Self>) {
        if !self.can_add_files(task) || !self.attachment_ui.uploads.start(task, true) {
            return;
        }
        let choice = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add file".into()),
        });
        cx.notify();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let choice = choice.await;
            let _ = this.update(cx, |v, cx| {
                let uploads = &mut v.attachment_ui.uploads;
                match choice {
                    Ok(Ok(Some(paths))) => {
                        uploads.finish(task, v.ui.task, None, Vec::new());
                        // A picker result belongs to the original task in this shell/pairing.
                        v.upload_files(task, paths, cx);
                    }
                    Ok(Ok(None)) => uploads.finish(task, v.ui.task, None, Vec::new()),
                    Ok(Err(_)) | Err(_) => uploads.finish(
                        task,
                        v.ui.task,
                        Some("Couldn’t open the file picker.".into()),
                        Vec::new(),
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn upload_files(
        &mut self,
        task: TaskId,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() || self.attachment_ui.uploads.busy(task) {
            return;
        }
        let Some(session) = self.store.session() else {
            return;
        };
        if let Some(reason) = self.upload_block(task) {
            self.attachment_ui
                .uploads
                .finish(task, self.ui.task, Some(reason.into()), Vec::new());
            cx.notify();
            return;
        }
        self.attachment_ui.uploads.start(task, false);
        let job = session.upload_attachments(task, paths);
        cx.notify();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let (error, unobserved) = match job.await {
                Ok(outcome) => (
                    outcome.error.map(std::borrow::Cow::into_owned),
                    outcome.unobserved,
                ),
                Err(_) => (
                    Some("The upload was stopped. Check the task before trying again.".into()),
                    Vec::new(),
                ),
            };
            let _ = this.update(cx, |v, cx| {
                v.attachment_ui
                    .uploads
                    .finish(task, v.ui.task, error, unobserved);
                // Show any attachments already delivered by sync when releasing the Add button.
                v.refresh(cx);
            });
        })
        .detach();
    }

    fn activate_file_control(&mut self, task: TaskId, control: Control, cx: &mut Context<Self>) {
        if self.repeat_target() != Some(task) {
            return;
        }
        let id = match control {
            Control::Add => return self.pick_files(task, cx),
            Control::Save(id) | Control::Open(id) | Control::Remove(id) => id,
        };
        let state = self.store.view();
        let Some(file) = state
            .attachments
            .get(&id)
            .filter(|f| f.task == task && state.is_live(id.into()))
        else {
            return;
        };
        match control {
            Control::Add => {}
            Control::Save(_) => self.save_attachment(id, file.file_name.clone().into(), cx),
            Control::Open(_) => self.open_attachment(id, file.file_name.clone().into(), cx),
            Control::Remove(_) => {
                let _ = self.submit_if(intent::delete(id), |s| {
                    s.is_live(id.into()) && s.attachments.get(&id).is_some_and(|f| f.task == task)
                });
                self.refresh(cx);
            }
        }
    }

    pub(super) fn sync_attachment_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.repeat_target();
        let mut removed_focus = target.is_none() && self.attachment_ui.add.is_focused(window);
        let rows = self
            .shell
            .detail
            .as_ref()
            .filter(|d| Some(d.task) == target)
            .map_or(&[][..], |d| d.attachments.as_slice());
        let controls: std::collections::BTreeSet<_> = file_controls(rows).collect();
        self.attachment_ui.rows.retain(|control, handle| {
            let keep = controls.contains(control);
            removed_focus |= !keep && handle.is_focused(window);
            keep
        });
        for control in controls {
            self.attachment_ui
                .rows
                .entry(control)
                .or_insert_with(|| cx.focus_handle());
        }
        self.attachment_ui.bounds.borrow_mut().retain(|control, _| {
            target.is_some()
                && (*control == Control::Add || self.attachment_ui.rows.contains_key(control))
        });
        if removed_focus {
            if target.is_some() {
                window.focus(&self.attachment_ui.add, cx);
                self.reveal_attachment(Control::Add);
            } else {
                window.focus(&self.list_focus, cx);
            }
        }
    }

    pub(super) fn reveal_attachment(&self, control: Control) {
        if let Some(&bounds) = self.attachment_ui.bounds.borrow().get(&control) {
            self.reveal_bounds(&self.detail_scroll, bounds);
        }
    }

    fn attachment_button(
        &self,
        button: FileButton,
        task: TaskId,
        live: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let FileButton {
            control,
            name,
            blocked,
            visual,
        } = button;
        let p = self.palette;
        let focus = match control {
            Control::Add => Some(&self.attachment_ui.add),
            _ => self.attachment_ui.rows.get(&control),
        }
        .filter(|_| live);
        let focused = focus.is_some_and(|f| f.is_focused(window));
        let button = div()
            .id(SharedString::from(format!("attachment-{control:?}")))
            .relative()
            .rounded(px(radius::BUTTON))
            .border(px(elevation::HAIRLINE))
            .border_color(if focused {
                p.accent
            } else {
                gpui::transparent_black()
            })
            .child(visual)
            .when_some(focus, |el, focus| {
                el.track_focus(focus)
                    .role(Role::Button)
                    .aria_label(name.clone())
                    .child(self.attachment_ui.probe(control))
                    .on_key_down(cx.listener(|v, e, window, cx| v.control_key(e, window, cx)))
                    .when_some(blocked, |el, reason| el.aria_description(reason))
                    .when(blocked.is_none(), |el| {
                        el.cursor_pointer()
                            .hover(|s| s.bg(p.hover))
                            .tooltip(kit::tooltip(p, name))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                cx.stop_propagation();
                                v.activate_file_control(task, control, cx);
                            }))
                    })
            });
        div()
            .min_w_0()
            .p(px(elevation::FOCUS_RING))
            .m(px(-elevation::FOCUS_RING))
            .rounded(px(radius::BUTTON + elevation::FOCUS_RING))
            .when(focused, |el| el.bg(p.tint))
            .child(button)
    }

    pub(super) fn attachment_section(
        &self,
        d: &Detail,
        live: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let task = d.task;
        let p = self.palette;
        let blocked = self.upload_block(task);
        let (status, error) = self
            .attachment_ui
            .uploads
            .get(task)
            .and_then(Upload::notice)
            .unwrap_or_else(|| {
                (
                    blocked.unwrap_or("or drop files on this task").into(),
                    false,
                )
            });
        let add_ink = if blocked.is_some() {
            p.ink_3
        } else {
            p.accent_strong
        };
        let add = FileButton {
            control: Control::Add,
            name: "Add file".into(),
            blocked,
            visual: div()
                .h(px(component::BUTTON_SMALL))
                .px(px(space::S2))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .style(ty::BUTTON_SMALL)
                .text_color(add_ink)
                .child(kit::icon(Icon::Plus, component::ICON_SMALL, add_ink))
                .child("Add file"),
        };
        let status: SharedString = status.into_owned().into();
        let mut section = div()
            .mx(px(desktop::DETAIL_PAD_X))
            .mt(px(space::S4))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(kit::label(&p, "Attachments"))
                    .child(
                        self.attachment_button(add, task, live, window, cx)
                            .flex_none(),
                    ),
            )
            .child(UploadStatus {
                element: div()
                    .style(ty::META)
                    .text_color(if error { p.danger_ink } else { p.ink_3 })
                    .child(status.clone()),
                status,
                live,
                error,
            });
        for row in &d.attachments {
            let id = row.id;
            let file_button = |control, verb, icon| FileButton {
                control,
                name: format!("{verb} “{}”", row.label).into(),
                blocked: None,
                visual: div()
                    .size(px(component::ICON_BUTTON))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(kit::icon(icon, component::ICON_SMALL, p.ink_3)),
            };
            let mut save = file_button(Control::Save(id), "Save", Icon::Download);
            save.visual = div()
                .h(px(component::ICON_BUTTON))
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(kit::icon(Icon::Paperclip, component::ICON_SMALL, p.ink_3))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .style(ty::NAV_SUB)
                        .text_color(p.ink)
                        .child(row.label.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .style(ty::TAG)
                        .text_color(p.ink_3)
                        .child(row.detail.clone()),
                )
                .child(kit::icon(Icon::Download, component::ICON_SMALL, p.ink_3));
            let save = self
                .attachment_button(save, task, live, window, cx)
                .flex_1();
            // Reserve the Open slot so the download and remove glyphs align in every row.
            let open = if files::can_open(&row.name) {
                self.attachment_button(
                    file_button(Control::Open(id), "Open", Icon::ArrowUpRight),
                    task,
                    live,
                    window,
                    cx,
                )
            } else {
                div().w(px(component::ICON_BUTTON + 2. * elevation::HAIRLINE))
            };
            let remove = self
                .attachment_button(
                    file_button(Control::Remove(id), "Remove", Icon::TrashTwo),
                    task,
                    live,
                    window,
                    cx,
                )
                .flex_none();
            section = section.child(
                div()
                    .h(px(desktop::PROP_ROW))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .border_b_1()
                    .border_color(p.line)
                    .child(save)
                    .child(open.flex_none())
                    .child(remove),
            );
        }
        section
    }
}

/// GPUI's Div doesn't expose AccessKit's live property. Keep its layout and paint, but supply
/// the live-region node explicitly: the role alone doesn't announce changes on Windows.
struct UploadStatus {
    element: Div,
    status: SharedString,
    live: bool,
    error: bool,
}

impl Element for UploadStatus {
    type RequestLayoutState = <Div as Element>::RequestLayoutState;
    type PrepaintState = <Div as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        Some("attachment-status".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        self.element.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.element
            .prepaint(id, inspector_id, bounds, state, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element
            .paint(id, inspector_id, bounds, state, prepaint, window, cx);
    }

    fn a11y_role(&self) -> Option<Role> {
        self.live.then_some(if self.error {
            Role::Alert
        } else {
            Role::Status
        })
    }

    fn write_a11y_info(&self, node: &mut Node) {
        node.set_label(self.status.as_ref());
        node.set_live(if self.error {
            Live::Assertive
        } else {
            Live::Polite
        });
    }
}

struct FileButton {
    control: Control,
    name: SharedString,
    blocked: Option<&'static str>,
    visual: Div,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_feedback_writes_an_explicit_live_setting_to_the_accesskit_node() {
        for (status, error, role, live) in [
            ("Uploading…", false, Role::Status, Live::Polite),
            (
                "Couldn’t upload the file.",
                true,
                Role::Alert,
                Live::Assertive,
            ),
        ] {
            let mut element = UploadStatus {
                element: div(),
                status: status.into(),
                live: true,
                error,
            };
            // These are the hooks Element uses to generate the real tree node.
            assert!(element.id().is_some());
            let mut node = Node::new(element.a11y_role().expect("accessible feedback"));
            element.write_a11y_info(&mut node);
            assert_eq!(node.role(), role);
            assert_eq!(node.live(), Some(live));
            assert_eq!(node.label(), Some(status));
            element.live = false;
            assert_eq!(element.a11y_role(), None, "closing detail is visual only");
        }
    }
}
