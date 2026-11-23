//! The detail pane: the selected task's title, steps, notes and fields, edited in place.
//!
//! Title and notes are edited directly in the pane and saved when editing ends (Enter or leaving the
//! title, Escape or leaving the notes). Steps can be added, completed, renamed (click the title), moved and
//! deleted. Due, priority, reminders, repeat and tags use pickers anchored to their rows. Web links are
//! added by typing or pasting an address, removed (Undo brings one back) and open in the browser
//! (anything else is shown as text). An attachment is saved where the owner chooses; documents and
//! images can also be opened, from this pairing's attachments directory and marked as downloaded from
//! the internet.

use gpui::{
    AnyElement, AsyncApp, Context, Div, Entity, FontWeight, SharedString, Subscription, WeakEntity,
    Window, div, prelude::*, px,
};
use todo_core::{
    ids::{AttachmentId, TaskId},
    time::Date,
};

use super::sidebar::InlineTarget;
use super::{Notice, ShellView, quick_add_colors};
use super::{menu::MenuKind, quick_pickers::Calendar};
use crate::model::activity::Mark;
use crate::model::draft::Draft;
use crate::model::files;
use crate::model::hints::Stop;
use crate::model::layout::Pane;
use crate::model::menus::TagPage;
use crate::model::{self, Detail, FieldEdit, FieldKind, commands, fit, quick_add};
use crate::theme::{
    self, Icon, Palette, TextStyled, component, desktop, elevation, radius, space, ty,
};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::kit;
use crate::ui::text_area::TextArea;
use todo_core::model::Priority;

/// The width of a property row's label column (Due, Priority, Tags): the row draws it and the Tags row's
/// chips are fitted beside it, so both read it from here.
const FIELD_LABEL: f32 = space::S10 * 2.;
const STEP_CONTROL: f32 = component::ICON_BUTTON - space::S2;

/// `ch` is the advance of the zero glyph in the text's own face and size.
fn prose_width(window: &Window, style: ty::TextStyle) -> gpui::Pixels {
    px(desktop::DETAIL_TEXT_MEASURE * kit::text_width(window, style, "0"))
}

fn log_save_failure(error: &anyhow::Error) {
    tracing::warn!("saving an attachment failed");
    tracing::debug!("saving an attachment failed: {error:#}");
}

#[cfg(test)]
#[test]
fn save_failure_only_logs_the_private_destination_at_debug() {
    use tracing::Level;

    let path = "private-destination/report.pdf";
    let error = anyhow::anyhow!("replacing {path}").context("couldn't save");
    let capture = |level| {
        let buffer = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = buffer.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(level)
            .with_writer(move || {
                // Keep the writer independent of the subscriber's borrow.
                struct Writer(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
                impl std::io::Write for Writer {
                    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                        self.0.lock().expect("log buffer").extend_from_slice(bytes);
                        Ok(bytes.len())
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        Ok(())
                    }
                }
                Writer(output.clone())
            })
            .finish();
        tracing::subscriber::with_default(subscriber, || log_save_failure(&error));
        let bytes = buffer.lock().expect("log buffer").clone();
        String::from_utf8(bytes).expect("UTF-8 log")
    };
    let warn = capture(Level::WARN);
    assert!(warn.contains("saving an attachment failed"));
    assert!(!warn.contains(path));
    let debug = capture(Level::DEBUG);
    assert!(debug.contains("couldn't save: replacing private-destination/report.pdf"));
}

/// The pane's editors and what they hold.
pub(super) struct Editors {
    pub title: Entity<TextArea>,
    pub notes: Entity<TextArea>,
    pub step: Entity<TextInput>,
    /// The field that adds a web link (L).
    pub link: Entity<TextInput>,
    /// The bulk due-date field while it is being edited.
    pub due: Option<(Entity<TextInput>, [Subscription; 2])>,
    /// The task whose title and notes the editors hold.
    pub loaded: Option<TaskId>,
    pub title_draft: Draft,
    pub notes_draft: Draft,
    _subscriptions: Vec<Subscription>,
}

impl Editors {
    pub fn new(p: &Palette, window: &mut Window, cx: &mut Context<ShellView>) -> Self {
        let colors = quick_add_colors(p);
        let title = cx.new(|cx| {
            let mut a = TextArea::new("Title", colors, cx);
            a.single = true;
            a
        });
        let notes = cx.new(|cx| TextArea::new("Add notes", colors, cx));
        let step = cx.new(|cx| TextInput::new("Add step", colors, cx));
        let link = cx.new(|cx| TextInput::new("Add a link", colors, cx));
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(
            &link,
            window,
            |this, input, e: &InputEvent, window, cx| match e {
                InputEvent::Submit => {
                    let text = input.read(cx).content.to_string();
                    if this.add_link(&text, cx) {
                        input.update(cx, |i, cx| i.reset(window, cx));
                    }
                }
                InputEvent::Cancel => {
                    input.update(cx, |i, cx| i.reset(window, cx));
                    window.focus(&this.list_focus, cx);
                }
                InputEvent::Changed => cx.notify(),
            },
        ));
        subs.push(
            cx.subscribe_in(&title, window, |this, _, e: &InputEvent, window, cx| {
                match e {
                    InputEvent::Changed => this.edit.title_draft.edited(),
                    InputEvent::Submit => {
                        this.commit_title(cx);
                        window.focus(&this.list_focus, cx);
                    }
                    InputEvent::Cancel => {
                        // Escape puts the saved title back.
                        this.edit.title_draft.discard();
                        this.edit.loaded = None;
                        this.load_detail(cx);
                        window.focus(&this.list_focus, cx);
                    }
                }
            }),
        );
        subs.push(cx.subscribe_in(
            &notes,
            window,
            |this, _, e: &InputEvent, window, cx| match e {
                InputEvent::Changed => this.edit.notes_draft.edited(),
                InputEvent::Submit | InputEvent::Cancel => {
                    this.commit_notes(cx);
                    window.focus(&this.list_focus, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &step,
            window,
            |this, input, e: &InputEvent, window, cx| match e {
                InputEvent::Submit => {
                    let text = input.read(cx).content.to_string();
                    if this.add_step(&text, cx) {
                        input.update(cx, |i, cx| i.reset(window, cx));
                    }
                }
                InputEvent::Cancel => window.focus(&this.list_focus, cx),
                InputEvent::Changed => cx.notify(),
            },
        ));
        let handle = title.read(cx).focus_handle.clone();
        subs.push(cx.on_blur(&handle, window, |this, _, cx| {
            this.commit_title(cx);
        }));
        let handle = notes.read(cx).focus_handle.clone();
        subs.push(cx.on_blur(&handle, window, |this, _, cx| {
            this.commit_notes(cx);
        }));
        Self {
            title,
            notes,
            step,
            link,
            due: None,
            loaded: None,
            title_draft: Draft::default(),
            notes_draft: Draft::default(),
            _subscriptions: subs,
        }
    }
}

impl ShellView {
    /// Puts the selected task's title and notes into the editors: all of it when another task was
    /// selected, otherwise only what isn't being edited (a change that arrived from elsewhere).
    pub(super) fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.shell.detail.as_ref() else {
            if self.edit.loaded.is_some() || self.bulk_ids().len() < 2 {
                self.edit.due = None;
            }
            self.edit.loaded = None;
            return;
        };
        let fresh = self.edit.loaded != Some(d.task);
        if fresh {
            self.edit.loaded = Some(d.task);
            self.edit.title_draft.discard();
            self.edit.notes_draft.discard();
            self.edit.due = None;
            // A link half typed for another task doesn't follow the selection.
            self.edit
                .link
                .update(cx, |i, cx| i.set_text(String::new(), cx));
        }
        let (title, notes) = (d.title.clone(), d.notes.clone());
        if self.edit.title_draft.accepts_incoming() && self.edit.title.read(cx).content != title {
            self.edit.title.update(cx, |a, cx| a.set_text(title, cx));
        }
        if self.edit.notes_draft.accepts_incoming() && self.edit.notes.read(cx).content != notes {
            self.edit.notes.update(cx, |a, cx| a.set_text(notes, cx));
        }
    }

    /// Saves the title being edited. Returns whether nothing is left unsaved; a failed save keeps the
    /// text in the editor (and the error in the status bar) for the next attempt.
    pub(super) fn commit_title(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.edit.loaded else {
            return true;
        };
        let text = self.edit.title.read(cx).content.to_string();
        let mut draft = self.edit.title_draft;
        let saved = draft.commit(|| {
            commands::retitle(&self.store.view(), id, &text)
                .is_none_or(|cmd| self.submit(vec![cmd]))
        });
        self.edit.title_draft = draft;
        self.refresh(cx);
        saved
    }

    /// Saves the notes being edited, like [`commit_title`](Self::commit_title).
    pub(super) fn commit_notes(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.edit.loaded else {
            return true;
        };
        let text = self.edit.notes.read(cx).content.to_string();
        let mut draft = self.edit.notes_draft;
        let saved = draft.commit(|| {
            commands::set_notes(&self.store.view(), id, &text)
                .is_none_or(|cmd| self.submit(vec![cmd]))
        });
        self.edit.notes_draft = draft;
        self.refresh(cx);
        saved
    }

    /// Saves whatever is being edited (before another task or room is shown). Returns false when a draft
    /// couldn't be saved: the caller stays on the task, so the text isn't lost.
    pub(super) fn commit_edits(&mut self, cx: &mut Context<Self>) -> bool {
        let title = self.commit_title(cx);
        let notes = self.commit_notes(cx);
        self.commit_inline(cx);
        title && notes
    }

    fn add_step(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(task) = self.edit.loaded else {
            return false;
        };
        let store = self.store.clone();
        let ok = commands::add_step(task, text, || store.new_uuid())
            .is_some_and(|cmd| self.submit(vec![cmd]));
        self.refresh(cx);
        ok
    }

    /// Adds the web link typed into the link field to the open task. Returns whether it was added; what
    /// can't be added says why in the status bar, and the text stays to be corrected.
    fn add_link(&mut self, text: &str, cx: &mut Context<Self>) -> bool {
        let Some(task) = self.edit.loaded else {
            return false;
        };
        let store = self.store.clone();
        let ok = match commands::add_link(&store.view(), task, text, || store.new_uuid()) {
            Ok(cmd) => self.submit(vec![cmd]),
            Err(commands::LinkError::Empty) => false,
            Err(commands::LinkError::NotWeb) => {
                self.notice = Some(Notice::error(format!(
                    "\u{201c}{}\u{201d} isn't a web address. Try https://example.com or example.com.",
                    model::labels::one_line(text.trim())
                )));
                false
            }
            Err(commands::LinkError::TaskGone) => {
                self.notice = Some(Notice::error("That task is gone."));
                false
            }
        };
        self.refresh(cx);
        ok
    }

    /// L, or a task's Links… menu: the keyboard goes to the detail pane's link field, for `task`
    /// (selected first, if it isn't), scrolled into view.
    pub(super) fn start_link(&mut self, task: TaskId, window: &mut Window, cx: &mut Context<Self>) {
        if self.ui.task != Some(task) {
            self.select_task(Some(task), cx);
        }
        if self.ui.task != Some(task) || self.shell.detail.is_none() {
            return;
        }
        let handle = self.edit.link.read(cx).focus_handle.clone();
        window.focus(&handle, cx);
        // Once the pane is laid out for this task: the field scrolled into view, wherever it is.
        let view = cx.entity().downgrade();
        window.on_next_frame(move |_, cx| {
            view.update(cx, |this, cx| {
                if this.scroll_into_view(&this.detail_scroll, "link-field") {
                    cx.notify();
                }
            })
            .ok();
        });
        cx.notify();
    }

    fn step_command(&mut self, cmd: Option<todo_core::Command>, cx: &mut Context<Self>) {
        if let Some(cmd) = cmd {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    pub(super) fn detail_calendar(&self, task: TaskId) -> MenuKind {
        let due = self.store.view().tasks.get(&task).and_then(|t| t.due);
        MenuKind::Date(Calendar::new(Some(task), self.store.now().today(), due))
    }

    /// D opens the calendar and reveals its anchor if the detail pane is scrolled.
    pub(super) fn start_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(task) = self.repeat_target() {
            self.scroll_into_view(&self.detail_scroll, "due-row");
            self.open_menu(
                self.detail_calendar(task),
                None,
                Some(Stop::Due),
                window,
                cx,
            );
        }
    }

    pub(super) fn start_bulk_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let colors = quick_add_colors(&self.palette);
        let input = cx.new(|cx| TextInput::new("tomorrow, fri, +3d or 2026-12-01", colors, cx));
        let events =
            cx.subscribe_in(
                &input,
                window,
                |this, input, e: &InputEvent, window, cx| match e {
                    InputEvent::Submit => {
                        let text = input.read(cx).content.to_string();
                        if let Some(date) = this.parse_due_text(&text, cx) {
                            this.apply_bulk(
                                todo_client::selection::BulkTaskAction::Due { date },
                                window,
                                cx,
                            );
                        }
                    }
                    InputEvent::Cancel => {
                        this.edit.due = None;
                        window.focus(&this.list_focus, cx);
                        cx.notify();
                    }
                    InputEvent::Changed => {}
                },
            );
        let handle = input.read(cx).focus_handle.clone();
        let blur = cx.on_blur(&handle, window, |this, _, cx| {
            this.edit.due = None;
            cx.notify();
        });
        window.focus(&handle, cx);
        self.edit.due = Some((input, [events, blur]));
        cx.notify();
    }

    fn parse_due_text(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Option<Option<todo_core::time::Date>> {
        let now = self.store.now();
        let due = if text.trim().is_empty() {
            None
        } else if let Some(d) = quick_add::due_text(text, &now.eval()) {
            Some(d)
        } else {
            self.notice = Some(Notice::error(format!(
                "“{}” isn't a date this understands. Try tomorrow, fri, +3d or 2026-12-01.",
                text.trim()
            )));
            cx.notify();
            return None;
        };
        Some(due)
    }

    pub(super) fn choose_task_due(
        &mut self,
        task: TaskId,
        due: Option<Date>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.repeat_target() != Some(task) {
            self.close_menu(false, window, cx);
            return;
        }
        let command = commands::set_due(&self.store.view(), task, due);
        if command.is_none_or(|cmd| self.submit(vec![cmd])) {
            self.close_menu(true, window, cx);
        }
        self.refresh(cx);
    }

    /// Saves an attachment where the owner chooses (a save dialog, starting in Downloads). The default
    /// action for every attachment: nothing is run.
    pub(super) fn save_attachment(
        &mut self,
        id: AttachmentId,
        name: SharedString,
        cx: &mut Context<Self>,
    ) {
        if self.store.session().is_none() {
            self.notice = Some(Notice::info(
                "Attachments download when connected to a server.",
            ));
            cx.notify();
            return;
        }
        // The dialog offers the stored name, made safe for Windows; notices show it on one line.
        let chosen = cx.prompt_for_new_path(&downloads_dir(), Some(&files::safe_file_name(&name)));
        let label = model::labels::one_line(&name);
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let path = match chosen.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(e)) => {
                    let _ = this.update(cx, |v, cx| {
                        v.notice = Some(Notice::error(format!("Couldn't save {label}: {e}")));
                        cx.notify();
                    });
                    return;
                }
            };
            let Ok(Some(job)) = this.update(cx, |v, cx| {
                v.notice = Some(Notice::info(format!("Downloading {label}…")));
                cx.notify();
                v.store.session().map(|s| s.download_to(id, path))
            }) else {
                return;
            };
            let result = job.await;
            let _ = this.update(cx, |v, cx| {
                v.notice = Some(match result {
                    Ok(saved) if saved.marked => {
                        Notice::info(format!("Saved {}", saved.path.display()))
                    }
                    // Saved, but the drive can't hold the mark (FAT, exFAT): say so.
                    Ok(saved) => Notice::error(format!(
                        "Saved {}, but this drive can't mark it as downloaded from the internet, \
                         so Windows won't warn when it's opened.",
                        saved.path.display()
                    )),
                    Err(e) => {
                        log_save_failure(&e);
                        Notice::error(format!("Couldn't save {label}: {e}"))
                    }
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Downloads a document or image (see `files::can_open`) into this pairing's attachments, marked as
    /// from the internet, and opens it with its app. Anything else can only be saved.
    pub(super) fn open_attachment(
        &mut self,
        id: AttachmentId,
        name: SharedString,
        cx: &mut Context<Self>,
    ) {
        let label = model::labels::one_line(&name);
        if !files::can_open(&name) {
            self.notice = Some(Notice::info(format!(
                "{label} can only be saved, not opened here."
            )));
            cx.notify();
            return;
        }
        let Some(session) = self.store.session() else {
            self.notice = Some(Notice::info("Attachments open when connected to a server."));
            cx.notify();
            return;
        };
        let job = session.download_to_open(id, &name);
        self.notice = Some(Notice::info(format!("Downloading {label}…")));
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = job.await;
            let _ = this.update(cx, |v, cx| {
                match result {
                    // Checked once more on the name it was written under.
                    Ok(path)
                        if path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(files::can_open) =>
                    {
                        v.notice = None;
                        cx.open_with_system(&path);
                    }
                    Ok(_) => {
                        v.notice = Some(Notice::error(format!("{label} can only be saved.")));
                    }
                    Err(e) => {
                        tracing::warn!("opening an attachment failed: {e:#}");
                        v.notice = Some(Notice::error(format!("Couldn't open {label}: {e}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn step_row(
        &self,
        i: usize,
        s: &model::StepRow,
        measure: gpui::Pixels,
        wrap: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let p = self.palette;
        let id = s.id;
        let mark = if s.done {
            kit::checked(&p, false, component::GLYPH)
        } else {
            kit::checkbox(&p, false, component::GLYPH)
        };
        let group: SharedString = format!("step-{id}").into();
        let title = match self.inline_for(InlineTarget::Step(id)) {
            Some(input) => div()
                .flex_1()
                .min_w_0()
                .max_w(measure)
                .style(ty::STEP)
                .child(input)
                .into_any_element(),
            None => {
                let t = div()
                    .id(("step-title", i))
                    .flex_1()
                    .min_w_0()
                    .max_w(measure)
                    .when(!wrap, |d| d.truncate())
                    .style(ty::STEP)
                    .child(s.title.clone());
                let t = if s.done {
                    t.text_color(p.ink_3).line_through()
                } else {
                    t.text_color(p.ink)
                };
                let name = s.title.clone();
                t.on_click(cx.listener(move |this, _, window, cx| {
                    this.start_inline(InlineTarget::Step(id), name.clone(), window, cx);
                }))
                .into_any_element()
            }
        };
        let control = |key: &'static str, icon: Icon| {
            div()
                .id((key, i))
                .size(px(STEP_CONTROL))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::SM))
                .hover(|s| s.bg(p.hover))
                .child(kit::icon(icon, component::ICON_SMALL, p.ink_3))
        };
        div()
            .group(group.clone())
            .relative()
            .min_h(px(desktop::STEP_ROW))
            .when(wrap, |d| d.py(px(space::S1)))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .px(px(desktop::DETAIL_PAD_X))
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .id(("step-check", i))
                    .child(mark)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let cmd = commands::toggle_step(&this.store.view(), id);
                        this.step_command(cmd, cx);
                    })),
            )
            .child(title)
            .child(
                div()
                    .ml_auto()
                    .flex_none()
                    .style(ty::COUNT)
                    .text_color(p.ink_4)
                    .child(format!("{:02}", i + 1)),
            )
            .child(
                // Move and delete show over the step number on hover.
                div()
                    .absolute()
                    .right(px(desktop::DETAIL_PAD_X - space::S1))
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .bg(p.pane)
                    .opacity(0.)
                    .group_hover(group, |s| s.opacity(1.))
                    .child(control("step-up", Icon::ChevronUp).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let cmd = commands::nudge(&this.store.view(), id.into(), -1);
                            this.step_command(cmd, cx);
                        },
                    )))
                    .child(
                        control("step-down", Icon::ChevronDown).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let cmd = commands::nudge(&this.store.view(), id.into(), 1);
                                this.step_command(cmd, cx);
                            },
                        )),
                    )
                    .child(control("step-delete", Icon::TrashTwo).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.step_command(Some(commands::delete(id)), cx);
                        },
                    ))),
            )
    }

    fn detail(&self, d: &Detail, live: bool, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let t = self.theme;
        let task = d.task;
        let steps_done = d.steps.iter().filter(|s| s.done).count() as u32;
        let steps_total = d.steps.len() as u32;
        let section = |label: &'static str| {
            div()
                .px(px(desktop::DETAIL_PAD_X))
                .pt(px(space::S5))
                .pb(px(space::S2))
                .child(kit::label(&p, label))
        };
        // The breadcrumb takes what the actions leave and ends in an ellipsis; the actions never move.
        let crumbs = div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(space::S2));
        let crumbs = match &d.list.collection {
            // The collection's chip keeps its width (up to its cap), so the list's name gives way first.
            Some(c) => crumbs
                .child(
                    kit::chip(
                        t,
                        &model::ListRef {
                            collection: None,
                            name: c.clone(),
                            hue: d.list.hue,
                        },
                        false,
                    )
                    .flex_none(),
                )
                .child(div().flex_none().text_color(p.ink_4).child("/"))
                .child(
                    div()
                        .min_w_0()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(p.ink)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(d.list.name.clone()),
                ),
            None => crumbs.child(kit::chip(t, &d.list, false)),
        };
        let top_button = |id: &'static str, label: &'static str, glyph: AnyElement| {
            div()
                .id(id)
                .size(px(component::ICON_BUTTON))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::MD))
                .hover(|s| s.bg(p.hover))
                .tooltip(kit::tooltip(p, label))
                .child(glyph)
        };
        let top_toggle = |id, label, action, icon, on, handle: &gpui::FocusHandle| {
            top_button(
                id,
                action,
                self.task_glyph(&p, task, icon, on, component::GLYPH)
                    .into_any_element(),
            )
            .when(live, |el| {
                el.track_focus(handle)
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .aria_toggled(if on {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .border(px(elevation::HAIRLINE))
                    .border_color(gpui::transparent_black())
                    .focus_visible(move |s| s.border_color(p.accent_strong))
                    .on_key_down(cx.listener(|this, e, window, cx| this.control_key(e, window, cx)))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_task_glyph(task, icon, cx)),
                    )
            })
        };
        let my_day = top_toggle(
            "detail-my-day",
            "My Day",
            if d.my_day {
                "Remove from My Day"
            } else {
                "Add to My Day"
            },
            Icon::Sun,
            d.my_day,
            &self.detail_my_day_focus,
        );
        let important = top_toggle(
            "detail-important",
            "Important",
            if d.important {
                "Remove importance"
            } else {
                "Mark as important"
            },
            Icon::Star,
            d.important,
            &self.detail_important_focus,
        );
        let links: Vec<_> = d
            .links
            .iter()
            .map(|l| {
                let url = l.url.clone();
                let link = l.id;
                let group: SharedString = format!("link-{}", l.id).into();
                // Remove shows on hover, over the card's right end; Undo brings the link back.
                let remove = div()
                    .id(SharedString::from(format!("link-remove-{}", l.id)))
                    .flex_none()
                    .size(px(component::ICON_BUTTON - space::S2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .opacity(0.)
                    .group_hover(group.clone(), |s| s.opacity(1.))
                    .hover(|s| s.bg(p.hover))
                    .child(kit::icon(Icon::X, component::ICON_SMALL, p.ink_3))
                    .when(live, |el| {
                        el.tooltip(kit::tooltip(p, "Remove link"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let cmd = commands::delete_link(&this.store.view(), link);
                                this.step_command(cmd, cx);
                            }))
                    });
                div()
                    .id(group.clone())
                    .group(group)
                    .mx(px(desktop::DETAIL_PAD_X))
                    .mb(px(space::S2))
                    .p(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .rounded(px(radius::LG))
                    .border(px(elevation::HAIRLINE))
                    .border_color(p.line_strong)
                    .bg(p.raised)
                    .child(
                        div()
                            .size(px(component::ICON_BUTTON))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(radius::MD))
                            .bg(p.wash)
                            .child(kit::icon(
                                if l.email { Icon::Mail } else { Icon::Link },
                                component::ICON_SMALL,
                                p.accent_strong,
                            )),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .style(ty::NAV_SUB.with_weight(600))
                                    .text_color(p.ink)
                                    .child(l.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .style(ty::TAG)
                                    .text_color(p.ink_3)
                                    .child(l.detail.clone()),
                            ),
                    )
                    .when(url.is_some(), |el| {
                        el.child(kit::icon(
                            Icon::ArrowUpRight,
                            component::ICON_SMALL,
                            p.ink_3,
                        ))
                        .hover(|s| s.bg(p.hover))
                        .on_click(move |_, _, cx| {
                            if let Some(u) = &url {
                                cx.open_url(u);
                            }
                        })
                    })
                    .when(live, |el| el.child(remove))
            })
            .collect();
        let link_focused = live && self.edit.link.read(cx).focus_handle.is_focused(window);
        // Below the links: the field that adds one (L), like Add step.
        let add_link = div()
            .relative()
            .mx(px(desktop::DETAIL_PAD_X))
            .h(px(desktop::STEP_ROW))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .style(ty::STEP)
            .text_color(p.ink_3)
            .child(kit::icon(
                Icon::Link,
                component::ICON_SMALL,
                if link_focused {
                    p.accent_strong
                } else {
                    p.ink_3
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(live, |el| el.child(self.edit.link.clone()))
                    .when(!live, |el| el.child("Add a link")),
            )
            .child(kit::kbd(&p, if link_focused { "Enter" } else { "L" }))
            .when(live, |d| d.child(self.probe("link-field")));
        let step_measure = prose_width(window, ty::STEP);
        // Keep the progress and step actions near the text on wide panes, leaving room for them
        // beyond the longest title line. This group uses the same left inset as Notes and Fields.
        let steps_width = step_measure
            + px(2. * desktop::DETAIL_PAD_X
                + component::GLYPH
                + 2. * space::S3
                + 3. * STEP_CONTROL);
        let wrap_steps = px(self.columns(window).width(Pane::Detail)) >= steps_width;
        let steps: Vec<AnyElement> = d
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                self.step_row(i, s, step_measure, wrap_steps, cx)
                    .into_any_element()
            })
            .collect();
        let fields: Vec<_> = d
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let due = f.edit == Some(FieldEdit::Due);
                let priority = f.edit == Some(FieldEdit::Priority);
                let tags = f.edit == Some(FieldEdit::Tags);
                // A tag being renamed (from the tag picker, whether or not this task has it): its
                // editor takes the row's value, where the picker hangs from.
                let renaming = (self.inline.as_ref())
                    .filter(|i| tags && live && i.target.tag().is_some())
                    .map(|i| i.input.clone());
                let value = match f.edit {
                    _ if renaming.is_some() => div()
                        .flex_1()
                        .min_w_0()
                        .style(ty::TAG)
                        .text_color(p.ink)
                        .children(renaming.clone())
                        .into_any_element(),
                    _ if matches!(f.edit, Some(FieldEdit::Repeat | FieldEdit::Reminder))
                        && f.kind == FieldKind::Empty =>
                    {
                        div()
                            .style(ty::NAV_COLLECTION.with_weight(400))
                            .text_color(p.ink_3)
                            .child(f.value.clone())
                            .into_any_element()
                    }
                    _ if tags && !d.tags.is_empty() => {
                        self.tag_chips(&d.tags, window).into_any_element()
                    }
                    _ if priority && d.priority != Priority::None => div()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(kit::priority(&p, priority_level(d.priority)))
                        .child(field_value(&p, f.kind, f.value.clone()))
                        .into_any_element(),
                    _ => field_value(&p, f.kind, f.value.clone()).into_any_element(),
                };
                let row = div()
                    .id(("field", i))
                    .relative()
                    .h(px(desktop::PROP_ROW))
                    .flex()
                    .items_center()
                    .gap(px(desktop::PROP_GAP))
                    .border_b_1()
                    .border_color(p.line)
                    .child(kit::icon(f.icon, component::ICON_SMALL, p.ink_3))
                    .child(
                        div()
                            .w(px(FIELD_LABEL))
                            .flex_none()
                            .style(ty::NAV_COLLECTION.with_weight(400))
                            .text_color(p.ink_3)
                            .child(f.label),
                    )
                    .child(value)
                    .when(due && live, |el| {
                        let belongs = |kind: &MenuKind| matches!(kind, MenuKind::Date(c) if c.task == Some(task));
                        let open = self.menu.as_ref().is_some_and(|m| belongs(&m.kind) && m.overlay.look(self.frame).open);
                        el.track_focus(&self.due_focus)
                            .role(gpui::Role::Button)
                            .aria_label(format!("Edit due date: {}", f.value))
                            .aria_expanded(open)
                            .hover(|s| s.bg(p.hover))
                            .cursor_pointer()
                            .child(div().flex_1())
                            .child(kit::kbd(&p, "D"))
                            .child(self.probe("due-row"))
                            .on_key_down(cx.listener(|this, e, window, cx| this.control_key(e, window, cx)))
                            .on_click(cx.listener(move |this, e, window, cx| {
                                this.toggle_menu(this.detail_calendar(task), Stop::Due, e, window, cx);
                            }))
                            .children(self.menu_anchor(belongs, cx))
                    })
                    // The four levels to pick from (Shift+P from the list); P cycles them.
                    .when(priority && live, |el| {
                        el.hover(|s| s.bg(p.hover))
                            .child(div().flex_1())
                            .child(kit::kbd(&p, "Shift P"))
                            .on_click(cx.listener(move |this, e, window, cx| {
                                this.toggle_menu(
                                    MenuKind::Priority { task },
                                    Stop::List,
                                    e,
                                    window,
                                    cx,
                                );
                            }))
                            .children(
                                self.menu_anchor(|k| matches!(k, MenuKind::Priority { .. }), cx),
                            )
                    })
                    .when(f.edit == Some(FieldEdit::Repeat) && live, |el| {
                        el.track_focus(&self.repeat_focus)
                            .role(gpui::Role::Button)
                            .tooltip(kit::tooltip(
                                p,
                                if f.kind == FieldKind::Empty {
                                    "Choose how often this task repeats".into()
                                } else {
                                    f.value.clone()
                                },
                            ))
                            .hover(|s| s.bg(p.hover))
                            .cursor_pointer()
                            .child(div().flex_1())
                            .child(kit::kbd(&p, "R"))
                            .child(self.probe("repeat-row"))
                            .on_key_down(
                                cx.listener(|this, e, window, cx| this.control_key(e, window, cx)),
                            )
                            .on_click(cx.listener(move |this, e, window, cx| {
                                this.toggle_menu(
                                    MenuKind::Repeat { task, draft: None },
                                    Stop::Repeat,
                                    e,
                                    window,
                                    cx,
                                )
                            }))
                            .children(
                                self.menu_anchor(|k| matches!(k, MenuKind::Repeat { .. }), cx),
                            )
                    })
                    // The reminder picker (Shift+R from the list): presets, a custom day and time,
                    // Remove.
                    .when(f.edit == Some(FieldEdit::Reminder) && live, |el| {
                        el.track_focus(&self.reminder_focus)
                            .role(gpui::Role::Button)
                            .tooltip(kit::tooltip(
                                p,
                                SharedString::from(if f.kind == FieldKind::Empty {
                                    "Remind me about this task".to_string()
                                } else {
                                    format!("Reminds you {}", f.value)
                                }),
                            ))
                            .hover(|s| s.bg(p.hover))
                            .cursor_pointer()
                            .child(div().flex_1())
                            .child(kit::kbd(&p, "Shift R"))
                            .child(self.probe("reminder-row"))
                            .on_key_down(
                                cx.listener(|this, e, window, cx| this.control_key(e, window, cx)),
                            )
                            .on_click(cx.listener(move |this, e, window, cx| {
                                let kind = this.reminder_menu(task, None);
                                this.toggle_menu(kind, Stop::Reminder, e, window, cx)
                            }))
                            .children(
                                self.menu_anchor(|k| matches!(k, MenuKind::Reminder { .. }), cx),
                            )
                    })
                    // The tag picker (T from the list): put tags on, take them off, make new ones.
                    .when(tags && live && renaming.is_none(), |el| {
                        el.hover(|s| s.bg(p.hover))
                            .when(d.tags.is_empty(), |el| el.child(div().flex_1()))
                            .child(kit::kbd(&p, "T"))
                            .on_click(cx.listener(move |this, e, window, cx| {
                                let kind = MenuKind::Tags {
                                    task,
                                    page: TagPage::Pick,
                                };
                                this.toggle_menu(kind, Stop::List, e, window, cx);
                                if this.menu.as_ref().is_some_and(|m| m.kind.same_menu(kind)) {
                                    this.highlight_first();
                                }
                            }))
                    })
                    // The picker hangs from the row, and fades out there even once a rename opens.
                    .when(tags && live, |el| {
                        el.children(self.menu_anchor(|k| matches!(k, MenuKind::Tags { .. }), cx))
                    })
                    // Shown, not edited here yet: say so rather than look like the rows that are.
                    .when(f.edit.is_none(), |el| {
                        el.tooltip(kit::tooltip(p, "Shown here; not editable in this app yet"))
                    });
                let focus = match f.edit {
                    Some(FieldEdit::Due) => Some(&self.due_focus),
                    Some(FieldEdit::Repeat) => Some(&self.repeat_focus),
                    Some(FieldEdit::Reminder) => Some(&self.reminder_focus),
                    _ => None,
                };
                if let Some(focus) = focus {
                    super::menu::ring(
                        &p,
                        live && focus.is_focused(window),
                        radius::BUTTON,
                        row.bg(p.pane),
                    )
                    .into_any_element()
                } else {
                    row.into_any_element()
                }
            })
            .collect();
        let has_mail = d.links.iter().any(|l| l.email);
        let step_focused = live && self.edit.step.read(cx).focus_handle.is_focused(window);
        let body = self.motion.detail.look(self.frame).body;
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .h(px(desktop::DETAIL_TOP))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .pl(px(desktop::DETAIL_PAD_X))
                    .pr(px(space::S3))
                    .border_b_1()
                    .border_color(p.line)
                    .when(live, |d| d.child(self.probe("detail-toolbar")))
                    .child(crumbs.style(ty::SUMMARY).text_color(p.ink_2))
                    .child(
                        // My Day, Important, Close: the same order on every client.
                        div()
                            .flex_none()
                            .flex()
                            .gap(px(space::S1))
                            .child(my_day)
                            .child(important)
                            .child(
                                top_button(
                                    "close-detail",
                                    "Close (Esc)",
                                    kit::icon(Icon::X, component::GLYPH, p.ink_3)
                                        .into_any_element(),
                                )
                                .role(gpui::Role::Button)
                                .aria_label("Close")
                                .on_click(cx.listener(|this, _, _, cx| this.select_task(None, cx))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .opacity(body)
                    .child(
                        div()
                            .px(px(desktop::DETAIL_PAD_X))
                            .pt(px(space::S5))
                            .child(
                                div()
                                    .flex()
                                    .items_start()
                                    .gap(px(space::S3))
                                    .child(
                                        div()
                                            .id("detail-check")
                                            .pt(px(space::S1))
                                            .child(if d.done {
                                                kit::checked(&p, false, component::CHECKBOX + 2.)
                                            } else {
                                                kit::checkbox(&p, false, component::CHECKBOX + 2.)
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.toggle_complete(task, cx);
                                            })),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .max_w(prose_width(window, ty::DETAIL_TITLE))
                                            .style(ty::DETAIL_TITLE)
                                            .text_color(p.ink)
                                            .when(live, |el| el.child(self.edit.title.clone()))
                                            .when(!live, |el| el.child(d.title.clone())),
                                    ),
                            )
                            .child(
                                div()
                                    .ml(px(component::CHECKBOX + 2. + space::S3))
                                    .mt(px(space::S1))
                                    .style(ty::COUNT)
                                    .text_color(p.ink_3)
                                    .child(d.subtitle.clone()),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .max_w(steps_width)
                            .child(
                                div()
                                    .px(px(desktop::DETAIL_PAD_X))
                                    .pt(px(space::S5))
                                    .pb(px(space::S1))
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(kit::label(&p, "Steps"))
                                    .when(steps_total > 0, |d| {
                                        d.child(
                                            div()
                                                .flex()
                                                .items_center()
                                                .gap(px(space::S2))
                                                .child(kit::meter(&p, steps_done, steps_total))
                                                .child(
                                                    div().style(ty::TAG).text_color(p.ink_2).child(
                                                        format!("{steps_done}/{steps_total}"),
                                                    ),
                                                ),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .h(px(elevation::HAIRLINE))
                                    .mx(px(desktop::DETAIL_PAD_X))
                                    .bg(p.line),
                            )
                            .children(steps)
                            .child(
                                div()
                                    .h(px(desktop::STEP_ROW))
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S3))
                                    .px(px(desktop::DETAIL_PAD_X))
                                    .style(ty::STEP)
                                    .text_color(p.ink_3)
                                    .child(kit::icon(
                                        Icon::Plus,
                                        component::GLYPH,
                                        if step_focused {
                                            p.accent_strong
                                        } else {
                                            p.ink_3
                                        },
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .max_w(step_measure)
                                            .when(live, |el| el.child(self.edit.step.clone()))
                                            .when(!live, |el| el.child("Add a step")),
                                    )
                                    .child(
                                        div().ml_auto().flex_none().child(kit::kbd(&p, "Enter")),
                                    ),
                            ),
                    )
                    .child(section("Notes"))
                    .child(
                        div().px(px(desktop::DETAIL_PAD_X)).child(
                            div()
                                .w_full()
                                .max_w(prose_width(window, ty::NOTES))
                                .style(ty::NOTES)
                                .text_color(p.ink_2)
                                .when(live, |el| el.child(self.edit.notes.clone()))
                                .when(!live, |el| el.child(d.notes.clone())),
                        ),
                    )
                    .child(
                        div()
                            .mt(px(space::S4))
                            .mx(px(desktop::DETAIL_PAD_X))
                            .border_t_1()
                            .border_color(p.line)
                            .children(fields),
                    )
                    .child(section(if has_mail { "Source" } else { "Links" }))
                    .children(links)
                    .child(add_link)
                    .child(self.attachment_section(d, live, window, cx))
                    .child(section("Activity"))
                    .child(
                        div()
                            .mx(px(desktop::DETAIL_PAD_X))
                            .border_t_1()
                            .border_color(p.line)
                            .children(d.activity.iter().map(|line| {
                                div()
                                    .min_h(px(desktop::PROP_ROW))
                                    .py(px(space::S1))
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S3))
                                    .child(self.change_mark(
                                        line.actor.as_ref().map_or(Mark::Other, Mark::of),
                                    ))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .style(ty::STEP)
                                            .text_color(p.ink_2)
                                            .child(format!("{} {}", line.who, line.action)),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .style(ty::TAG)
                                            .text_color(p.ink_3)
                                            .child(line.when.clone()),
                                    )
                            })),
                    )
                    .child(div().h(px(space::S6))),
            )
    }

    /// The column stays the same width while content fades in place. A leaving task is
    /// a snapshot: no input entities or active scrollbar, and a shield for its old controls.
    pub(super) fn detail_pane(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let p = &self.palette;
        let look = self.motion.detail.look(self.frame);
        let narrow = self.width == super::Width::Narrow;
        // Fade the column surface, not the whole subtree: the task and placeholder already
        // have their own exit curves. The narrow overlay fades its own container instead.
        let surface = if narrow || self.detail_placement() == super::DetailPane::Docked {
            1.
        } else {
            look.task.max(look.placeholder)
        };
        let placeholder = (look.placeholder > 0.).then(|| {
            let count = self.bulk_ids().len();
            let (icon, text, ink) = if count > 0 {
                (
                    Icon::List,
                    todo_client::selection::count_label(count).into(),
                    p.accent_strong,
                )
            } else {
                (
                    Icon::Text,
                    SharedString::from("Select a task to see its details"),
                    p.ink_3,
                )
            };
            div()
                .absolute()
                .inset_0()
                .opacity(look.placeholder)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(space::S2 + 2.))
                        .child(kit::icon(
                            icon,
                            component::ICON + space::S1,
                            if count > 0 { ink } else { p.ink_4 },
                        ))
                        .child(div().style(ty::BODY).text_color(ink).child(text)),
                )
        });
        let task = self
            .shell
            .detail_layer(self.closing.as_ref())
            .filter(|_| look.task > 0.)
            .map(|(d, live)| {
                let scroll = div()
                    .id("detail-scroll")
                    .size_full()
                    .flex()
                    .flex_col()
                    .when(live, |el| {
                        el.overflow_y_scroll().track_scroll(&self.detail_scroll)
                    })
                    .when(live && self.can_add_files(d.task), |el| {
                        let task = d.task;
                        let wash = p.wash;
                        el.drag_over::<gpui::ExternalPaths>(move |s, _, _, _| s.bg(wash))
                            .on_drop(cx.listener(
                                move |this, paths: &gpui::ExternalPaths, _, cx| {
                                    if this.can_add_files(task) {
                                        this.upload_files(task, paths.0.to_vec(), cx);
                                    }
                                },
                            ))
                    })
                    .child(
                        self.detail(d, live, window, cx)
                            // Keep the last row clear of the reserved toast strip when scrolled down.
                            .when(narrow, |el| el.pb(px(desktop::TOAST_INSET)))
                            .when(!live, |el| el.relative().top(self.detail_scroll.offset().y)),
                    );
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .w_full()
                    .left(px(if narrow { 0. } else { look.task_x }))
                    .opacity(if narrow { 1. } else { look.task })
                    .child(scroll)
                    .when(live, |el| {
                        el.child(
                            self.detail_bar
                                .overlay(&self.detail_scroll, p, self.animate),
                        )
                    })
                    .when(!live, |el| {
                        el.child(
                            div()
                                .id("detail-closing-shield")
                                .absolute()
                                .inset_0()
                                .occlude(),
                        )
                    })
            });
        div()
            .id("detail")
            .relative()
            .w(px(self.columns(window).width(super::Pane::Detail)))
            .flex_none()
            .h_full()
            .overflow_hidden()
            .bg(theme::with_alpha(p.pane, surface))
            .border_l_1()
            .border_color(theme::with_alpha(p.line, surface))
            .children(placeholder)
            .children(task)
    }
}

impl ShellView {
    /// The Tags row's value: each tag as a dot in its colour and its name, side by side. Tags that don't
    /// fit whole are counted as `+N` (`fit::fit_tags`, as on a row), never cut.
    fn tag_chips(&self, tags: &[model::TagChip], window: &Window) -> Div {
        let p = self.palette;
        let gap = space::S3;
        // What the row leaves the value: its icon, its label, the T keycap and the gaps between them.
        let room = self.columns(window).width(super::Pane::Detail)
            - 2. * desktop::DETAIL_PAD_X
            - component::ICON_SMALL
            - FIELD_LABEL
            - kit::kbd_width(window, "T")
            - 3. * desktop::PROP_GAP;
        let widths: Vec<f32> = tags
            .iter()
            .map(|t| {
                component::CHIP_DOT
                    + component::CHIP_GAP
                    + kit::text_width(window, ty::TAG, &t.name)
            })
            .collect();
        let more_width = |n: usize| kit::text_width(window, ty::TAG, &fit::more_label(n));
        // The first tag has no gap before it.
        let fit = fit::fit_tags(room + gap, &widths, gap, more_width);
        div()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap(px(gap))
            .children(tags.iter().take(fit.shown).map(|t| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(component::CHIP_GAP))
                    .child(
                        div()
                            .size(px(component::CHIP_DOT))
                            .rounded_full()
                            .bg(theme::list_colors(self.theme, t.hue).dot),
                    )
                    .child(
                        div()
                            .whitespace_nowrap()
                            .style(ty::TAG)
                            .text_color(p.ink_2)
                            .child(t.name.clone()),
                    )
            }))
            .when(fit.more > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .style(ty::TAG)
                        .text_color(p.ink_3)
                        .child(fit::more_label(fit.more)),
                )
            })
    }
}

/// A detail field's value: dates and times in mono, tags in quieter mono, words in the UI face. A value
/// longer than its row (many tags) ends in an ellipsis.
fn field_value(p: &Palette, kind: FieldKind, value: SharedString) -> Div {
    let v = div()
        .min_w_0()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(value);
    match kind {
        FieldKind::Tags => v.style(ty::TAG).text_color(p.ink_2),
        FieldKind::Date => v.style(ty::DUE.with_weight(600)).text_color(p.ink),
        FieldKind::Words => v.style(ty::NAV_COLLECTION).text_color(p.ink),
        FieldKind::Empty => v.style(ty::DUE).text_color(p.ink_4),
    }
}

/// The marks a priority shows on a row (`kit::priority`).
fn priority_level(p: Priority) -> u8 {
    match p {
        Priority::None => 0,
        Priority::Low => 1,
        Priority::Normal => 2,
        Priority::High => 3,
    }
}

/// Where the save dialog starts: the user's Downloads folder, or their profile.
fn downloads_dir() -> std::path::PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}
