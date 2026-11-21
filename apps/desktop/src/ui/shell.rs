//! The main window: titlebar, sidebar, the room pane under its sky header, the detail pane and the status
//! bar. Step 3b ships these as a skeleton fed by `fixtures`; the real views arrive in step 7.

use gpui::{
    AnyElement, App, Context, Div, Entity, FocusHandle, Focusable, FontWeight, Hsla, SharedString,
    Window, WindowControlArea, actions, div, prelude::*, px,
};

use super::input::{InputColors, InputEvent, TextInput};
use super::kit;
use super::landscape::{LandscapeCache, Sky, landscape};
use crate::fixtures::{Detail, Due, Eyebrow, ListRef, Selected, Shell, Task, Tomorrow};
use crate::theme::{
    self, Icon, Palette, TextStyled, Theme, component, desktop, elevation, radius, space, ty,
};

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
        let control = |icon: Icon, area: WindowControlArea, id: &'static str| {
            div()
                .id(id)
                // Windows caption buttons are 46 px wide; the design has no token for OS chrome.
                .w(px(46.))
                .h_full()
                .flex()
                .items_center()
                .justify_center()
                .window_control_area(area)
                .hover(|s| s.bg(p.hover))
                .child(kit::icon(icon, component::GLYPH, p.ink_2))
        };
        div()
            .h(px(desktop::TITLEBAR))
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
                    .gap(px(space::S2))
                    .pl(px(space::S4))
                    .window_control_area(WindowControlArea::Drag)
                    .child(kit::icon(
                        Icon::CircleCheck,
                        component::ICON,
                        p.accent_strong,
                    ))
                    .child(
                        div()
                            .style(ty::NAV_COLLECTION.with_weight(650))
                            .text_color(p.ink)
                            .child("To-do"),
                    ),
            )
            .child(
                div()
                    // The search field is centred in the title bar at the mockup width.
                    .w(px(440.))
                    .h(px(desktop::TITLEBAR - space::S2 - space::S1 / 2.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .pl(px(space::S2))
                    .pr(px(space::S1))
                    .rounded(px(radius::MD))
                    .border(px(elevation::HAIRLINE))
                    .border_color(p.line_strong)
                    .bg(p.pane)
                    .child(kit::icon(Icon::Search, component::ICON_SMALL, p.ink_3))
                    .child(
                        div()
                            .flex_1()
                            .style(ty::SUMMARY)
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
                    .child(control(Icon::Minus, WindowControlArea::Min, "win-min"))
                    .child(control(Icon::Square, WindowControlArea::Max, "win-max"))
                    .child(control(Icon::X, WindowControlArea::Close, "win-close")),
            )
    }

    fn sidebar(&self) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let sb = &self.shell.sidebar;
        let selected_bg = |on: bool, el: Div| {
            if on {
                el.bg(p.raised)
                    .border(px(elevation::HAIRLINE))
                    .border_color(p.line)
                    .text_color(p.ink)
                    .font_weight(FontWeight::SEMIBOLD)
            } else {
                el.border(px(elevation::HAIRLINE))
                    .border_color(gpui::transparent_black())
            }
        };
        let count = |n: u32| {
            div()
                .style(ty::COUNT)
                .text_color(p.ink_3)
                .child(n.to_string())
        };

        let smart = sb.smart.iter().enumerate().map(|(i, item)| {
            let row = div()
                .h(px(desktop::NAV_ROW))
                .flex()
                .items_center()
                .gap(px(space::S2 + 2.))
                .pl(px(space::S2))
                .pr(px(space::S2 + 2.))
                .rounded(px(radius::MD))
                .style(ty::NAV)
                .text_color(p.ink_2)
                .child(kit::icon(
                    item.icon,
                    component::ICON,
                    theme::list_colors(t, item.hue).glyph,
                ))
                .child(div().flex_1().child(item.label))
                .children(item.count.map(count))
                .children(item.pip.map(|pip| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(component::CHIP_GAP))
                        .style(ty::COUNT)
                        .text_color(p.accent_strong)
                        .child(
                            div()
                                .size(px(component::CHIP_DOT))
                                .rounded_full()
                                .bg(p.accent),
                        )
                        .child(pip)
                }));
            selected_bg(sb.selected == Selected::Smart(i), row)
        });

        let collections = sb.collections.iter().map(|c| {
            let head = div()
                .h(px(desktop::NAV_SUB_ROW))
                .mt(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .pl(px(space::S1))
                .pr(px(space::S2 + 2.))
                .rounded(px(radius::MD))
                .style(ty::NAV_COLLECTION)
                .text_color(p.ink)
                .child(kit::icon(Icon::ChevronDown, component::ICON_SMALL, p.ink_4))
                .child(kit::glyph(c.glyph, theme::list_colors(t, c.hue).glyph))
                .child(div().flex_1().child(c.name))
                .child(count(c.count));
            let head = selected_bg(sb.selected == Selected::Collection(c.name), head);
            let lists = c.lists.iter().map(|(list, glyph, n)| {
                let row = div()
                    .h(px(desktop::NAV_SUB_ROW))
                    .flex()
                    .items_center()
                    .gap(px(space::S2 + 2.))
                    .pl(px(space::S6 + space::S1))
                    .pr(px(space::S2 + 2.))
                    .rounded(px(radius::MD))
                    .style(ty::NAV_SUB)
                    .text_color(p.ink_2)
                    .child(kit::glyph(*glyph, theme::list_colors(t, list.hue).glyph))
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
                .pt(px(space::S3))
                .px(px(space::S1))
                .pb(px(space::S2))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .px(px(space::S1))
                        .mb(px(space::S1))
                        .child(kit::label(p, "Agents"))
                        .child(kit::label(p, "Today")),
                )
                .children(sb.agents.iter().map(|a| {
                    div()
                        .h(px(desktop::NAV_ROW))
                        .flex()
                        .items_center()
                        .gap(px(space::S2 + 2.))
                        .px(px(space::S1))
                        .style(ty::NAV_SUB.with_weight(500))
                        .text_color(p.ink_2)
                        .child(kit::agent_mark(t, a.agent, component::AGENT_MARK))
                        .child(div().flex_1().child(a.name))
                        .child(div().style(ty::COUNT).text_color(p.ink_3).child(a.status))
                }))
        });

        div()
            .w(px(desktop::SIDEBAR))
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
                    .pt(px(space::S3))
                    .px(px(space::S2))
                    .children(smart)
                    .children(collections)
                    .child(
                        div()
                            .h(px(desktop::NAV_ROW))
                            .mt(px(space::S3))
                            .flex()
                            .items_center()
                            .gap(px(space::S2 + 2.))
                            .pl(px(space::S2))
                            .pr(px(space::S2 + 2.))
                            .style(ty::NAV_SUB)
                            .text_color(p.ink_3)
                            .child(kit::icon(Icon::Plus, component::GLYPH, p.ink_3))
                            .child(div().flex_1().child("New list"))
                            .child(kit::kbd(p, "Ctrl N")),
                    )
                    .child(div().flex_1())
                    .children(agents),
            )
            .child(
                div()
                    .h(px(desktop::SIDEBAR_FOOTER))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(space::S4))
                    .border_t_1()
                    .border_color(p.line)
                    .text_color(p.ink_3)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .style(ty::COUNT)
                            .child(
                                div()
                                    .size(px(component::CHIP_DOT))
                                    .rounded_full()
                                    .bg(p.success),
                            )
                            .child(sb.synced),
                    )
                    .child(kit::icon(
                        Icon::SlidersHorizontal,
                        component::GLYPH,
                        p.ink_3,
                    )),
            )
    }

    fn eyebrow(&self) -> Div {
        let (ink_2, ink_3, line) = self.header_inks();
        let bar = || {
            div()
                .w(px(elevation::HAIRLINE))
                .h(px(ty::CAPS_LABEL.size - 0.5))
                .bg(line)
        };
        let mono = |s: String| {
            div()
                .style(ty::EYEBROW_META)
                .text_color(ink_3)
                .child(s.to_uppercase())
        };
        let dow = |s: SharedString| div().style(ty::EYEBROW_DAY).text_color(ink_2).child(s);
        let row = div()
            .flex()
            .items_center()
            .gap(px(space::S2 + 2.))
            .h(px(ty::EYEBROW_DAY.line_height));
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
        let (ink_2, _, line) = self.header_inks();
        let finished_day = self.finished_day();
        let ink = if finished_day { ink_2 } else { p.ink };
        let strong = if finished_day { ink_2 } else { p.accent_strong };
        let sep = || {
            div()
                .size(px(elevation::SELECTION_BAR))
                .rounded_full()
                .bg(line)
        };
        let mut row = div()
            .flex()
            .items_center()
            .gap(px(space::S2 + 2.))
            .pb(px(space::S3))
            .style(ty::SUMMARY)
            .text_color(ink_2);
        if total == 0 {
            let finished = room.completed;
            return row.child(div().style(ty::SUMMARY_FIGURES).child(if finished > 0 {
                format!("{finished} finished this month")
            } else {
                "nothing yet".to_string()
            }));
        }
        row = row
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .style(ty::SUMMARY_FIGURES.with_weight(600))
                            .text_color(ink)
                            .child(done.to_string()),
                    )
                    .child(format!("of {total} done")),
            )
            .child(sep())
            .child(if finished_day {
                // The day is done: the count says so in the accent, without congratulations.
                div()
                    .style(ty::SUMMARY_FIGURES.with_weight(600))
                    .text_color(self.dusk_accent())
                    .child("all clear")
            } else {
                div()
                    .style(ty::SUMMARY_FIGURES)
                    .child(format!("{open} open"))
            });
        if let Some(est) = room.estimate {
            row = row.child(sep()).child(
                div()
                    .style(ty::SUMMARY_FIGURES.with_weight(600))
                    .text_color(strong)
                    .child(format!("est. {est}")),
            );
        }
        row
    }

    /// My Day with every task done: the header turns into the finished day's dusk scene.
    fn finished_day(&self) -> bool {
        let room = &self.shell.room;
        room.finishes_day && room.total > 0 && room.done >= room.total
    }

    /// (secondary ink, metadata ink, separator) for text on the header. The dusk scene is a night scene in
    /// both themes, so its header uses the dark theme's inks.
    fn header_inks(&self) -> (Hsla, Hsla, Hsla) {
        let p = if self.finished_day() {
            theme::palette(Theme::Dark, self.shell.room.hue)
        } else {
            self.palette
        };
        (p.ink_2, p.ink_3, p.line_strong)
    }

    /// The accent on the dusk scene: the dark theme's, since the scene is a night scene.
    fn dusk_accent(&self) -> Hsla {
        theme::palette(Theme::Dark, self.shell.room.hue).accent_strong
    }

    fn header(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        let finished = self.finished_day();
        let (sky_bg, title_ink, sky) = if finished {
            let dusk = theme::dusk(room.hue);
            let ink = theme::palette(Theme::Dark, room.hue).ink;
            (dusk.top, ink, Sky::Finished(dusk))
        } else {
            (p.sky, p.ink, Sky::Slim(self.palette))
        };
        let (_, ink_3, _) = self.header_inks();
        let action = |icon: Icon| {
            div()
                .size(px(component::ICON_BUTTON))
                .flex()
                .items_center()
                .justify_center()
                .child(kit::icon(icon, component::ICON, ink_3))
        };
        div()
            .flex_none()
            .flex()
            .flex_col()
            .bg(sky_bg)
            .child(
                div()
                    .pt(px(desktop::HEADER_PAD_TOP))
                    .pb(px(space::S2))
                    .px(px(desktop::LIST_PAD_X))
                    .child(
                        div()
                            .h(px(desktop::EYEBROW))
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(self.eyebrow())
                            .child(
                                div()
                                    .flex()
                                    .gap(px(space::S1))
                                    .mr(px(-space::S2))
                                    .child(action(Icon::SlidersHorizontal))
                                    .child(action(Icon::Ellipsis)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .justify_between()
                            .child(
                                div()
                                    .style(ty::ROOM_TITLE)
                                    .text_color(title_ink)
                                    .child(room.title),
                            )
                            .child(self.summary()),
                    ),
            )
            // The finished scene's rings reach past the strip; the header ends at its bottom edge.
            .child(div().overflow_hidden().child(landscape(
                &self.landscape,
                sky,
                room.done,
                room.total,
            )))
            .child(div().h(px(elevation::HAIRLINE)).bg(p.line))
    }

    fn quick_add(&self, window: &Window, cx: &App) -> Div {
        let p = &self.palette;
        let focused = self.quick_add.read(cx).focus_handle.is_focused(window);
        let typed = !self.quick_add.read(cx).content.is_empty();
        let field = div()
            .h(px(desktop::QUICK_ADD))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .pl(px(space::S3 + 2.))
            .pr(px(space::S2 + 2.))
            .rounded(px(radius::QUICK_ADD))
            .border(px(elevation::HAIRLINE))
            .border_color(if focused || typed {
                p.accent
            } else {
                p.control
            })
            .bg(p.raised)
            .style(ty::QUICK_ADD)
            .child(kit::icon(Icon::Plus, component::ICON, p.accent_strong))
            .child(self.quick_add.clone());
        let field = if typed {
            field
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(component::CHIP_GAP))
                        .px(px(space::S2))
                        .h(px(component::CHIP + 2.))
                        .rounded(px(radius::SM))
                        .bg(p.wash)
                        .text_color(p.accent_strong)
                        .style(ty::COUNT.with_weight(500))
                        .child(kit::icon(
                            Icon::Calendar,
                            component::ICON_SMALL,
                            p.accent_strong,
                        ))
                        .child("Sun 22 Nov"),
                )
                .child(kit::kbd(p, "Enter"))
        } else {
            field
                .child(kit::icon(Icon::Calendar, component::GLYPH, p.ink_3))
                .child(kit::icon(Icon::Flag, component::GLYPH, p.ink_3))
                .child(kit::kbd(p, "N"))
        };
        // The focus ring is a band of the room tint around the field (a border, not a shadow).
        let ring = if focused || typed {
            p.tint
        } else {
            gpui::transparent_black()
        };
        div()
            .mx(px(desktop::LIST_PAD_X - elevation::FOCUS_RING))
            .my(px(desktop::QUICK_ADD_MARGIN - elevation::FOCUS_RING))
            .p(px(elevation::FOCUS_RING))
            .rounded(px(radius::QUICK_ADD + elevation::FOCUS_RING))
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
            .gap(px(space::S2))
            .h(px(component::CHIP))
            .mt(px(desktop::ROW_LINE_GAP))
            .overflow_hidden();
        if let Some(list) = &task.list {
            meta = meta.child(kit::chip(t, list, self.shell.room.smart));
        }
        if let Some((d, n)) = task.steps {
            meta = meta.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(component::CHIP_GAP))
                    .text_color(p.ink_3)
                    .style(ty::COUNT.with_weight(500))
                    .child(kit::meter(p, d, n))
                    .child(format!("{d}/{n}")),
            );
        }
        for tag in task.tags {
            meta = meta.child(div().style(ty::TAG).text_color(p.ink_3).child(*tag));
        }
        if task.from_mail {
            meta = meta.child(kit::icon(Icon::Mail, component::ICON_SMALL, p.ink_3));
        }
        let due: AnyElement = match &task.due {
            Due::None => div()
                .style(ty::DUE)
                .text_color(p.ink_4)
                .child("–")
                .into_any_element(),
            Due::Soon(s) => div()
                .style(ty::DUE.with_weight(600))
                .text_color(p.accent_strong)
                .child(*s)
                .into_any_element(),
            Due::Plain(s) => div()
                .style(ty::DUE)
                .text_color(p.ink_2)
                .child(*s)
                .into_any_element(),
            Due::Overdue(s) => kit::overdue_badge(p, s).into_any_element(),
        };
        let row = div()
            .relative()
            .h(px(desktop::ROW))
            .flex_none()
            .flex()
            .items_start()
            .gap(px(desktop::ROW_GAP))
            .pt(px(desktop::ROW_PAD_TOP))
            .px(px(desktop::LIST_PAD_X))
            .border_b_1()
            .border_color(p.line)
            .child(kit::checkbox(p, overdue, desktop::COL_CHECK))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .style(ty::TASK_TITLE)
                            .text_color(p.ink)
                            .truncate()
                            .child(task.title.clone()),
                    )
                    .child(meta),
            )
            .child(
                div().w(px(desktop::COL_AGENT)).flex_none().children(
                    task.agent
                        .map(|a| kit::agent_mark(t, a, component::AGENT_MARK)),
                ),
            )
            .child(
                div()
                    .w(px(desktop::COL_PRIORITY))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(kit::priority(p, task.priority)),
            )
            .child(
                div()
                    .w(px(desktop::COL_DUE))
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
                    .top(px(-elevation::HAIRLINE))
                    .bottom(px(-elevation::HAIRLINE))
                    .w(px(elevation::SELECTION_BAR))
                    .bg(p.accent),
            )
        } else {
            row
        }
    }

    fn list(&self) -> Div {
        let p = &self.palette;
        let room = &self.shell.room;
        let body = if let (true, Some(next)) = (self.finished_day(), &room.tomorrow) {
            self.tomorrow(next)
        } else if room.tasks.is_empty() {
            div()
                .px(px(desktop::LIST_PAD_X))
                .pt(px(space::S6 + space::S1))
                .child(
                    div()
                        .style(ty::EMPTY_LINE)
                        .text_color(p.ink)
                        .child(room.empty_line),
                )
                .child(
                    div()
                        .mt(px(space::S2 + 2.))
                        .style(ty::BODY)
                        .text_color(p.ink_3)
                        .child("Add one above, or press Ctrl+Alt+Space from anywhere."),
                )
        } else {
            div()
                .flex()
                .flex_col()
                .children(room.tasks.iter().enumerate().map(|(i, t)| self.row(i, t)))
        };
        // The reserved strip at the bottom: "Completed n" on the left, room for the undo toast on the right.
        let completed = (room.completed > 0).then(|| {
            div()
                .h(px(desktop::TOAST_INSET))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(desktop::LIST_PAD_X))
                .border_t_1()
                .border_color(p.line)
                .child(kit::icon(
                    Icon::ChevronRight,
                    component::ICON_SMALL,
                    p.ink_3,
                ))
                .child(kit::label(p, "Completed"))
                .child(
                    div()
                        .style(ty::CAPS_LABEL)
                        .text_color(p.ink_3)
                        .child(room.completed.to_string()),
                )
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(div().h(px(elevation::HAIRLINE)).bg(p.line))
            .child(div().flex_1().min_h_0().overflow_hidden().child(body))
            .children(completed)
    }

    /// The finished day hands over to tomorrow: its first task with a sunrise mark and a button to plan it,
    /// then the rest of tomorrow and the rest of the week. No congratulations; the scene is the reward.
    fn tomorrow(&self, next: &Tomorrow) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let section = |label: SharedString| {
            div()
                .px(px(desktop::LIST_PAD_X))
                .pt(px(space::S6))
                .pb(px(space::S2))
                .text_color(p.ink_3)
                .style(ty::CAPS_LABEL)
                .child(label.to_uppercase())
        };
        let compact = |task: &Task| {
            let when = match &task.due {
                Due::Soon(s) | Due::Plain(s) | Due::Overdue(s) => *s,
                Due::None => "",
            };
            div()
                .mx(px(desktop::LIST_PAD_X))
                .h(px(component::ICON_BUTTON + space::S1))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .border_t_1()
                .border_color(p.line)
                .child(kit::checkbox(
                    p,
                    false,
                    component::CHECKBOX - space::S1 / 2.,
                ))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .style(ty::BODY)
                        .text_color(p.ink)
                        .child(task.title.clone()),
                )
                .children(task.list.map(|l| kit::chip(t, &l, false)))
                .child(div().flex_1())
                .child(div().style(ty::DUE).text_color(p.ink_2).child(when))
        };
        let first = &next.first;
        let mark = div()
            .size(px(desktop::QUICK_ADD + space::S2))
            .flex_none()
            .rounded_full()
            .bg(p.wash)
            .flex()
            .items_center()
            .justify_center()
            .child(kit::icon(
                Icon::Sunrise,
                component::ICON + space::S1,
                p.accent_strong,
            ));
        let card = div()
            .mx(px(desktop::LIST_PAD_X))
            .flex()
            .items_center()
            .gap(px(space::S4))
            .child(mark)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .child(
                        div()
                            .style(ty::NEXT_TITLE)
                            .text_color(p.ink)
                            .truncate()
                            .child(first.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                div()
                                    .style(ty::DUE.with_weight(600))
                                    .text_color(p.accent_strong)
                                    .child(next.first_when),
                            )
                            .children(first.list.map(|l| kit::chip(t, &l, true))),
                    ),
            )
            .child(
                div()
                    .h(px(component::BUTTON))
                    .px(px(space::S4))
                    .flex()
                    .items_center()
                    .rounded(px(radius::BUTTON))
                    .bg(p.accent_fill)
                    .text_color(p.on_accent)
                    .style(ty::BUTTON)
                    .child(format!("Plan {}", next.weekday)),
            );
        div()
            .flex()
            .flex_col()
            .child(section("When the sun comes up".into()))
            .child(card)
            .child(section(format!("Also on {}", next.weekday).into()))
            .children(next.also.iter().map(compact))
            .child(section("Rest of the week".into()))
            .children(next.week.iter().map(compact))
            .child(
                div()
                    .px(px(desktop::LIST_PAD_X))
                    .pt(px(space::S3))
                    .style(ty::COUNT)
                    .text_color(p.ink_3)
                    .child(format!("{} more", next.more)),
            )
    }

    fn detail(&self, d: &Detail) -> Div {
        let p = &self.palette;
        let t = self.theme;
        let steps_done = d.steps.iter().filter(|s| s.done).count() as u32;
        let steps_total = d.steps.len() as u32;
        let step_row = |i: usize, s: &crate::fixtures::Step| {
            let selected = i == steps_done as usize;
            let mark = if s.done {
                kit::checked(p, selected, component::GLYPH)
            } else {
                kit::checkbox(p, false, component::GLYPH)
            };
            let title = div().flex_1().truncate().style(ty::STEP);
            let title = if s.done {
                title.text_color(p.ink_3).line_through()
            } else {
                title.text_color(p.ink)
            };
            let row = div()
                .relative()
                .h(px(desktop::STEP_ROW))
                .flex()
                .items_center()
                .gap(px(space::S3))
                .px(px(desktop::DETAIL_PAD_X))
                .border_b_1()
                .border_color(p.line)
                .child(mark)
                .child(title.child(s.title))
                .child(
                    div()
                        .style(ty::COUNT)
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
                        .w(px(elevation::SELECTION_BAR))
                        .bg(p.accent),
                )
            } else {
                row
            }
        };
        let section = |label: &'static str| {
            div()
                .px(px(desktop::DETAIL_PAD_X))
                .pt(px(space::S5))
                .pb(px(space::S2))
                .child(kit::label(p, label))
        };
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
                    .pr(px(space::S4))
                    .border_b_1()
                    .border_color(p.line)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .style(ty::SUMMARY)
                            .text_color(p.ink_2)
                            .child(kit::chip(
                                t,
                                &ListRef {
                                    name: d.list.collection,
                                    ..d.list
                                },
                                false,
                            ))
                            .child(div().text_color(p.ink_4).child("/"))
                            .child(
                                div()
                                    .text_color(p.ink)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(d.list.name),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(space::S3 + 2.))
                            .child(kit::icon(Icon::Star, component::GLYPH, p.ink_3))
                            .child(kit::icon(Icon::Ellipsis, component::GLYPH, p.ink_3))
                            .child(kit::icon(Icon::X, component::GLYPH, p.ink_3)),
                    ),
            )
            .child(
                div()
                    .px(px(desktop::DETAIL_PAD_X))
                    .pt(px(space::S5))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S3))
                            .child(kit::checkbox(p, false, component::CHECKBOX + 2.))
                            .child(
                                div()
                                    .style(ty::DETAIL_TITLE)
                                    .text_color(p.ink)
                                    .child(d.title),
                            ),
                    )
                    .child(
                        div()
                            .ml(px(component::CHECKBOX + 2. + space::S3))
                            .mt(px(space::S1))
                            .style(ty::COUNT)
                            .text_color(p.ink_3)
                            .child(d.subtitle),
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
                    .child(kit::label(p, "Steps"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(kit::meter(p, steps_done, steps_total))
                            .child(
                                div()
                                    .style(ty::TAG)
                                    .text_color(p.ink_2)
                                    .child(format!("{steps_done}/{steps_total}")),
                            ),
                    ),
            )
            .child(
                div()
                    .h(px(elevation::HAIRLINE))
                    .mx(px(desktop::DETAIL_PAD_X))
                    .bg(p.line),
            )
            .children(d.steps.iter().enumerate().map(|(i, s)| step_row(i, s)))
            .child(
                div()
                    .h(px(desktop::STEP_ROW))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .px(px(desktop::DETAIL_PAD_X))
                    .style(ty::STEP)
                    .text_color(p.ink_3)
                    .child(kit::icon(Icon::Plus, component::GLYPH, p.ink_3))
                    .child(div().flex_1().child("Add step"))
                    .child(kit::kbd(p, "Tab")),
            )
            .child(section("Notes"))
            .child(
                div()
                    .px(px(desktop::DETAIL_PAD_X))
                    .style(ty::NOTES)
                    .text_color(p.ink_2)
                    .child(d.notes),
            )
            .child(
                div()
                    .mt(px(space::S4))
                    .mx(px(desktop::DETAIL_PAD_X))
                    .border_t_1()
                    .border_color(p.line)
                    .children(d.fields.iter().map(|(icon, label, value)| {
                        div()
                            .h(px(desktop::PROP_ROW))
                            .flex()
                            .items_center()
                            .gap(px(space::S2 + 2.))
                            .border_b_1()
                            .border_color(p.line)
                            .child(kit::icon(*icon, component::ICON_SMALL, p.ink_3))
                            .child(
                                div()
                                    .w(px(space::S10 * 2.))
                                    .style(ty::NAV_COLLECTION.with_weight(400))
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
            .w(px(desktop::DETAIL))
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

    fn status_bar(&self) -> Div {
        let p = &self.palette;
        let (done, _, open) = crate::fixtures::summary(&self.shell.room);
        let hint = |key: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .gap(px(component::CHIP_GAP))
                .child(kit::kbd(p, key))
                .child(what)
        };
        let strong = |s: String| {
            div()
                .text_color(p.ink_2)
                .font_weight(FontWeight::MEDIUM)
                .child(s)
        };
        div()
            .h(px(desktop::STATUSBAR))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(space::S4))
            .border_t_1()
            .border_color(p.line)
            .bg(p.canvas)
            .style(ty::COUNT)
            .text_color(p.ink_3)
            .child(
                div()
                    .flex()
                    .gap(px(space::S4))
                    .child(
                        div()
                            .flex()
                            .gap(px(component::CHIP_GAP))
                            .child(strong(open.to_string()))
                            .child("open"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(component::CHIP_GAP))
                            .child(strong(format!("{done:02}")))
                            .child("done"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(space::S4 + 2.))
                    .style(ty::META)
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
            .style(ty::BODY)
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
                            // The finished day hands over to tomorrow instead of taking new tasks.
                            .when(!self.finished_day(), |pane| {
                                pane.child(self.quick_add(window, cx))
                            })
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
        v.style(ty::TAG).text_color(p.ink_2)
    } else if value.chars().any(|c| c.is_ascii_digit()) {
        v.style(ty::DUE.with_weight(600)).text_color(p.ink)
    } else {
        v.style(ty::NAV_COLLECTION).text_color(p.ink)
    }
}
