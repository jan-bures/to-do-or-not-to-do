//! The detail pane: the selected task's title, steps, notes and fields, edited in place.
//!
//! Title and notes are edited directly in the pane and saved when editing ends (Enter or leaving the
//! title, Escape or leaving the notes). Steps can be added, completed, renamed (click the title), moved and
//! deleted. The due date is edited by typing a date; reminders and repeats are shown but edited elsewhere
//! for now. Links open in the browser; an attachment is downloaded to a temporary file and opened.

use gpui::{
    AnyElement, AsyncApp, Context, Div, Entity, FontWeight, SharedString, Subscription, WeakEntity,
    Window, div, prelude::*, px,
};
use todo_core::ids::{AttachmentId, TaskId};

use super::sidebar::InlineTarget;
use super::{Notice, ShellView, quick_add_colors};
use crate::model::{self, Detail, FieldKind, commands, quick_add};
use crate::theme::{Icon, Palette, TextStyled, component, desktop, elevation, radius, space, ty};
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
    pub title_dirty: bool,
    pub notes_dirty: bool,
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
                    InputEvent::Changed => this.edit.title_dirty = true,
                    InputEvent::Submit => {
                        this.commit_title(cx);
                        window.focus(&this.list_focus, cx);
                    }
                    InputEvent::Cancel => {
                        // Escape puts the saved title back.
                        this.edit.title_dirty = false;
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
                InputEvent::Changed => this.edit.notes_dirty = true,
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
        subs.push(cx.on_blur(&handle, window, |this, _, cx| this.commit_title(cx)));
        let handle = notes.read(cx).focus_handle.clone();
        subs.push(cx.on_blur(&handle, window, |this, _, cx| this.commit_notes(cx)));
        Self {
            title,
            notes,
            step,
            due: None,
            loaded: None,
            title_dirty: false,
            notes_dirty: false,
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
            self.edit.title_dirty = false;
            self.edit.notes_dirty = false;
            self.edit.due = None;
        }
        let (title, notes) = (d.title.clone(), d.notes.clone());
        if !self.edit.title_dirty && self.edit.title.read(cx).content != title {
            self.edit.title.update(cx, |a, cx| a.set_text(title, cx));
        }
        if !self.edit.notes_dirty && self.edit.notes.read(cx).content != notes {
            self.edit.notes.update(cx, |a, cx| a.set_text(notes, cx));
        }
    }

    pub(super) fn commit_title(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.edit.title_dirty) {
            return;
        }
        let Some(id) = self.edit.loaded else {
            return;
        };
        let text = self.edit.title.read(cx).content.to_string();
        if let Some(cmd) = commands::retitle(&self.store.view(), id, &text) {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    pub(super) fn commit_notes(&mut self, cx: &mut Context<Self>) {
        if !std::mem::take(&mut self.edit.notes_dirty) {
            return;
        }
        let Some(id) = self.edit.loaded else {
            return;
        };
        let text = self.edit.notes.read(cx).content.to_string();
        if let Some(cmd) = commands::set_notes(&self.store.view(), id, &text) {
            self.submit(vec![cmd]);
        }
        self.refresh(cx);
    }

    /// Saves whatever is being edited (before another task or room is shown).
    pub(super) fn commit_edits(&mut self, cx: &mut Context<Self>) {
        self.commit_title(cx);
        self.commit_notes(cx);
        self.commit_inline(cx);
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

    /// Downloads an attachment to a temporary file and opens it with its app.
    fn open_attachment(&mut self, id: AttachmentId, name: SharedString, cx: &mut Context<Self>) {
        let Some(session) = self.store.session() else {
            self.notice = Some(Notice::info("Attachments open when connected to a server."));
            cx.notify();
            return;
        };
        let dir = std::env::temp_dir()
            .join("to-do-or-not-to-do")
            .join("attachments");
        let job = session.download(id, &name, dir);
        self.notice = Some(Notice::info(format!("Downloading {name}…")));
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = job.await;
            let _ = this.update(cx, |v, cx| {
                match result {
                    Ok(path) => {
                        v.notice = None;
                        cx.open_with_system(&path);
                    }
                    Err(e) => {
                        tracing::warn!("opening an attachment failed: {e:#}");
                        v.notice = Some(Notice::error(format!("Couldn't open {name}: {e}")));
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

    fn detail(&self, d: &Detail, window: &Window, cx: &mut Context<Self>) -> Div {
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
        let crumbs = match &d.list.collection {
            Some(c) => div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(kit::chip(
                    t,
                    &model::ListRef {
                        collection: None,
                        name: c.clone(),
                        hue: d.list.hue,
                    },
                    false,
                ))
                .child(div().text_color(p.ink_4).child("/"))
                .child(
                    div()
                        .text_color(p.ink)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(d.list.name.clone()),
                ),
            None => div().child(kit::chip(t, &d.list, false)),
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
                            .child(a.name.clone()),
                    )
                    .child(
                        div()
                            .style(ty::TAG)
                            .text_color(p.ink_3)
                            .child(a.detail.clone()),
                    )
                    .child(kit::icon(Icon::Download, component::ICON_SMALL, p.ink_3))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_attachment(id, name.clone(), cx);
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
        let step_focused = self.edit.step.read(cx).focus_handle.is_focused(window);
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(desktop::DETAIL_TOP))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(desktop::DETAIL_PAD_X))
                    .pr(px(space::S3))
                    .border_b_1()
                    .border_color(p.line)
                    .child(crumbs.style(ty::SUMMARY).text_color(p.ink_2))
                    .child(
                        div()
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
                                    .child(self.edit.title.clone()),
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
                    .child(div().flex_1().min_w_0().child(self.edit.step.clone()))
                    .child(kit::kbd(&p, "Enter")),
            )
            .child(section("Notes"))
            .child(
                div()
                    .px(px(desktop::DETAIL_PAD_X))
                    .style(ty::NOTES)
                    .text_color(p.ink_2)
                    .child(self.edit.notes.clone()),
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
            .child(div().h(px(space::S6)))
    }

    pub(super) fn detail_pane(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = &self.palette;
        let pane = div()
            .id("detail")
            .w(px(desktop::DETAIL))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(p.pane)
            .border_l_1()
            .border_color(p.line);
        match self.shell.detail.clone() {
            Some(d) => pane.child(self.detail(&d, window, cx)),
            None => pane.items_center().justify_center().child(
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
            ),
        }
    }
}

/// A detail field's value: dates and times in mono, tags in quieter mono, words in the UI face.
fn field_value(p: &Palette, kind: FieldKind, value: SharedString) -> Div {
    let v = div().child(value);
    match kind {
        FieldKind::Tags => v.style(ty::TAG).text_color(p.ink_2),
        FieldKind::Date => v.style(ty::DUE.with_weight(600)).text_color(p.ink),
        FieldKind::Words => v.style(ty::NAV_COLLECTION).text_color(p.ink),
        FieldKind::Empty => v.style(ty::DUE).text_color(p.ink_4),
    }
}
