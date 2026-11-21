//! The main window: titlebar, sidebar, the room pane under its sky header, the detail pane and the status
//! bar. Step 3b ships these as a skeleton fed by `fixtures`; the real views arrive in step 7.

use gpui::{
    AnyElement, App, Context, Div, Entity, FocusHandle, Focusable, FontWeight, SharedString,
    Window, WindowControlArea, actions, div, prelude::*, px,
};

use super::input::{InputColors, InputEvent, TextInput};
use super::kit::{self, Type};
use super::landscape::{LandscapeCache, landscape};
use crate::fixtures::{Detail, Due, Eyebrow, Selected, Shell, Task};
use crate::theme::{self, Palette, Theme, radius, space, text, weight};

actions!(shell, [FocusQuickAdd]);

/// Key bindings for the main window.
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    vec![gpui::KeyBinding::new(
        "ctrl-n",
        FocusQuickAdd,
        Some("Shell"),
    )]
}

/// Colours for an input on the raised quick-add surface.
pub fn quick_add_colors(p: &Palette) -> InputColors {
    InputColors {
        text: p.ink,
        placeholder: p.ink_3,
        caret: p.accent_strong,
        selection: theme::with_alpha(p.accent, 0.28),
    }
}

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct ShellView {
    shell: Shell,
    theme: Theme,
    palette: Palette,
    landscape: LandscapeCache,
    quick_add: Entity<TextInput>,
    focus: FocusHandle,
}

impl ShellView {
    pub fn new(shell: Shell, theme: Theme, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let palette = theme::palette(theme, shell.room.hue);
        let placeholder = shell.room.quick_add_placeholder;
        let typed = shell.room.quick_add_text;
        let quick_add = cx.new(|cx| {
            let mut input = TextInput::new(placeholder, quick_add_colors(&palette), cx);
            if let Some(t) = typed {
                input.set_text(t, cx);
            }
            input
        });
        cx.subscribe_in(
            &quick_add,
            window,
            |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::Submit => {
                    let title = input.read(cx).content.trim().to_string();
                    if !title.is_empty() {
                        this.add_task(title);
                        input.update(cx, |i, cx| i.reset(window, cx));
                        cx.notify();
                    }
                }
                InputEvent::Cancel => window.focus(&this.focus, cx),
                InputEvent::Changed => cx.notify(),
            },
        )
        .detach();
        Self {
            shell,
            theme,
            palette,
            landscape: LandscapeCache::default(),
            quick_add,
            focus: cx.focus_handle(),
        }
    }

    /// Until sync lands, quick-add keeps tasks in memory so the shell can be tried out.
    pub fn add_task(&mut self, title: String) {
        self.shell.room.tasks.insert(
            0,
            Task {
                title: title.into(),
                list: None,
                tags: &[],
                steps: None,
                agent: None,
                priority: 0,
                due: Due::None,
                from_mail: false,
            },
        );
        self.shell.room.total += 1;
        self.shell.selected_task = self.shell.selected_task.map(|i| i + 1);
    }

    fn titlebar(&self) -> Div {
        let p = &self.palette;
        let control = |icon: &'static str, area: WindowControlArea, id: &'static str| {
            div()
                .id(id)
                .w(px(46.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(area)
                .hover(|s| s.bg(p.hover))
                .child(kit::icon(icon, 16., p.ink_2))
        };
        div()
            .h(px(space::TITLEBAR_H))
            .flex_none()
            .flex()
            .items_center()
            .bg(p.canvas)
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(16.))
                    .window_control_area(WindowControlArea::Drag)
                    .child(kit::icon("done", 18., p.accent_strong))
                    .child(
                        div()
                            .ui(text::TITLEBAR, 650.)
                            .text_color(p.ink)
                            .child("To-do"),
                    ),
            )
            .child(
                div()
                    .w(px(440.))
                    .h(px(26.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(8.))
                    .pr(px(4.))
                    .rounded(px(radius::MD))
                    .border_1()
                    .border_color(p.line_strong)
                    .bg(p.pane)
                    .child(kit::icon("search", 14., p.ink_3))
                    .child(
                        div()
                            .flex_1()
                            .ui((12.5, 16.), weight::REGULAR)
                            .text_color(p.ink_3)
                            .child("Search tasks, lists, commands"),
                    )
                    .child(kit::kbd(p, "Ctrl K")),
            )
            .child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .justify_end()
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    )
                    .child(control("min", WindowControlArea::Min, "win-min"))
                    .child(control("max", WindowControlArea::Max, "win-max"))
                    .child(control("x", WindowControlArea::Close, "win-close")),
            )
    }

    fn sidebar(&self) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let sb = &self.shell.sidebar;
        let selected_bg = |on: bool, el: Div| {
            if on {
                el.bg(p.raised)
                    .border_1()
                    .border_color(p.line)
                    .text_color(p.ink)
                    .font_weight(FontWeight(weight::SEMIBOLD))
            } else {
                el.border_1().border_color(gpui::transparent_black())
            }
        };
        let count = |n: u32| {
            div()
                .mono(text::COUNT, weight::REGULAR)
                .text_color(p.ink_3)
                .child(n.to_string())
        };

        let smart = sb.smart.iter().enumerate().map(|(i, item)| {
            let row = div()
                .h(px(space::NAV_H))
                .flex()
                .items_center()
                .gap(px(10.))
                .pl(px(8.))
                .pr(px(10.))
                .rounded(px(radius::MD))
                .ui(text::NAV, weight::MEDIUM)
                .text_color(p.ink_2)
                .child(kit::icon(item.icon, 18., theme::glyph_color(t, item.hue)))
                .child(div().flex_1().child(item.label))
                .children(item.count.map(count))
                .children(item.pip.map(|pip| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .mono(text::COUNT, weight::REGULAR)
                        .text_color(p.accent_strong)
                        .child(div().size(px(6.)).rounded_full().bg(p.accent))
                        .child(pip)
                }));
            selected_bg(sb.selected == Selected::Smart(i), row)
        });

        let collections = sb.collections.iter().map(|c| {
            let head = div()
                .h(px(28.))
                .mt(px(12.))
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(4.))
                .pr(px(10.))
                .rounded(px(radius::MD))
                .ui((13., 16.), weight::SEMIBOLD)
                .text_color(p.ink)
                .child(kit::icon("chev", 14., p.ink_4))
                .child(kit::glyph(c.glyph, 16., theme::glyph_color(t, c.hue)))
                .child(div().flex_1().child(c.name))
                .child(count(c.count));
            let head = selected_bg(sb.selected == Selected::Collection(c.name), head);
            let lists = c.lists.iter().map(|(list, glyph, n)| {
                let row = div()
                    .h(px(space::SUB_NAV_H))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .pl(px(28.))
                    .pr(px(10.))
                    .rounded(px(radius::MD))
                    .ui(text::SUB_NAV, weight::REGULAR)
                    .text_color(p.ink_2)
                    .child(kit::glyph(
                        glyph,
                        16.,
                        theme::list_colors(t, list.hue).glyph,
                    ))
                    .child(div().flex_1().child(list.name))
                    .child(count(*n));
                selected_bg(sb.selected == Selected::List(list.name), row)
            });
            div().flex().flex_col().child(head).children(lists)
        });

        let agents = (!sb.agents.is_empty()).then(|| {
            div()
                .border_t_1()
                .border_color(p.line)
                .pt(px(12.))
                .px(px(4.))
                .pb(px(8.))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .px(px(4.))
                        .mb(px(4.))
                        .child(kit::label(p, "Agents"))
                        .child(kit::label(p, "Today")),
                )
                .children(sb.agents.iter().map(|a| {
                    div()
                        .h(px(32.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .px(px(4.))
                        .ui(text::SUB_NAV, weight::MEDIUM)
                        .text_color(p.ink_2)
                        .child(kit::agent_mark(t, a.agent, 20.))
                        .child(div().flex_1().child(a.name))
                        .child(
                            div()
                                .ui(text::COUNT, weight::REGULAR)
                                .text_color(p.ink_3)
                                .child(a.status),
                        )
                }))
        });

        div()
            .w(px(space::SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .border_r_1()
            .border_color(p.line)
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .pt(px(12.))
                    .px(px(8.))
                    .gap(px(0.))
                    .children(smart)
                    .children(collections)
                    .child(
                        div()
                            .h(px(32.))
                            .mt(px(12.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .pl(px(8.))
                            .pr(px(10.))
                            .ui(text::SUB_NAV, weight::REGULAR)
                            .text_color(p.ink_3)
                            .child(kit::icon("plus", 16., p.ink_3))
                            .child(div().flex_1().child("New list"))
                            .child(kit::kbd(p, "Ctrl N")),
                    )
                    .child(div().flex_1())
                    .children(agents),
            )
            .child(
                div()
                    .h(px(44.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(16.))
                    .border_t_1()
                    .border_color(p.line)
                    .text_color(p.ink_3)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .ui(text::COUNT, weight::REGULAR)
                            .child(div().size(px(6.)).rounded_full().bg(p.success))
                            .child(sb.synced),
                    )
                    .child(kit::icon("sliders", 16., p.ink_3)),
            )
    }

    fn eyebrow(&self) -> Div {
        let p = &self.palette;
        let bar = || div().w(px(1.)).h(px(10.)).bg(p.line_strong);
        let mono = |s: String| {
            div()
                .mono(text::EYEBROW_MONO, weight::REGULAR)
                .text_color(p.ink_3)
                .child(s.to_uppercase())
        };
        let dow = |s: SharedString| {
            div()
                .display(text::EYEBROW_DOW, weight::REGULAR)
                .italic()
                .text_color(p.ink_2)
                .child(s)
        };
        let row = div().flex().items_center().gap(px(10.)).h(px(22.));
        match &self.shell.room.eyebrow {
            Eyebrow::Day {
                weekday,
                date,
                week,
            } => row
                .child(dow(weekday.clone().into()))
                .child(bar())
                .child(mono(date.clone()))
                .child(bar())
                .child(mono(week.clone())),
            Eyebrow::Kind { kind, detail } => row
                .child(dow((*kind).into()))
                .child(bar())
                .child(mono(detail.clone())),
        }
    }

    fn summary(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        let (done, total, open) = crate::fixtures::summary(room);
        let sep = || div().size(px(3.)).rounded_full().bg(p.ink_4);
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .pb(px(12.))
            .ui(text::SUMMARY, weight::REGULAR)
            .text_color(p.ink_2);
        if total == 0 {
            let finished = room.completed;
            return row.child(
                div()
                    .mono((12., 16.), weight::REGULAR)
                    .child(if finished > 0 {
                        format!("{finished} finished this month")
                    } else {
                        "nothing yet".to_string()
                    }),
            );
        }
        row = row
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(4.))
                    .child(
                        div()
                            .mono((12.5, 16.), weight::SEMIBOLD)
                            .text_color(p.ink)
                            .child(done.to_string()),
                    )
                    .child(format!("of {total} done")),
            )
            .child(sep())
            .child(
                div()
                    .mono((12., 16.), weight::REGULAR)
                    .child(format!("{open} open")),
            );
        if let Some(est) = room.estimate {
            row = row.child(sep()).child(
                div()
                    .mono((12., 16.), weight::SEMIBOLD)
                    .text_color(p.accent_strong)
                    .child(format!("est. {est}")),
            );
        }
        row
    }

    fn header(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(p.sky)
            .child(
                div()
                    .pt(px(20.))
                    .px(px(space::PANE_PAD_X))
                    .pb(px(6.))
                    .child(
                        div()
                            .h(px(28.))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(self.eyebrow())
                            .child(
                                div()
                                    .flex()
                                    .gap(px(4.))
                                    .mr(px(-8.))
                                    .child(
                                        div()
                                            .size(px(32.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(kit::icon("sliders", 18., p.ink_3)),
                                    )
                                    .child(
                                        div()
                                            .size(px(32.))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(kit::icon("more", 18., p.ink_3)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .flex()
                            .items_end()
                            .justify_between()
                            .child(
                                div()
                                    .display(text::ROOM_TITLE, weight::DISPLAY)
                                    .text_color(p.ink)
                                    .child(room.title),
                            )
                            .child(self.summary()),
                    ),
            )
            .child(landscape(
                &self.landscape,
                self.palette,
                room.done,
                room.total,
                room.total == 0,
            ))
            .child(div().h(px(1.)).bg(p.line))
    }

    fn quick_add(&self, window: &Window, cx: &App) -> Div {
        let p = &self.palette;
        let focused = self.quick_add.read(cx).focus_handle.is_focused(window);
        let typed = !self.quick_add.read(cx).content.is_empty();
        let field = div()
            .h(px(space::QUICK_ADD_H))
            .flex()
            .items_center()
            .gap(px(12.))
            .pl(px(14.))
            .pr(px(10.))
            .rounded(px(radius::LG))
            .border_1()
            .border_color(if focused || typed {
                p.accent
            } else {
                p.control
            })
            .bg(p.raised)
            .ui(text::QUICK_ADD, weight::REGULAR)
            .child(kit::icon("plus", 18., p.accent_strong))
            .child(self.quick_add.clone());
        let field = if typed {
            field
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .h(px(22.))
                        .rounded(px(radius::SM))
                        .bg(p.wash)
                        .text_color(p.accent_strong)
                        .mono((11.5, 22.), weight::MEDIUM)
                        .child(kit::icon("cal", 14., p.accent_strong))
                        .child("Sun 22 Nov"),
                )
                .child(kit::kbd(p, "Enter"))
        } else {
            field
                .child(kit::icon("cal", 16., p.ink_3))
                .child(kit::icon("flag", 16., p.ink_3))
                .child(kit::kbd(p, "N"))
        };
        // The focus ring is a 3 px band of the room tint around the field (a border, not a shadow).
        let ring = if focused || typed {
            p.tint
        } else {
            gpui::transparent_black()
        };
        div()
            .mx(px(space::PANE_PAD_X - 3.))
            .my(px(13.))
            .p(px(3.))
            .rounded(px(radius::LG + 3.))
            .bg(ring)
            .child(field)
    }

    fn row(&self, i: usize, task: &Task) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let selected = self.shell.selected_task == Some(i);
        let overdue = matches!(task.due, Due::Overdue(_));
        let mut meta = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .h(px(20.))
            .mt(px(4.))
            .overflow_hidden();
        if let Some(list) = &task.list {
            meta = meta.child(kit::chip(t, list, self.shell.room.smart));
        }
        if let Some((d, n)) = task.steps {
            meta = meta.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_color(p.ink_3)
                    .mono((11.5, 16.), weight::MEDIUM)
                    .child(kit::meter(p, d, n))
                    .child(format!("{d}/{n}")),
            );
        }
        for tag in task.tags {
            meta = meta.child(
                div()
                    .mono((12., 16.), weight::MEDIUM)
                    .text_color(p.ink_3)
                    .child(*tag),
            );
        }
        if task.from_mail {
            meta = meta.child(kit::icon("mail", 14., p.ink_3));
        }
        let due: AnyElement = match &task.due {
            Due::None => div()
                .mono((12.5, 20.), weight::MEDIUM)
                .text_color(p.ink_4)
                .child("–")
                .into_any_element(),
            Due::Soon(s) => div()
                .mono((12.5, 20.), weight::SEMIBOLD)
                .text_color(p.accent_strong)
                .child(*s)
                .into_any_element(),
            Due::Plain(s) => div()
                .mono((12.5, 20.), weight::MEDIUM)
                .text_color(p.ink_2)
                .child(*s)
                .into_any_element(),
            Due::Overdue(s) => kit::overdue_badge(p, s).into_any_element(),
        };
        let row = div()
            .relative()
            .h(px(space::ROW_H))
            .flex_none()
            .flex()
            .items_start()
            .gap(px(space::COL_GAP))
            .pt(px(8.))
            .px(px(space::PANE_PAD_X))
            .border_b_1()
            .border_color(p.line)
            .child(kit::checkbox(p, overdue, space::CHECKBOX))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .ui(text::TASK_TITLE, weight::MEDIUM)
                            .text_color(p.ink)
                            .truncate()
                            .child(task.title.clone()),
                    )
                    .child(meta),
            )
            .child(
                div()
                    .w(px(space::COL_AGENT))
                    .flex_none()
                    .children(task.agent.map(|a| kit::agent_mark(t, a, 20.))),
            )
            .child(
                div()
                    .w(px(space::COL_PRIORITY))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(kit::priority(p, task.priority)),
            )
            .child(
                div()
                    .w(px(space::COL_DUE))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(due),
            );
        if selected {
            row.bg(p.wash).child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(-1.))
                    .bottom(px(-1.))
                    .w(px(3.))
                    .bg(p.accent),
            )
        } else {
            row
        }
    }

    fn list(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        let body = if room.tasks.is_empty() {
            div()
                .px(px(space::PANE_PAD_X))
                .pt(px(28.))
                .child(
                    div()
                        .display((30., 36.), weight::REGULAR)
                        .italic()
                        .text_color(p.ink)
                        .child(room.empty_line),
                )
                .child(
                    div()
                        .mt(px(10.))
                        .ui(text::BODY, weight::REGULAR)
                        .text_color(p.ink_3)
                        .child("Add one above, or press Ctrl+Alt+Space from anywhere."),
                )
        } else {
            div()
                .flex()
                .flex_col()
                .children(room.tasks.iter().enumerate().map(|(i, t)| self.row(i, t)))
        };
        let completed = (room.completed > 0).then(|| {
            div()
                .h(px(56.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(space::PANE_PAD_X))
                .border_t_1()
                .border_color(p.line)
                .child(kit::icon("chevr", 14., p.ink_3))
                .child(kit::label(p, "Completed"))
                .child(
                    div()
                        .mono(text::LABEL, weight::SEMIBOLD)
                        .text_color(p.ink_3)
                        .child(room.completed.to_string()),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().h(px(1.)).bg(p.line))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .children(completed)
    }

    fn detail(&self, d: &Detail) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let lc = theme::list_colors(t, d.list.hue);
        let steps_done = d.steps.iter().filter(|s| s.done).count() as u32;
        let steps_total = d.steps.len() as u32;
        let step_row = |i: usize, s: &crate::fixtures::Step| {
            let selected = i == steps_done as usize;
            let mark = if s.done {
                div()
                    .size(px(16.))
                    .rounded_full()
                    .bg(p.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(kit::icon("check", 12., p.on_accent))
            } else {
                kit::checkbox(p, false, 16.)
            };
            let title = div().flex_1().truncate().ui((13.5, 20.), weight::REGULAR);
            let title = if s.done {
                title.text_color(p.ink_3).line_through()
            } else {
                title.text_color(p.ink)
            };
            let row = div()
                .relative()
                .h(px(30.))
                .flex()
                .items_center()
                .gap(px(12.))
                .px(px(24.))
                .border_b_1()
                .border_color(p.line)
                .child(mark)
                .child(title.child(s.title))
                .child(
                    div()
                        .mono((11.5, 16.), weight::REGULAR)
                        .text_color(p.ink_4)
                        .child(format!("{:02}", i + 1)),
                );
            if selected {
                row.bg(p.wash).child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .bg(p.accent),
                )
            } else {
                row
            }
        };
        let section = |label: &'static str| {
            div()
                .px(px(24.))
                .pt(px(20.))
                .pb(px(6.))
                .child(kit::label(p, label))
        };
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .pl(px(24.))
                    .pr(px(16.))
                    .border_b_1()
                    .border_color(p.line)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .ui((12.5, 16.), weight::REGULAR)
                            .text_color(p.ink_2)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .h(px(20.))
                                    .px(px(8.))
                                    .rounded(px(radius::CHIP))
                                    .bg(lc.tint)
                                    .text_color(lc.ink)
                                    .ui((12., 20.), 550.)
                                    .child(div().size(px(6.)).rounded_full().bg(lc.dot))
                                    .child(d.list.collection),
                            )
                            .child(div().text_color(p.ink_4).child("/"))
                            .child(
                                div()
                                    .text_color(p.ink)
                                    .font_weight(FontWeight(weight::SEMIBOLD))
                                    .child(d.list.name),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(14.))
                            .child(kit::icon("star", 16., p.ink_3))
                            .child(kit::icon("more", 16., p.ink_3))
                            .child(kit::icon("x", 16., p.ink_3)),
                    ),
            )
            .child(
                div()
                    .px(px(24.))
                    .pt(px(20.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(kit::checkbox(p, false, 22.))
                            .child(
                                div()
                                    .ui(text::DETAIL_TITLE, weight::SEMIBOLD)
                                    .text_color(p.ink)
                                    .child(d.title),
                            ),
                    )
                    .child(
                        div()
                            .ml(px(34.))
                            .mt(px(4.))
                            .mono((11.5, 16.), weight::REGULAR)
                            .text_color(p.ink_3)
                            .child(d.subtitle),
                    ),
            )
            .child(
                div()
                    .px(px(24.))
                    .pt(px(20.))
                    .pb(px(4.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(kit::label(p, "Steps"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(kit::meter(p, steps_done, steps_total))
                            .child(
                                div()
                                    .mono((12., 16.), weight::MEDIUM)
                                    .text_color(p.ink_2)
                                    .child(format!("{steps_done}/{steps_total}")),
                            ),
                    ),
            )
            .child(div().h(px(1.)).mx(px(24.)).bg(p.line))
            .children(d.steps.iter().enumerate().map(|(i, s)| step_row(i, s)))
            .child(
                div()
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(24.))
                    .ui((13.5, 20.), weight::REGULAR)
                    .text_color(p.ink_3)
                    .child(kit::icon("plus", 16., p.ink_3))
                    .child(div().flex_1().child("Add step"))
                    .child(kit::kbd(p, "Tab")),
            )
            .child(section("Notes"))
            .child(
                div()
                    .px(px(24.))
                    .ui((14., 22.), weight::REGULAR)
                    .text_color(p.ink_2)
                    .child(d.notes),
            )
            .child(
                div()
                    .mt(px(16.))
                    .mx(px(24.))
                    .border_t_1()
                    .border_color(p.line)
                    .children(d.fields.iter().map(|(icon, label, value)| {
                        div()
                            .h(px(34.))
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .border_b_1()
                            .border_color(p.line)
                            .child(kit::icon(icon, 15., p.ink_3))
                            .child(
                                div()
                                    .w(px(74.))
                                    .ui((13., 16.), weight::REGULAR)
                                    .text_color(p.ink_3)
                                    .child(*label),
                            )
                            .child(field_value(p, value))
                    })),
            )
    }

    fn detail_pane(&self) -> Div {
        let p = &self.palette;
        let pane = div()
            .w(px(space::DETAIL_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(p.pane)
            .border_l_1()
            .border_color(p.line);
        match &self.shell.detail {
            Some(d) => pane.child(self.detail(d)),
            None => pane.items_center().justify_center().child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(kit::icon("note", 22., p.ink_4))
                    .child(
                        div()
                            .ui(text::BODY, weight::REGULAR)
                            .text_color(p.ink_3)
                            .child("Select a task to see its details"),
                    ),
            ),
        }
    }

    fn status_bar(&self) -> Div {
        let p = &self.palette;
        let (done, _, open) = crate::fixtures::summary(&self.shell.room);
        let hint = |key: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(kit::kbd(p, key))
                .child(what)
        };
        let strong = |s: String| {
            div()
                .text_color(p.ink_2)
                .font_weight(FontWeight(weight::MEDIUM))
                .child(s)
        };
        div()
            .h(px(space::STATUS_H))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(16.))
            .border_t_1()
            .border_color(p.line)
            .bg(p.canvas)
            .mono((11.5, 16.), weight::REGULAR)
            .text_color(p.ink_3)
            .child(
                div()
                    .flex()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(strong(open.to_string()))
                            .child("open"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(strong(format!("{done:02}")))
                            .child("done"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(18.))
                    .ui((12., 16.), weight::REGULAR)
                    .child(hint("J K", "move"))
                    .child(hint("Space", "complete"))
                    .child(hint("E", "edit"))
                    .child(hint("D", "due")),
            )
            .child(div().child("Ctrl Z undoes the last agent change"))
    }
}

impl Focusable for ShellView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ShellView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        div()
            .id("shell")
            .key_context("Shell")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &FocusQuickAdd, window, cx| {
                let handle = this.quick_add.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .font_family(theme::font::UI)
            .text_color(p.ink)
            .child(self.titlebar())
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .border_t_1()
                    .border_color(p.line)
                    .child(self.sidebar())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .bg(p.pane)
                            .child(self.header())
                            .child(self.quick_add(window, cx))
                            .child(self.list()),
                    )
                    .child(self.detail_pane()),
            )
            .child(self.status_bar())
    }
}

/// A detail field's value: dates and times in mono, tags in quieter mono, words in the UI face.
fn field_value(p: &Palette, value: &'static str) -> Div {
    let v = div().child(value);
    if value.starts_with('#') {
        v.mono((12.5, 16.), weight::MEDIUM).text_color(p.ink_2)
    } else if value.chars().any(|c| c.is_ascii_digit()) {
        v.mono((12.5, 16.), weight::SEMIBOLD).text_color(p.ink)
    } else {
        v.ui((13., 16.), weight::SEMIBOLD).text_color(p.ink)
    }
}
