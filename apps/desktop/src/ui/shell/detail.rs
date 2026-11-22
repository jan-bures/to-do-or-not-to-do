//! The detail pane: the selected task's title, steps, notes and fields, edited in place.
//!
//! Title and notes are edited directly in the pane and saved when editing ends (Enter or leaving the
//! title, Escape or leaving the notes). Steps can be added, completed, renamed (click the title), moved and
//! deleted. The due date is edited by typing a date; reminders and repeats are shown but edited elsewhere
//! for now. Web links open in the browser (anything else is shown as text). An attachment is saved where
//! the owner chooses; documents and images can also be opened, from this pairing's attachments directory
//! and marked as downloaded from the internet.

use gpui::{
    AnyElement, AsyncApp, Context, Div, Entity, FontWeight, SharedString, Subscription, WeakEntity,
    Window, div, prelude::*, px,
};
use todo_core::ids::{AttachmentId, TaskId};

use super::sidebar::InlineTarget;
use super::{Notice, ShellView, quick_add_colors};
use crate::model::draft::Draft;
use crate::model::files;
use crate::model::{self, Detail, FieldKind, commands, quick_add};
use crate::theme::{
    self, Icon, Palette, TextStyled, component, desktop, elevation, radius, space, ty,
};
use crate::ui::input::{InputEvent, TextInput};
use crate::ui::kit;
use crate::ui::text_area::TextArea;

/// The pane's editors and what they hold.
pub(super) struct Editors {
    pub title: Entity<TextArea>,
    pub notes: Entity<TextArea>,
    pub step: Entity<TextInput>,
    /// The due-date field while it is being edited.
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
        let mut subs = Vec::new();
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
            self.edit.loaded = None;
            self.edit.due = None;
            return;
        };
        let fresh = self.edit.loaded != Some(d.task);
        if fresh {
            self.edit.loaded = Some(d.task);
            self.edit.title_draft.discard();
            self.edit.notes_draft.discard();
            self.edit.due = None;
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

    fn step_command(&mut self, cmd: Option<todo_core::Command>, cx: &mut Context<Self>) {
        if let Some(cmd) = cmd {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    /// Opens the due-date editor (the D key, or a click on the field).
    pub(super) fn start_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.ui.task else {
            return;
        };
        let current = self
            .store
            .view()
            .tasks
            .get(&id)
            .and_then(|t| t.due)
            .map(|d| d.to_string());
        let colors = quick_add_colors(&self.palette);
        let input = cx.new(|cx| {
            let mut i = TextInput::new("tomorrow, fri, +3d or 2026-12-01", colors, cx);
            if let Some(c) = current {
                i.set_text(c, cx);
                i.select_everything(cx);
            }
            i
        });
        let events = cx.subscribe_in(
            &input,
            window,
            move |this, input, e: &InputEvent, window, cx| match e {
                InputEvent::Submit => {
                    let text = input.read(cx).content.to_string();
                    this.set_due_text(id, &text, cx);
                    this.edit.due = None;
                    window.focus(&this.list_focus, cx);
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

    fn set_due_text(&mut self, id: TaskId, text: &str, cx: &mut Context<Self>) {
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
            return;
        };
        if let Some(cmd) = commands::set_due(&self.store.view(), id, due) {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    /// Saves an attachment where the owner chooses (a save dialog, starting in Downloads). The default
    /// action for every attachment: nothing is run.
    fn save_attachment(&mut self, id: AttachmentId, name: SharedString, cx: &mut Context<Self>) {
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
                        tracing::warn!("saving an attachment failed: {e:#}");
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
    fn open_attachment(&mut self, id: AttachmentId, name: SharedString, cx: &mut Context<Self>) {
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

    fn step_row(&self, i: usize, s: &model::StepRow, cx: &mut Context<Self>) -> impl IntoElement {
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
                .style(ty::STEP)
                .child(input)
                .into_any_element(),
            None => {
                let t = div()
                    .id(("step-title", i))
                    .flex_1()
                    .min_w_0()
                    .truncate()
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
                .size(px(component::ICON_BUTTON - space::S2))
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
            .h(px(desktop::STEP_ROW))
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
        let toggle = |id: &'static str, icon: Icon, on: bool| {
            div()
                .id(id)
                .size(px(component::ICON_BUTTON))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(radius::MD))
                .hover(|s| s.bg(p.hover))
                .child(kit::icon(
                    icon,
                    component::GLYPH,
                    if on { p.accent_strong } else { p.ink_3 },
                ))
        };
        let links = d.links.iter().map(|l| {
            let url = l.url.clone();
            div()
                .id(SharedString::from(format!("link-{}", l.id)))
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
        });
        let steps: Vec<AnyElement> = d
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| self.step_row(i, s, cx).into_any_element())
            .collect();
        let attachments: Vec<_> = d
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let (id, name) = (a.id, a.name.clone());
                div()
                    .id(("attachment", i))
                    .mx(px(desktop::DETAIL_PAD_X))
                    .h(px(desktop::PROP_ROW))
                    .flex()
                    .items_center()
                    .gap(px(space::S2 + 2.))
                    .border_b_1()
                    .border_color(p.line)
                    .hover(|s| s.bg(p.hover))
                    .child(kit::icon(Icon::Paperclip, component::ICON_SMALL, p.ink_3))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .style(ty::NAV_SUB)
                            .text_color(p.ink)
                            .child(a.label.clone()),
                    )
                    .child(
                        div()
                            .style(ty::TAG)
                            .text_color(p.ink_3)
                            .child(a.detail.clone()),
                    )
                    .when(files::can_open(&a.name), |row| {
                        let name = a.name.clone();
                        row.child(
                            div()
                                .id(("open-attachment", i))
                                .px(px(space::S2))
                                .rounded(px(radius::MD))
                                .style(ty::TAG)
                                .text_color(p.accent_strong)
                                .hover(|s| s.bg(p.wash))
                                .child("Open")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_attachment(id, name.clone(), cx);
                                })),
                        )
                    })
                    .child(kit::icon(Icon::Download, component::ICON_SMALL, p.ink_3))
                    // Saving is the default for every attachment.
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.save_attachment(id, name.clone(), cx);
                    }))
            })
            .collect();
        let fields: Vec<_> = d
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let due = f.label == "Due";
                let value = match (&self.edit.due, due) {
                    (Some((input, _)), true) => div()
                        .flex_1()
                        .style(ty::DUE.with_weight(600))
                        .text_color(p.ink)
                        .child(input.clone())
                        .into_any_element(),
                    _ => field_value(&p, f.kind, f.value.clone()).into_any_element(),
                };
                div()
                    .id(("field", i))
                    .h(px(desktop::PROP_ROW))
                    .flex()
                    .items_center()
                    .gap(px(space::S2 + 2.))
                    .border_b_1()
                    .border_color(p.line)
                    .child(kit::icon(f.icon, component::ICON_SMALL, p.ink_3))
                    .child(
                        div()
                            .w(px(space::S10 * 2.))
                            .flex_none()
                            .style(ty::NAV_COLLECTION.with_weight(400))
                            .text_color(p.ink_3)
                            .child(f.label),
                    )
                    .child(value)
                    .when(due && self.edit.due.is_none(), |el| {
                        el.hover(|s| s.bg(p.hover))
                            .child(div().flex_1())
                            .child(kit::kbd(&p, "D"))
                            .on_click(cx.listener(|this, _, window, cx| this.start_due(window, cx)))
                    })
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
                    .h(px(desktop::DETAIL_TOP))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .pl(px(desktop::DETAIL_PAD_X))
                    .pr(px(space::S3))
                    .border_b_1()
                    .border_color(p.line)
                    .child(crumbs.style(ty::SUMMARY).text_color(p.ink_2))
                    .child(
                        // My Day, Important, Close: the same order on every client.
                        div()
                            .flex_none()
                            .flex()
                            .gap(px(space::S1))
                            .child(toggle("detail-my-day", Icon::Sun, d.my_day).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    let today = this.store.now().today();
                                    let cmd =
                                        commands::toggle_my_day(&this.store.view(), task, today);
                                    this.step_command(cmd, cx);
                                }),
                            ))
                            .child(
                                toggle("detail-important", Icon::Star, d.important).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        let cmd =
                                            commands::toggle_important(&this.store.view(), task);
                                        this.step_command(cmd, cx);
                                    }),
                                ),
                            )
                            .child(toggle("close-detail", Icon::X, false).on_click(
                                cx.listener(|this, _, _, cx| this.select_task(None, cx)),
                            )),
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
                                            div()
                                                .style(ty::TAG)
                                                .text_color(p.ink_2)
                                                .child(format!("{steps_done}/{steps_total}")),
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
                                    .when(live, |el| el.child(self.edit.step.clone()))
                                    .when(!live, |el| el.child("Add a step")),
                            )
                            .child(kit::kbd(&p, "Enter")),
                    )
                    .child(section("Notes"))
                    .child(
                        div()
                            .px(px(desktop::DETAIL_PAD_X))
                            .style(ty::NOTES)
                            .text_color(p.ink_2)
                            .when(live, |el| el.child(self.edit.notes.clone()))
                            .when(!live, |el| el.child(d.notes.clone())),
                    )
                    .child(
                        div()
                            .mt(px(space::S4))
                            .mx(px(desktop::DETAIL_PAD_X))
                            .border_t_1()
                            .border_color(p.line)
                            .children(fields),
                    )
                    .when(!d.links.is_empty(), |el| {
                        el.child(section(if has_mail { "Source" } else { "Links" }))
                            .children(links)
                    })
                    .when(!d.attachments.is_empty(), |el| {
                        el.child(section("Attachments")).children(attachments)
                    })
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
                        .child(kit::icon(Icon::Text, component::ICON + space::S1, p.ink_4))
                        .child(
                            div()
                                .style(ty::BODY)
                                .text_color(p.ink_3)
                                .child("Select a task to see its details"),
                        ),
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

/// Where the save dialog starts: the user's Downloads folder, or their profile.
fn downloads_dir() -> std::path::PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}
