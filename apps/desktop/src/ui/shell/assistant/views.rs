//! Desktop assistant presentation. Cards use trusted simulated effects, never model-authored UI.

mod cloud;
mod markdown;

use super::*;
use crate::theme::Agent;
use crate::ui::kit::QuietButton as _;
use gpui::Role;
use std::sync::Arc;
use todo_client::assistant::{
    Edit, Effect, ListAccess, ProposalItem, ProposalKind, StepKind, StepStatus, TaskRef, ToolKind,
    VISIBLE_TASK_LINKS,
};
use todo_core::{
    State,
    command::Placement,
    ids::{CollectionId, ListId},
    model::WorkflowStatus,
    time::Date,
};

#[derive(Clone, Copy)]
enum AccessTarget {
    List(ListId),
    Collection(CollectionId),
}

fn access_label(access: Option<ListAccess>) -> &'static str {
    match access {
        None => "Mixed",
        Some(ListAccess::Hidden) => "Hidden",
        Some(ListAccess::PrivateOnly) => "Private only",
        Some(ListAccess::AllModels) => "All models",
    }
}

#[derive(Clone, Copy)]
enum Tone {
    Quiet,
    Outline,
    Primary,
    Danger,
}

fn text(style: ty::TextStyle, color: Hsla, content: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .style(style)
        .text_color(color)
        .child(content.into())
}

fn one_line(el: Div) -> Div {
    el.overflow_hidden().text_ellipsis().whitespace_nowrap()
}

fn box_panel(p: &Palette) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .bg(p.raised)
        .border_1()
        .border_color(p.line)
        .rounded(px(radius::LG))
        .overflow_hidden()
}

fn line(p: &Palette) -> Div {
    div()
        .h(px(elevation::HAIRLINE))
        .flex_none()
        .w_full()
        .bg(p.line)
}

fn proposal_kind(kind: ProposalKind, p: &Palette) -> (Icon, &'static str, Hsla) {
    match kind {
        ProposalKind::Create => (Icon::Plus, "NEW TASK", p.accent_strong),
        ProposalKind::Edit => (Icon::Pencil, "EDIT TASK", p.sun_ink),
        ProposalKind::Complete => (Icon::CircleCheck, "CHANGE COMPLETION", p.sun_ink),
        ProposalKind::Destructive => (Icon::TrashTwo, "DELETE TASK", p.danger_ink),
    }
}

fn step_label(kind: StepKind) -> &'static str {
    match kind {
        StepKind::Generate => "Waiting for the model",
        StepKind::Tool(ToolKind::Lists) => "Checked permitted lists",
        StepKind::Tool(ToolKind::Search) => "Searched visible tasks",
        StepKind::Tool(ToolKind::Task) => "Read task details",
        StepKind::Tool(ToolKind::Create) => "Prepared a new task",
        StepKind::Tool(ToolKind::Update) => "Prepared task edits",
        StepKind::Tool(ToolKind::Complete) => "Prepared a completion change",
    }
}

fn completion(done: bool) -> String {
    if done { "Completed" } else { "Open" }.into()
}

impl ShellView {
    pub(super) fn ai_date(&self, date: Option<Date>) -> String {
        date.map_or_else(|| "No date".into(), |d| self.store.now().dates().weekday(d))
    }

    fn ai_action(
        &self,
        id: impl Into<SharedString>,
        contents: Div,
        enabled: bool,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.ai_clickable(id, contents, enabled, f, cx)
            .when(enabled, |row| row.hover(|s| s.opacity(0.85)))
    }

    fn ai_clickable(
        &self,
        id: impl Into<SharedString>,
        contents: Div,
        enabled: bool,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let id = id.into();
        let p = self.palette;
        let element = contents.id(id.clone()).role(Role::Button);
        if !enabled {
            return element;
        }
        element
            .track_focus(&self.ai.focus_handle(&id, cx))
            .focus_visible(|d| d.border_1().border_color(p.accent_strong))
            .cursor_pointer()
            // GPUI maps a focused Enter/Space press-release to this same click. Handling
            // KeyDown separately would toggle controls twice for one gesture.
            .on_click(cx.listener(move |this, _, window, cx| f(this, window, cx)))
    }

    fn ai_quiet_action(
        &self,
        id: impl Into<SharedString>,
        contents: Div,
        enabled: bool,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.ai_clickable(id, contents.rounded(px(radius::MD)), enabled, f, cx)
            .when(enabled, |d| d.quiet_button(self.palette))
    }

    fn ai_button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        tone: Tone,
        enabled: bool,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let label = label.into();
        let (bg, fg, border) = match tone {
            Tone::Quiet => (
                gpui::transparent_black(),
                if enabled { p.accent_strong } else { p.ink_4 },
                gpui::transparent_black(),
            ),
            Tone::Outline => (p.raised, p.ink_2, p.line_strong),
            Tone::Primary if enabled => (p.accent_fill, p.on_accent, p.accent_fill),
            Tone::Primary => (p.canvas, p.ink_3, p.line),
            Tone::Danger => (p.danger_bg, p.danger_ink, p.danger),
        };
        self.ai_clickable(
            id,
            div()
                .h(px(component::BUTTON_SMALL))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .rounded(px(radius::MD))
                .border_color(border)
                .px(px(space::S2))
                .bg(bg)
                .style(ty::BUTTON_SMALL)
                .text_color(fg)
                .child(label.clone()),
            enabled,
            f,
            cx,
        )
        .when(enabled && matches!(tone, Tone::Quiet), |d| {
            d.quiet_button(p)
        })
        .when(enabled && !matches!(tone, Tone::Quiet), |d| {
            d.hover(|s| s.opacity(0.85))
        })
        .aria_label(label)
    }

    fn ai_icon(
        &self,
        id: &'static str,
        icon: Icon,
        label: &'static str,
        enabled: bool,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        self.ai_quiet_action(
            id,
            kit::toolbar_icon()
                .relative()
                .border_1()
                .border_color(gpui::transparent_black())
                .when(id == "assistant-settings", |el| el.child(self.probe(id)))
                .child(kit::icon(
                    icon,
                    component::ICON,
                    if enabled { p.ink_3 } else { p.ink_4 },
                )),
            enabled,
            f,
            cx,
        )
        .aria_label(label)
        .tooltip(kit::tooltip(p, label))
    }

    pub(in crate::ui::shell) fn ai_trigger(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = self.palette;
        let pending = !self.ai.open && matches!(self.ai.view().composer, Composer::Review { .. });
        self.ai_action(
            "assistant-trigger",
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .mx(px(space::S3))
                .px(px(space::S2))
                .h(px(desktop::TITLEBAR - space::S2 - space::S1 / 2.))
                .rounded(px(radius::MD))
                .border_1()
                .bg(if self.ai.open {
                    p.surface_wash()
                } else {
                    p.surface_control()
                })
                .border_color(if self.ai.open {
                    p.accent
                } else {
                    p.line_strong
                })
                .child(kit::icon(
                    Icon::Sparkles,
                    component::ICON_SMALL,
                    p.accent_strong,
                ))
                .child(text(
                    ty::BUTTON_SMALL,
                    if self.ai.open { p.accent_strong } else { p.ink },
                    "Ask AI",
                ))
                .when(pending, |b| b.child(text(ty::COUNT, p.accent_strong, "1")))
                .when(self.width == Width::Wide, |b| {
                    b.child(kit::kbd(&p, "Ctrl J"))
                }),
            true,
            |this, window, cx| this.toggle_ai(window, cx),
            cx,
        )
        .aria_label(if pending {
            "Ask AI · proposal waiting for review"
        } else {
            "Ask AI"
        })
        .aria_toggled(toggled(self.ai.open))
    }

    pub(in crate::ui::shell) fn ai_width_changed(&mut self, window: &Window) {
        let before = self.ai_shares_detail();
        self.ai.viewport = f32::from(window.viewport_size().width);
        if before != self.ai_shares_detail() {
            self.settle_detail();
        }
    }

    pub(in crate::ui::shell) fn ai_pane(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.ai.open {
            return None;
        }
        let p = self.palette;
        let width = self.columns(window).width(Pane::Ai);
        let pane = self.ai_panel(width, window, cx).into_any_element();
        Some(if self.ai_docked() {
            pane
        } else {
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .left(px(self.columns(window).width(Pane::Sidebar)))
                .child(
                    div()
                        .id("assistant-overlay-scrim")
                        .absolute()
                        .inset_0()
                        .bg(p.scrim)
                        .occlude()
                        .on_click(cx.listener(|this, _, window, cx| this.close_ai(window, cx))),
                )
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .shadow(kit::overlay_shadow(&p))
                        .child(pane),
                )
                .into_any_element()
        })
    }

    fn ai_panel(&self, width: f32, window: &Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = self.palette;
        let view = self.ai.view();
        let title = view.turns.first().map_or_else(
            || "New chat".into(),
            |turn| model::labels::one_line(&turn.prompt),
        );
        let is_empty = view.turns.is_empty();
        let pending = match view.composer {
            Composer::Review {
                selected, total, ..
            } => Some((if self.ai.review.zero { 0 } else { selected }, total)),
            _ => None,
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .h(px(desktop::DETAIL_TOP))
            .px(px(space::S4))
            .gap(px(space::S2))
            .border_b_1()
            .border_color(p.line)
            .child(kit::agent_mark(
                self.theme,
                Agent::Assistant,
                component::AGENT_MARK,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(one_line(text(ty::NAV_COLLECTION, p.ink, title)))
                    .child(text(
                        ty::META,
                        if self.ai.blocked() {
                            p.danger_ink
                        } else {
                            p.ink_3
                        },
                        self.ai.route_label(),
                    )),
            )
            .child(self.ai_icon(
                "assistant-new",
                Icon::Plus,
                "New chat",
                !is_empty,
                |this, window, cx| this.new_ai_chat(window, cx),
                cx,
            ))
            .child(self.ai_icon(
                "assistant-settings",
                Icon::Settings,
                "AI settings",
                true,
                |this, window, cx| this.open_ai_settings(window, cx),
                cx,
            ))
            .child(self.ai_icon(
                "assistant-close",
                Icon::X,
                "Close assistant",
                true,
                |this, window, cx| this.close_ai(window, cx),
                cx,
            ));
        // Both the empty state and turns use `space::S4` (16 px >= SCROLLBAR_GUTTER) padding.
        let mut feed = self.ai.feed.viewport("assistant-feed").size_full();
        if is_empty {
            feed = feed.child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .child(self.ai_empty(&view, cx)),
            );
        } else {
            feed = feed.child(
                div()
                    .p(px(space::S4))
                    .flex()
                    .flex_col()
                    .gap(px(space::S4))
                    .children(
                        view.turns
                            .iter()
                            .enumerate()
                            .map(|(i, turn)| self.ai_turn(turn, i, i + 1 == view.turns.len(), cx)),
                    ),
            );
        }
        let banner = pending.map(|(selected, total)| {
            div()
                .h(px(component::ICON_BUTTON))
                .px(px(space::S3))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .flex_none()
                .bg(p.wash)
                .border_y_1()
                .border_color(p.line)
                .child(kit::icon(
                    Icon::CircleAlert,
                    component::ICON_SMALL,
                    p.accent_strong,
                ))
                .child(
                    text(
                        ty::META,
                        p.ink,
                        if total == 1 {
                            "1 proposal waiting for review".into()
                        } else {
                            format!("{selected} of {total} changes selected")
                        },
                    )
                    .flex_1(),
                )
                .child(self.ai_quiet_action(
                    "assistant-jump",
                    text(ty::BUTTON_SMALL, p.accent_strong, "Jump up"),
                    true,
                    |this, _, cx| {
                        this.ai.feed.handle.scroll_to_bottom();
                        cx.notify();
                    },
                    cx,
                ))
        });
        div()
            .id("assistant-panel")
            .track_focus(&self.ai.panel)
            .tab_group()
            .aria_label("Ask AI")
            .w(px(width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .min_h_0()
            .border_l_1()
            .border_color(p.line)
            .bg(if self.ai_docked() {
                p.surface_pane()
            } else {
                p.pane
            })
            .block_mouse_except_scroll()
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                if super::super::navigation_tab(e) {
                    if e.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                } else if !e.is_held
                    && !e.keystroke.modifiers.modified()
                    && e.keystroke.key == "escape"
                {
                    this.escape_ai(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(header)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(feed)
                    .child(
                        self.ai
                            .feed
                            .bar
                            .overlay(&self.ai.feed.handle, &p, self.animate),
                    ),
            )
            .children(self.ai.notice.as_ref().map(|notice| {
                text(ty::META, p.danger_ink, notice.clone())
                    .bg(p.danger_bg)
                    .px(px(space::S3))
                    .py(px(space::S2))
            }))
            .children(banner)
            .child(self.ai_composer(&view, window, cx))
    }

    fn ai_empty(&self, view: &AssistantView, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let blocked = self.ai.blocked();
        let ready = view.model.is_some() && !blocked;
        let examples = [
            (
                Icon::Calendar,
                "What’s due this week?",
                "What's due this week?",
            ),
            (
                Icon::List,
                "Summarize my open tasks",
                "Summarize my open tasks.",
            ),
            (
                Icon::Flag,
                "Review my priorities",
                "Help me review my priorities: ",
            ),
            (
                Icon::Plus,
                "Turn a rough note into tasks…",
                "Turn this note into tasks: ",
            ),
        ];
        let mut result = div().flex().flex_col().gap(px(space::S3)).p(px(space::S4)).pb(px(space::S5))
            .child(div().size(px(component::BUTTON)).flex().items_center().justify_center().bg(p.wash).rounded(px(radius::LG)).border_1().border_color(p.line)
                .child(kit::icon(Icon::Sparkles, component::ICON, p.accent_strong)))
            .child(text(ty::DETAIL_TITLE, p.ink, if blocked { "Connection blocked" } else if ready { "What can I help plan?" } else { "Connect a model" }))
            .child(text(ty::NOTES, p.ink_2, if blocked { NOT_PRIVATE } else if ready {
                "Ask about your tasks, or describe changes to make. Every edit or new task waits for your approval first."
            } else if self.ai.profile.cloud {
                "Sign in to ChatGPT and choose a model in AI settings. Your chat won't be sent until you approve cloud sharing."
            } else { "Choose a model to start. Lists are hidden until you allow access." }));
        if ready {
            result =
                result.child(
                    div()
                        .mt(px(space::S3))
                        .flex()
                        .flex_col()
                        .gap(px(space::S2))
                        .children(examples.into_iter().enumerate().map(
                            |(i, (icon, label, value))| {
                                self.ai_action(
                                    format!("assistant-suggestion-{i}"),
                                    div()
                                        .w_full()
                                        .min_h(px(component::BUTTON))
                                        .px(px(space::S3))
                                        .flex()
                                        .items_center()
                                        .gap(px(space::S2))
                                        .border_1()
                                        .border_color(p.line)
                                        .rounded(px(radius::MD))
                                        .bg(p.raised)
                                        .child(kit::icon(
                                            icon,
                                            component::ICON_SMALL,
                                            p.accent_strong,
                                        ))
                                        .child(text(ty::BODY, p.ink, label).flex_1())
                                        .child(kit::icon(Icon::ArrowUpRight, space::S3, p.ink_3)),
                                    true,
                                    move |this, window, cx| {
                                        this.ai.composer.update(cx, |a, cx| a.set_text(value, cx));
                                        this.focus_ai(window, cx);
                                        cx.notify();
                                    },
                                    cx,
                                )
                            },
                        )),
                );
        } else if blocked {
            result = result.child(
                self.ai_button(
                    "assistant-connect",
                    "Open AI settings",
                    Tone::Primary,
                    true,
                    |this, w, cx| this.open_ai_settings(w, cx),
                    cx,
                )
                .self_start(),
            );
        } else {
            use todo_client::assistant::native_relay::Hosted;
            let choices = [
                (
                    "assistant-connect-chatgpt",
                    Icon::Sparkles,
                    "ChatGPT",
                    Some(Hosted::ChatGpt(Uuid::nil())),
                    self.ai.cloud.available(),
                ),
                (
                    "assistant-connect-private",
                    Icon::Settings,
                    "Your private models",
                    Some(Hosted::Private),
                    self.ai
                        .cloud
                        .status
                        .as_ref()
                        .is_some_and(|s| s.private_models),
                ),
                ("assistant-connect", Icon::Link, "This device", None, true),
            ];
            for (id, glyph, label, provider, available) in choices {
                if available {
                    result = result.child(
                        self.ai_action(
                            id,
                            div()
                                .w_full()
                                .min_h(px(component::BUTTON))
                                .px(px(space::S3))
                                .flex()
                                .items_center()
                                .gap(px(space::S2))
                                .border_1()
                                .border_color(p.line)
                                .rounded(px(radius::MD))
                                .bg(p.raised)
                                .child(kit::icon(glyph, component::ICON_SMALL, p.accent_strong))
                                .child(one_line(text(ty::BODY, p.ink, label)).flex_1())
                                .child(kit::icon(Icon::ArrowUpRight, space::S3, p.ink_3)),
                            true,
                            move |this, w, cx| {
                                this.open_ai_settings(w, cx);
                                this.choose_ai_provider(provider, cx);
                                if provider.is_some() {
                                    w.focus(&this.ai.dialog, cx);
                                }
                            },
                            cx,
                        )
                        .aria_label(label),
                    );
                }
            }
        }
        result
    }

    fn ai_composer(&self, view: &AssistantView, window: &Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let streaming = matches!(view.composer, Composer::Streaming { .. });
        let blocked = self.ai.blocked();
        let ready = !blocked && view.composer != Composer::ChooseModel;
        let body = self.ai.composer.read(cx);
        let focused = body.focus_handle.is_focused(window);
        let paused = self.ai.profile.cloud && self.ai.cloud.usage_paused;
        let can_send = streaming
            || (ready
                && !paused
                && !self.ai.cloud.sending
                && !body.content.trim().is_empty()
                && self.retry_seconds() == 0);
        let placeholder = match view.composer {
            _ if blocked => "Confirm your server in AI settings",
            Composer::ChooseModel => "Choose a model to start",
            Composer::Review { .. } => "Approve above, or reply to adjust…",
            _ => "Ask anything, or propose a change…",
        };
        if body.placeholder.as_ref() != placeholder {
            self.ai.composer.update(cx, |a, cx| {
                a.placeholder = placeholder.into();
                cx.notify();
            });
        }
        let send = self
            .ai_action(
                "assistant-send",
                div()
                    .size(px(component::BUTTON_SMALL))
                    .flex_none()
                    .rounded(px(radius::MD))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(if streaming { p.line_strong } else { p.line })
                    .bg(if can_send && !streaming {
                        p.accent_fill
                    } else if streaming {
                        p.raised
                    } else {
                        p.canvas
                    })
                    .child(kit::icon(
                        if streaming {
                            Icon::Square
                        } else {
                            Icon::ArrowUp
                        },
                        component::ICON_SMALL,
                        if streaming {
                            p.ink
                        } else if can_send {
                            p.on_accent
                        } else {
                            p.ink_3
                        },
                    )),
                can_send,
                |this, _, cx| {
                    if matches!(this.ai.view().composer, Composer::Streaming { .. }) {
                        this.ai.controller.cancel();
                    } else {
                        this.send_ai_input(cx);
                    }
                },
                cx,
            )
            .aria_label(if streaming {
                "Stop reply"
            } else {
                "Send message"
            });
        let hidden = if self.ai.fixture.is_some() {
            view.turns.last().map_or(1, |t| t.summary.private_lists)
        } else {
            let state = self.ai_access_state();
            state
                .lists
                .keys()
                .filter(|&&id| {
                    state.is_live(id.into())
                        && !self.ai.profile.access.list(&state, id).allows(
                            if self.ai.profile.cloud {
                                todo_client::assistant::provider::DataClass::Cloud
                            } else {
                                todo_client::assistant::provider::DataClass::Local
                            },
                        )
                })
                .count()
        };
        let model = view.model.as_ref().map_or_else(
            || "No model enabled".into(),
            |m| model::labels::one_line(&m.model),
        );
        let (light, route, route_ink, model_ink) = match self.ai.connection_health() {
            ConnectionHealth::Unknown => (None, self.ai.route_label(), p.ink_2, p.ink_2),
            ConnectionHealth::Testing => (Some(p.accent), "Testing…", p.ink_2, p.ink_2),
            ConnectionHealth::Connected => {
                (Some(p.success), self.ai.route_label(), p.ink_2, p.ink_2)
            }
            ConnectionHealth::Unreachable => {
                (Some(p.danger), "Unreachable", p.danger_ink, p.danger_ink)
            }
        };
        let privacy = self
            .ai_quiet_action(
                "assistant-route",
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .min_w_0()
                    .children(light.map(|color| kit::status_light(color, ty::META)))
                    .child(text(ty::META, route_ink, route).flex_none())
                    .child(one_line(text(ty::COUNT, model_ink, format!("· {model}"))))
                    .child(kit::icon(Icon::ChevronDown, space::S3, p.ink_3)),
                true,
                |this, w, cx| this.open_ai_settings(w, cx),
                cx,
            )
            .aria_label(format!("{route} · {model} · AI settings"));
        let visibility = self
            .ai_quiet_action(
                "assistant-privacy",
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S1))
                    .flex_none()
                    .child(kit::icon(Icon::EyeOff, space::S3, p.ink_3))
                    .child(text(
                        ty::COUNT,
                        p.ink_2,
                        format!(
                            "{hidden} {} hidden",
                            if hidden == 1 { "list" } else { "lists" }
                        ),
                    )),
                true,
                |this, w, cx| this.open_ai_settings(w, cx),
                cx,
            )
            .aria_label(format!("{hidden} lists hidden · Edit list access"));
        div()
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(space::S2))
            .px(px(space::S3))
            .py(px(space::S2))
            .border_t_1()
            .border_color(p.line)
            .child(
                div()
                    .min_h(px(desktop::AI_COMPOSER_MIN))
                    .max_h(px(desktop::AI_COMPOSER_MAX))
                    .border_1()
                    .border_color(if focused { p.accent_strong } else { p.control })
                    .rounded(px(radius::LG))
                    .bg(p.raised)
                    .pl(px(space::S3))
                    .pr(px(space::S2))
                    .py(px(space::S1))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h(px(ty::BODY.line_height))
                            .max_h(px(desktop::AI_COMPOSER_MAX - space::S5))
                            .flex()
                            .items_center()
                            .child(if ready {
                                div()
                                    .relative()
                                    .w_full()
                                    .child(
                                        self.ai
                                            .composer_scroll
                                            .viewport("assistant-compose-field")
                                            .w_full()
                                            .max_h(px(desktop::AI_COMPOSER_MAX - space::S5))
                                            .when(
                                                self.ai.composer_scroll.handle.max_offset().y
                                                    > px(0.),
                                                |d| d.pr(px(kit::SCROLLBAR_GUTTER)),
                                            )
                                            .style(ty::BODY)
                                            .child(self.ai.composer.clone()),
                                    )
                                    .child(self.ai.composer_scroll.bar.overlay(
                                        &self.ai.composer_scroll.handle,
                                        &p,
                                        self.animate,
                                    ))
                                    .into_any_element()
                            } else {
                                text(ty::BODY, p.ink_3, placeholder).into_any_element()
                            }),
                    )
                    .child(send),
            )
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .gap(px(space::S2))
                    .child(privacy)
                    .child(visibility),
            )
            .when(self.ai.profile.cloud && view.model.is_some(), |footer| {
                footer.child(
                    div()
                        .flex()
                        .justify_between()
                        .items_center()
                        .gap(px(space::S2))
                        .child(text(ty::META, p.ink_2, "Using ChatGPT plan"))
                        .child(self.ai_manage_usage(
                            "assistant-chatgpt-composer-usage",
                            Tone::Quiet,
                            cx,
                        )),
                )
            })
    }

    fn ai_turn(&self, turn: &TurnView, index: usize, last: bool, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let mut output = div().flex().flex_col().gap(px(space::S3)).child(
            div()
                .ml(px(space::S8))
                .px(px(space::S3))
                .py(px(space::S2))
                .rounded(px(radius::LG))
                .border_1()
                .border_color(p.line)
                .bg(p.wash)
                .child(text(ty::BODY, p.ink, turn.prompt.clone())),
        );
        if turn.stale_snapshot || turn.text_only {
            output = output.child(text(
                ty::META,
                p.ink_3,
                if turn.text_only {
                    "Text only · no task access for this reply"
                } else {
                    "Offline · reading the last synced copy"
                },
            ));
        }
        if !turn.steps.is_empty() {
            output = output.child(self.ai_steps(turn, index, cx));
        }
        if !turn.prose.is_empty() {
            output = output.child(markdown::reply(&turn.prose, p));
        }
        if !turn.task_links.is_empty() {
            let id = turn.id;
            let expanded = self.ai.expanded_task_links.contains(&id);
            let more = turn.task_links.len().saturating_sub(VISIBLE_TASK_LINKS);
            output = output.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(space::S1))
                    .children(
                        turn.task_links
                            .iter()
                            .take(if expanded {
                                turn.task_links.len()
                            } else {
                                VISIBLE_TASK_LINKS
                            })
                            .map(|task| self.ai_task_link(task, format!("chat-{index}"), cx)),
                    )
                    .when(more > 0, |links| {
                        links.child(
                            self.ai_button(
                                format!("assistant-task-links-{id}"),
                                if expanded {
                                    "Show fewer".into()
                                } else {
                                    format!("Show {more} more")
                                },
                                Tone::Quiet,
                                true,
                                move |this, _, cx| {
                                    if !this.ai.expanded_task_links.remove(&id) {
                                        this.ai.expanded_task_links.insert(id);
                                    }
                                    cx.notify();
                                },
                                cx,
                            )
                            .self_start()
                            .aria_expanded(expanded),
                        )
                    }),
            );
        }
        if let Some(card) = &turn.proposal {
            output = output.child(self.ai_proposal(turn, card, cx));
        }
        if let Some(error) = turn.error {
            if error == AssistantError::Cancelled {
                return output.child(text(ty::META, p.ink_3, "Reply stopped"));
            }
            let usage_limit = self.ai.profile.cloud
                && matches!(
                    error,
                    AssistantError::Provider(ProviderError::RateLimited { .. })
                );
            let message = match error {
                AssistantError::Provider(error) => route_problem(&self.ai.profile, error),
                error => error.to_string(),
            };
            output = output.child(
                box_panel(&p)
                    .relative()
                    .border_color(p.danger)
                    .p(px(space::S3))
                    .gap(px(space::S2))
                    .child(kit::leading_rail(p.danger, component::PROPOSAL_BAR))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(kit::icon(
                                Icon::CircleAlert,
                                component::ICON_SMALL,
                                p.danger_ink,
                            ))
                            .child(text(ty::CAPS_LABEL, p.danger_ink, "REPLY UNAVAILABLE")),
                    )
                    .child(text(ty::BODY, p.ink_2, message))
                    .when(last, |error| {
                        let seconds = self.retry_seconds();
                        let prompt = turn.prompt.clone();
                        error.child(
                            div()
                                .flex()
                                .gap(px(space::S2))
                                .when(usage_limit, |row| {
                                    row.child(self.ai_manage_usage(
                                        "assistant-chatgpt-limit-usage",
                                        Tone::Primary,
                                        cx,
                                    ))
                                    .child(self.ai_button(
                                        "assistant-chatgpt-resume",
                                        "Resume",
                                        Tone::Outline,
                                        self.ai.cloud.usage_paused,
                                        |this, _, cx| {
                                            this.ai.cloud.usage_paused = false;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                })
                                .when(!usage_limit, |row| {
                                    row.child(self.ai_button(
                                        "assistant-retry",
                                        if seconds > 0 {
                                            format!("Retry in {seconds}s")
                                        } else {
                                            "Retry".into()
                                        },
                                        Tone::Outline,
                                        seconds == 0 && self.ai.view().composer == Composer::Ready,
                                        move |this, w, cx| {
                                            this.focus_ai(w, cx);
                                            this.send_ai(prompt.clone(), cx);
                                        },
                                        cx,
                                    ))
                                })
                                .child(self.ai_button(
                                    "assistant-error-settings",
                                    "Settings",
                                    Tone::Quiet,
                                    true,
                                    |this, w, cx| this.open_ai_settings(w, cx),
                                    cx,
                                )),
                        )
                    }),
            );
        }
        output
    }

    fn retry_seconds(&self) -> u64 {
        self.ai.rate_until.map_or(0, |end| {
            let left = end.saturating_duration_since(Instant::now());
            left.as_secs() + u64::from(left.subsec_nanos() > 0)
        })
    }

    fn ai_steps(&self, turn: &TurnView, index: usize, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let running =
            matches!(self.ai.view().composer, Composer::Streaming { turn: id } if id == turn.id);
        let expanded = self.ai.step_details.contains(&turn.id);
        let id = turn.id;
        let summary = turn.summary;
        let rows = if running {
            &turn.steps[turn.steps.len().saturating_sub(3)..]
        } else {
            &turn.steps[..]
        };
        div()
            .flex()
            .flex_col()
            .gap(px(space::S1))
            .when(!running, |card| {
                card.child(
                    self.ai_quiet_action(
                        format!("assistant-steps-{index}"),
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S1))
                            .child(kit::icon(
                                if expanded {
                                    Icon::ChevronDown
                                } else {
                                    Icon::ChevronRight
                                },
                                space::S3,
                                p.ink_3,
                            ))
                            .child(text(ty::META, p.ink_2, summary.label())),
                        true,
                        move |this, _, cx| {
                            if !this.ai.step_details.remove(&id) {
                                this.ai.step_details.insert(id);
                            }
                            cx.notify();
                        },
                        cx,
                    )
                    .aria_expanded(expanded),
                )
            })
            .when(running || expanded, |card| {
                card.child(
                    div()
                        .pl(px(space::S2))
                        .border_l_1()
                        .border_color(p.line)
                        .flex()
                        .flex_col()
                        .gap(px(space::S1))
                        .children(rows.iter().map(|step| {
                            let (icon, ink, duration) = match step.status {
                                StepStatus::Running => {
                                    (Icon::Search, p.accent_strong, String::new())
                                }
                                StepStatus::Done { elapsed, .. } => (
                                    Icon::Check,
                                    p.success,
                                    format!("{} ms", elapsed.as_millis()),
                                ),
                                StepStatus::Failed => {
                                    (Icon::CircleAlert, p.danger_ink, String::new())
                                }
                            };
                            div()
                                .flex()
                                .items_center()
                                .gap(px(space::S2))
                                .child(kit::icon(icon, space::S3, ink))
                                .child(text(ty::META, p.ink_2, step_label(step.kind)).flex_1())
                                .child(text(ty::COUNT, p.ink_3, duration))
                        })),
                )
            })
    }

    fn ai_task_link(
        &self,
        task: &TaskRef,
        prefix: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let state = self.store.view();
        let id = task.id;
        self.ai_action(
            format!("assistant-task-{prefix}-{id}"),
            div()
                .min_h(px(component::BUTTON_SMALL))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .px(px(space::S2))
                .rounded(px(radius::MD))
                .border_1()
                .border_color(p.line)
                .bg(p.pane)
                .child(
                    one_line(text(ty::BODY, p.ink, model::labels::one_line(&task.title))).flex_1(),
                )
                .child(kit::icon(Icon::ArrowUpRight, space::S3, p.ink_3)),
            state.is_live(id.into()),
            move |this, window, cx| this.open_ai_task(id, window, cx),
            cx,
        )
        .aria_label(format!(
            "Open task {}",
            model::labels::one_line(&task.title)
        ))
    }

    fn ai_proposal(&self, turn: &TurnView, card: &ProposalCard, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let id = card.preview.id;
        let digest = card.preview.digest;
        match card.status {
            ProposalStatus::Submitted => return self.ai_receipt(card, cx),
            ProposalStatus::Declined | ProposalStatus::Superseded => {
                return box_panel(&p).p(px(space::S2)).child(text(
                    ty::META,
                    p.ink_3,
                    if card.status == ProposalStatus::Declined {
                        "Proposal declined"
                    } else {
                        "Superseded by your follow-up"
                    },
                ));
            }
            _ => {}
        }
        let stale = card.status == ProposalStatus::Stale;
        let selected = self.ai.review.indices(card);
        let batch = card.available.len() > 1;
        let kind = card.preview.kind().unwrap_or(ProposalKind::Create);
        let (icon, label, ink) = proposal_kind(kind, &p);
        let band = if kind == ProposalKind::Destructive {
            p.danger_bg
        } else {
            p.raised
        };
        let tone = if kind == ProposalKind::Destructive {
            Tone::Danger
        } else {
            Tone::Primary
        };
        let each = if stale { None } else { self.ai.review.each };
        let mut label = if batch {
            format!(
                "{} {}",
                card.available.len(),
                if card
                    .available
                    .iter()
                    .flat_map(|i| &i.effects)
                    .all(|e| e.kind() == ProposalKind::Create)
                {
                    "NEW TASKS"
                } else {
                    "CHANGES"
                }
            )
        } else {
            label.into()
        };
        if let Some(i) = each {
            label = format!("{} OF {}", i + 1, card.available.len());
        }
        let mut top = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(space::S2))
            .px(px(space::S3))
            .py(px(space::S2))
            .bg(band)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(kit::icon(icon, component::ICON_SMALL, ink))
                    .child(text(ty::CAPS_LABEL, ink, label)),
            );
        if batch && !stale {
            top = top.child(self.ai_button(
                "assistant-review-each",
                if each.is_some() {
                    "Summary"
                } else {
                    "Review each"
                },
                Tone::Outline,
                true,
                |this, _, cx| {
                    this.ai.review.each = if this.ai.review.each.is_some() {
                        None
                    } else {
                        Some(0)
                    };
                    cx.notify();
                },
                cx,
            ));
        } else if let Some(list) = card
            .preview
            .items
            .first()
            .and_then(|i| i.effects.first())
            .map(effect_task)
            .map(|t| &t.list)
        {
            top = top.child(self.ai_list_chip(list));
        }
        let mut result = box_panel(&p)
            .border_color(if kind == ProposalKind::Destructive {
                p.danger
            } else {
                p.line_strong
            })
            .child(
                div()
                    .flex()
                    .w_full()
                    .relative()
                    .pl(px(space::S2))
                    .child(kit::leading_rail(ink, component::PROPOSAL_BAR))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(top)
                            .child(line(&p))
                            .children(if !batch {
                                card.preview
                                    .items
                                    .first()
                                    .map(|item| self.ai_effects(item, &id.to_string(), cx))
                                    .into_iter()
                                    .collect::<Vec<_>>()
                            } else if let Some(i) = each {
                                card.available
                                    .get(i)
                                    .map(|item| {
                                        self.ai_review_item(turn.id, card, item, i, &selected, cx)
                                    })
                                    .into_iter()
                                    .collect()
                            } else {
                                card.available
                                    .iter()
                                    .take(
                                        if card.available.len() <= 10 || self.ai.review.show_all {
                                            usize::MAX
                                        } else {
                                            5
                                        },
                                    )
                                    .map(|item| {
                                        self.ai_batch_row(
                                            turn.id, card, item, &selected, !stale, cx,
                                        )
                                    })
                                    .collect()
                            })
                            .when(
                                batch && each.is_none() && !stale && card.available.len() > 10,
                                |c| {
                                    c.child(self.ai_button(
                                        "assistant-show-batch",
                                        if self.ai.review.show_all {
                                            "Show fewer".into()
                                        } else {
                                            format!("Show all {} changes", card.available.len())
                                        },
                                        Tone::Quiet,
                                        true,
                                        |this, _, cx| {
                                            this.ai.review.show_all = !this.ai.review.show_all;
                                            cx.notify();
                                        },
                                        cx,
                                    ))
                                },
                            )
                            .child(line(&p))
                            .child({
                                let ready = self.ai.fixture.is_some()
                                    || self.store.session().is_some_and(|s| {
                                        s.client()
                                            .assistant_snapshot()
                                            .is_some_and(|snapshot| !snapshot.stale)
                                            && s.client().status().pending == 0
                                    });
                                let confirm = self
                                    .ai_button(
                                        "assistant-approve",
                                        if batch {
                                            if selected.len() == card.available.len() {
                                                format!("Approve all ({})", selected.len())
                                            } else {
                                                format!("Approve {}", selected.len())
                                            }
                                        } else {
                                            match kind {
                                                ProposalKind::Edit => "Approve edit",
                                                ProposalKind::Destructive => "Delete",
                                                _ => "Approve",
                                            }
                                            .into()
                                        },
                                        tone,
                                        ready && !selected.is_empty() && !stale,
                                        move |this, _, cx| this.approve_ai(id, digest, cx),
                                        cx,
                                    )
                                    .into_any_element();
                                let decline = self
                                    .ai_button(
                                        "assistant-decline",
                                        if batch { "Decline all" } else { "Decline" },
                                        Tone::Outline,
                                        true,
                                        move |this, _, cx| this.decline_ai(id, digest, cx),
                                        cx,
                                    )
                                    .into_any_element();
                                let mut buttons = div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S2))
                                    .px(px(space::S3))
                                    .py(px(space::S2));
                                if stale {
                                    let prompt = turn.prompt.clone();
                                    buttons = buttons
                                        .child(self.ai_button(
                                            "assistant-refresh-proposal",
                                            "Refresh proposal",
                                            Tone::Primary,
                                            self.retry_seconds() == 0,
                                            move |this, _, cx| this.send_ai(prompt.clone(), cx),
                                            cx,
                                        ))
                                        .child(decline);
                                } else if kind == ProposalKind::Destructive {
                                    buttons = buttons.child(decline).child(confirm);
                                } else {
                                    buttons = buttons.child(confirm).child(decline);
                                }
                                buttons
                            }),
                    ),
            );
        if stale {
            result = result.child(text(ty::META, p.danger_ink, "Tasks changed since this proposal was prepared. Review a new proposal before approving.").bg(p.danger_bg).px(px(space::S3)).py(px(space::S2)));
        }
        result
    }

    fn ai_batch_row(
        &self,
        turn: Uuid,
        card: &ProposalCard,
        original: &ProposalItem,
        selected: &BTreeSet<usize>,
        live: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let index = original.index;
        let id = card.preview.id;
        let digest = card.preview.digest;
        let on = selected.contains(&index);
        let item = card
            .preview
            .items
            .iter()
            .find(|i| i.index == index)
            .unwrap_or(original);
        let task = item.effects.first().map(effect_task);
        let expanded = self.ai.review.expanded.contains(&index);
        let toggle = self
            .ai_action(
                format!("assistant-include-{index}"),
                div()
                    .size(px(space::S4))
                    .flex_none()
                    .rounded_full()
                    .border_1()
                    .border_color(if on { p.accent_fill } else { p.control })
                    .bg(if on { p.accent_fill } else { p.raised })
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(on, |b| {
                        b.child(kit::icon(Icon::Check, space::S3, p.on_accent))
                    }),
                live,
                move |this, _, cx| this.select_ai_item(turn, id, digest, index, cx),
                cx,
            )
            .aria_label(format!(
                "{} change {}",
                if on { "Exclude" } else { "Include" },
                index + 1
            ))
            .aria_toggled(toggled(on));
        let detail = self
            .ai_quiet_action(
                format!("assistant-expand-{index}"),
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .justify_between()
                    .items_start()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .min_w_0()
                            .gap(px(space::S1))
                            .child(one_line(text(
                                ty::TASK_TITLE,
                                if on { p.ink } else { p.ink_3 },
                                task.map_or_else(
                                    || "Change".into(),
                                    |t| model::labels::one_line(&t.title),
                                ),
                            )))
                            .children(task.map(|t| {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::S2))
                                    .child(self.ai_list_chip(&t.list))
                                    .child(one_line(text(ty::COUNT, p.ink_3, self.ai_date(t.due))))
                            })),
                    )
                    .child(kit::icon(
                        if expanded {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        space::S3,
                        p.ink_3,
                    )),
                live,
                move |this, _, cx| {
                    if !this.ai.review.expanded.remove(&index) {
                        this.ai.review.expanded.insert(index);
                    }
                    cx.notify();
                },
                cx,
            )
            .aria_expanded(expanded);
        div()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(p.line)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .child(toggle)
                    .child(detail),
            )
            .when(expanded, |d| {
                d.child(
                    div()
                        .mx(px(space::S3))
                        .mb(px(space::S2))
                        .bg(p.canvas)
                        .rounded(px(radius::MD))
                        .child(self.ai_batch_effects(item, index, cx)),
                )
            })
    }

    fn ai_review_item(
        &self,
        turn: Uuid,
        card: &ProposalCard,
        original: &ProposalItem,
        page: usize,
        selected: &BTreeSet<usize>,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let index = original.index;
        let id = card.preview.id;
        let digest = card.preview.digest;
        let on = selected.contains(&index);
        let item = card
            .preview
            .items
            .iter()
            .find(|i| i.index == index)
            .unwrap_or(original);
        div()
            .flex()
            .flex_col()
            .child(self.ai_effects(item, &format!("review-{index}"), cx))
            .child(
                div()
                    .px(px(space::S3))
                    .py(px(space::S2))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .child(self.ai_button(
                        "assistant-review-prev",
                        "Prev",
                        Tone::Outline,
                        page > 0,
                        move |this, _, cx| {
                            this.ai.review.each = Some(page.saturating_sub(1));
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(self.ai_button(
                        "assistant-review-next",
                        "Next",
                        Tone::Outline,
                        page + 1 < card.available.len(),
                        move |this, _, cx| {
                            this.ai.review.each = Some(page + 1);
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(self.ai_button(
                        "assistant-review-select",
                        if on { "Skip change" } else { "Include change" },
                        Tone::Quiet,
                        true,
                        move |this, _, cx| this.select_ai_item(turn, id, digest, index, cx),
                        cx,
                    ))
                    .child(text(
                        ty::COUNT,
                        p.ink_3,
                        if on { "Included" } else { "Skipped" },
                    )),
            )
    }

    fn ai_list_chip(&self, list: &todo_client::assistant::ListRef) -> Div {
        let hue = self
            .store
            .view()
            .lists
            .get(&list.id)
            .map_or(self.shell.room.hue, |l| {
                model::lists::hue(l.color.as_deref(), l.id.as_uuid().as_bytes())
            });
        let c = theme::list_colors(self.theme, hue);
        div()
            .h(px(component::CHIP))
            .max_w(px(desktop::CHIP_MAX))
            .min_w_0()
            .flex()
            .items_center()
            .px(px(component::CHIP_PAD_X))
            .rounded(px(radius::CHIP))
            .bg(c.tint)
            .child(one_line(text(
                ty::META,
                c.ink,
                model::labels::one_line(&list.name),
            )))
    }

    fn ai_batch_effects(&self, item: &ProposalItem, index: usize, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        if let [
            Effect::Created {
                task,
                priority,
                status,
                placement,
            },
        ] = item.effects.as_slice()
        {
            return div()
                .p(px(space::S2))
                .flex()
                .flex_col()
                .gap(px(space::S1))
                .child(self.ai_field("Due", text(ty::COUNT, p.ink, self.ai_date(task.due))))
                .child(self.ai_field("Priority", text(ty::COUNT, p.ink, priority.name())))
                .child(self.ai_field(
                    "Placement",
                    text(
                        ty::META,
                        p.ink_2,
                        format!("{} · {}", stage(*status), position(*placement)),
                    ),
                ));
        }
        self.ai_effects(item, &format!("batch-{index}"), cx)
    }

    fn ai_effects(&self, item: &ProposalItem, context: &str, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let mut panel = div().p(px(space::S3)).flex().flex_col().gap(px(space::S2));
        for (i, effect) in item.effects.iter().enumerate() {
            panel = match effect {
                Effect::Created {
                    task,
                    priority,
                    status,
                    placement,
                } => panel
                    .child(text(
                        ty::TASK_TITLE,
                        p.ink,
                        model::labels::one_line(&task.title),
                    ))
                    .child(self.ai_field("List", self.ai_list_chip(&task.list)))
                    .child(self.ai_field("Due", text(ty::COUNT, p.ink, self.ai_date(task.due))))
                    .child(self.ai_field("Priority", text(ty::COUNT, p.ink, priority.name())))
                    .child(self.ai_field(
                        "Status",
                        text(
                            ty::META,
                            p.ink,
                            match status {
                                WorkflowStatus::Backlog => "Backlog",
                                WorkflowStatus::Todo => "To do",
                                WorkflowStatus::InProgress => "In progress",
                            },
                        ),
                    ))
                    .child(self.ai_field(
                        "Position",
                        text(
                            ty::META,
                            p.ink,
                            match placement {
                                Placement::First => "Top of the list",
                                Placement::Last => "End of the list",
                                _ => "Relative to another task",
                            },
                        ),
                    )),
                Effect::Edited { task, changes } => panel
                    .child(self.ai_task_link(task, format!("{context}-{i}"), cx))
                    .children(changes.iter().map(|change| self.ai_diff(change))),
                Effect::Deleted { task } => panel
                    .child(self.ai_task_link(task, format!("{context}-{i}"), cx))
                    .child(text(
                        ty::META,
                        p.danger_ink,
                        "Moves to Trash. You can undo this change from Activity.",
                    )),
            };
        }
        panel
    }

    fn ai_field(&self, label: &'static str, value: Div) -> Div {
        div()
            .flex()
            .items_start()
            .gap(px(space::S2))
            .child(
                text(ty::META, self.palette.ink_3, label)
                    .w(px(space::S10 + space::S5))
                    .flex_none(),
            )
            .child(value)
    }

    fn ai_diff(&self, change: &Edit) -> Div {
        let p = self.palette;
        let (field, before, after) = match change {
            Edit::Title(c) => (
                "Title",
                model::labels::one_line(&c.old),
                model::labels::one_line(&c.new),
            ),
            Edit::Due(c) => ("Due", self.ai_date(c.old), self.ai_date(c.new)),
            Edit::StartDate(c) => ("Starts", self.ai_date(c.old), self.ai_date(c.new)),
            Edit::Priority(c) => ("Priority", c.old.name().into(), c.new.name().into()),
            Edit::Completed(c) => ("Status", completion(c.old), completion(c.new)),
        };
        let mut old = text(ty::META, p.ink_3, before);
        old.text_style().strikethrough = Some(StrikethroughStyle {
            thickness: px(elevation::HAIRLINE),
            color: Some(p.ink_3),
        });
        self.ai_field(
            field,
            div()
                .min_w_0()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(space::S1))
                .child(old)
                .child(text(ty::META, p.ink_3, "→"))
                .child(text(ty::NAV_SUB, p.ink, after)),
        )
    }

    fn ai_receipt(&self, card: &ProposalCard, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let id = card.preview.id;
        let receipt = self.ai.receipts.get(&id);
        let sync = self.store.session().map(|s| s.client());
        let mut undo = false;
        let message = match receipt.zip(sync).map(|(r, sync)| {
            (
                sync.approval_outcome(&r.approval),
                r.undo.map(|id| sync.undo_result(id)),
            )
        }) {
            Some((ApprovalOutcome::Saving, _)) => "Saving approved changes…".into(),
            Some((ApprovalOutcome::Pending, _)) => "Approved · waiting for sync".into(),
            Some((ApprovalOutcome::Confirmed { .. }, Some(None))) => {
                "Undoing approved changes…".into()
            }
            Some((ApprovalOutcome::Confirmed { .. }, Some(Some(Ok(()))))) => {
                "Approved changes undone".into()
            }
            Some((ApprovalOutcome::Confirmed { .. }, Some(Some(Err(error))))) => {
                undo = true;
                error.into()
            }
            Some((ApprovalOutcome::Confirmed { .. }, None)) => {
                undo = true;
                format!(
                    "{} confirmed · In-app assistant",
                    change_count(card.preview.items.len())
                )
            }
            Some((ApprovalOutcome::Rejected(rejection), _)) => {
                todo_client::failure::approval_message(&rejection.code).into()
            }
            _ if self.ai.fixture.is_some() => {
                undo = true;
                "Added Pick up oat milk to Errands · Just now".into()
            }
            _ => "Receipt unavailable · check Activity".into(),
        };
        box_panel(&p).bg(p.canvas).p(px(space::S2)).child(
            div()
                .flex()
                .items_center()
                .gap(px(space::S2))
                .child(kit::agent_mark(
                    self.theme,
                    Agent::Assistant,
                    component::AGENT_MARK,
                ))
                .child(text(ty::META, p.ink_2, message).flex_1())
                .when(undo, |r| {
                    r.child(self.ai_button(
                        format!("assistant-undo-{id}"),
                        "Undo",
                        Tone::Quiet,
                        true,
                        move |this, _, cx| this.undo_ai(id, cx),
                        cx,
                    ))
                }),
        )
    }

    pub(in crate::ui::shell) fn ai_settings_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let setup = self.ai.settings.as_ref()?;
        let p = self.palette;
        let running = matches!(setup.check, Check::Running(_));
        let endpoint = setup.draft.endpoint();
        let destination = endpoint.map(|info| info.destination);
        let blocked = !setup.draft.url.is_empty() && destination.is_none();
        let route = destination.map_or("No address", Destination::label);
        let allowed = !blocked && !running && self.retry_seconds() == 0;
        let (light, status_ink, connection) = match &setup.check {
            Check::NotTested if blocked => (
                None,
                p.danger_ink,
                "Only loopback URLs can be used directly.".into(),
            ),
            Check::NotTested
                if self.ai.same_destination(&setup.draft)
                    && self.ai.connection == ConnectionHealth::Connected =>
            {
                (Some(p.success), p.ink_2, "Connected".into())
            }
            Check::NotTested
                if self.ai.same_destination(&setup.draft)
                    && self.ai.connection == ConnectionHealth::Unreachable =>
            {
                (Some(p.danger), p.danger_ink, "Unreachable".into())
            }
            Check::NotTested => (None, p.ink_3, "Connection not tested".into()),
            Check::Running(label) => (Some(p.accent), p.ink_2, (*label).into()),
            Check::Catalog { count, elapsed } => (
                Some(p.success),
                p.ink_2,
                format!("Connected · {count} models · {} ms", elapsed.as_millis()),
            ),
            Check::NoCatalog => (
                Some(p.success),
                p.ink_2,
                "Model listing not supported. Enter a model ID manually.".into(),
            ),
            Check::Capable(support) => (Some(p.success), p.ink_2, support.label().into()),
            Check::Unreachable(message) => (
                Some(p.danger),
                p.danger_ink,
                format!("Unreachable · {message}"),
            ),
            Check::Failed(message) => (None, p.danger_ink, message.clone()),
        };
        let picker = setup.pick_model.then(|| self.ai_model_picker(setup, cx));
        let provider = box_panel(&p).flex_none().border_color(p.accent).child(
            div().flex().w_full().child(
                div()
                    .flex_1()
                    .min_w_0()
                    .p(px(space::S3))
                    .flex()
                    .flex_col()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(space::S2))
                            .child(text(ty::NAV_COLLECTION, p.ink, "This device"))
                            .child(text(ty::CAPS_LABEL, p.ink_3, route.to_uppercase())),
                    )
                    .child(self.ai_setting_field("Base URL", self.ai.url.clone(), None, window, cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                self.ai_setting_field(
                                    "API key",
                                    self.ai.key.clone(),
                                    None,
                                    window,
                                    cx,
                                )
                                .flex_1(),
                            )
                            .when(!setup.draft.key.is_empty(), |r| {
                                r.child(self.ai_button(
                                    "assistant-remove-key",
                                    "Remove",
                                    Tone::Quiet,
                                    true,
                                    |this, _, cx| {
                                        this.ai.key.update(cx, |i, cx| {
                                            i.set_text("", cx);
                                            i.placeholder = "Optional bearer key".into();
                                        });
                                        this.ai_setting_changed(Field::Key, cx);
                                    },
                                    cx,
                                ))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .child(
                                self.ai_setting_field(
                                    "Model",
                                    self.ai.model.clone(),
                                    Some(
                                        self.ai_icon(
                                            "assistant-catalog-toggle",
                                            Icon::ChevronDown,
                                            "Show discovered models",
                                            !setup.models.is_empty() && !running,
                                            |this, _, cx| {
                                                if let Some(setup) = this.ai.settings.as_mut() {
                                                    setup.pick_model = !setup.pick_model;
                                                    cx.notify();
                                                }
                                            },
                                            cx,
                                        )
                                        .aria_expanded(setup.pick_model)
                                        .into_any_element(),
                                    ),
                                    window,
                                    cx,
                                )
                                .flex_1(),
                            )
                            .child(self.ai_button(
                                "assistant-test",
                                "Test connection",
                                Tone::Outline,
                                allowed,
                                |this, _, cx| this.test_ai(false, cx),
                                cx,
                            )),
                    )
                    .children(picker)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(space::S2))
                            .children(light.map(|color| kit::status_light(color, ty::META)))
                            .child(text(ty::META, status_ink, connection)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(space::S2))
                            .child(
                                text(
                                    ty::META,
                                    p.ink_3,
                                    "Tests model replies and tools with invented data only.",
                                )
                                .flex_1(),
                            )
                            .child(self.ai_button(
                                "assistant-test-tools",
                                "Test model",
                                Tone::Outline,
                                allowed && valid_model(&setup.draft.model),
                                |this, _, cx| this.test_ai(true, cx),
                                cx,
                            )),
                    ),
            ),
        );
        let list_controls = self.ai_access_controls(setup, cx);
        let settings = div().id("assistant-settings-dialog").track_focus(&self.ai.dialog).tab_group().role(Role::Dialog).aria_label("AI Assistant settings")
            .w(px(desktop::AI_SETTINGS_WIDTH)).max_w_full().max_h_full().rounded(px(radius::XL)).bg(p.pane).shadow(kit::overlay_shadow(&p)).border_1().border_color(p.line_strong)
            .flex().flex_col().occlude().overflow_hidden()
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                if super::super::navigation_tab(e) {
                    if e.keystroke.modifiers.shift { window.focus_prev(cx); } else { window.focus_next(cx); }
                    if !this.ai.dialog.contains_focused(window, cx) {
                        if this.ai.settings.as_ref().is_some_and(|setup| setup.draft.uses_relay()) {
                            window.focus(&this.ai.focus_handle("assistant-settings-close", cx), cx);
                        } else { window.focus(&this.ai.url.read(cx).focus_handle.clone(), cx); }
                    }
                    cx.stop_propagation(); cx.notify();
                } else if !e.is_held && !e.keystroke.modifiers.modified() && e.keystroke.key == "escape" { this.escape_ai(window, cx); cx.stop_propagation(); }
            }))
            .child(div().flex_none().px(px(space::S5)).py(px(space::S3)).flex().items_center().gap(px(space::S3)).border_b_1().border_color(p.line)
                .child(kit::agent_mark(self.theme, Agent::Assistant, component::ICON_BUTTON)).child(text(ty::DETAIL_TITLE, p.ink, "AI Assistant").flex_1())
                .child(self.ai_icon("assistant-settings-close", Icon::X, "Close AI settings without saving", true, |this, w, cx| this.close_ai_settings(w, cx), cx)))
            .child(div().relative().min_h_0().flex().flex_col()
                .child(self.ai.settings_scroll.viewport("assistant-settings-scroll").min_h_0().px(px(space::S5)).py(px(space::S4)).flex().flex_col().gap(px(space::S3))
                .child(kit::label(&p, "Model provider").flex_none())
                .child(self.ai_provider_choices(setup, !running, cx))
                .child(if setup.draft.uses_relay() { self.ai_hosted_settings(setup, cx) } else { provider })
                .child(div().flex_none().flex().items_start().gap(px(space::S2)).p(px(space::S3)).rounded(px(radius::LG)).bg(p.canvas)
                    .child(kit::icon(if setup.draft.cloud { Icon::CircleAlert } else { Icon::CloudOff }, component::ICON_SMALL, p.ink_3))
                    .child(text(ty::META, p.ink_2, if setup.draft.cloud {
                        "No automatic fallback. ChatGPT receives prompts and allowed summaries from All models lists only."
                    } else if setup.draft.private_relay { "No cloud fallback. Only allowed task summaries go to your private model through your server." } else { "No cloud fallback. Prompts and allowed task summaries go to the loopback address you enter." })))
                .child(kit::label(&p, "Lists the assistant can see").flex_none())
                .child(text(ty::META, p.ink_3, "Select a row to change its access. Lists inherit their collection’s choice unless set individually. Without a choice, lists are hidden.").flex_none())
                .child(list_controls))
                .child(self.ai.settings_scroll.bar.overlay(&self.ai.settings_scroll.handle, &p, self.animate)))
            .child(kit::dialog_footer(&p, radius::XL).flex().justify_between().items_center().gap(px(space::S2)).px(px(space::S5)).py(px(space::S3))
                .when(setup.draft.uses_relay() && setup.draft.model.is_empty(), |footer| footer.justify_end())
                .when(setup.draft.uses_relay() && !setup.draft.model.is_empty(), |footer| footer.child(
                    self.ai_button("assistant-disconnect-model", "Disconnect model", Tone::Quiet, !running,
                        |this, _, cx| this.pick_chatgpt_model(String::new(), cx), cx),
                ))
                .when(!setup.draft.uses_relay(), |footer| footer.child(self.ai_button("assistant-forget", "Forget this device’s model", Tone::Quiet, setup.saved_file || !self.ai.profile.key.is_empty() || !setup.draft.key.is_empty(), |this, w, cx| this.forget_ai(w, cx), cx)))
                .child(self.ai_button("assistant-save", "Done", Tone::Primary, !running, |this, w, cx| this.save_ai_settings(w, cx), cx)));
        Some(
            div()
                .id("assistant-settings-overlay")
                .absolute()
                .inset_0()
                .bg(p.scrim)
                .flex()
                .items_center()
                .justify_center()
                .p(px(space::S5))
                .occlude()
                .child(settings),
        )
    }

    fn ai_model_picker(&self, setup: &Setup, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let hosted = setup.draft.uses_relay();
        let (id, row_prefix) = if hosted {
            ("assistant-chatgpt-model-picker", "assistant-chatgpt-model")
        } else {
            ("assistant-model-catalog", "assistant-model-option")
        };
        let enabled = !matches!(setup.check, Check::Running(_));
        let scroll = &self.ai.model_scroll;
        div()
            .relative()
            .flex_none()
            .rounded(px(radius::MD))
            .border_1()
            .border_color(p.line)
            .bg(p.canvas)
            .overflow_hidden()
            .child(
                scroll
                    .viewport(id)
                    .max_h(px(desktop::AI_COMPOSER_MAX))
                    .flex()
                    .flex_col()
                    .p(px(space::S1))
                    .pr(px(space::S4))
                    .children(setup.models.iter().enumerate().map(|(i, model)| {
                        let id = model.id.clone();
                        let selected = setup.draft.model == id;
                        let label = model::labels::one_line(&model.label);
                        self.ai_clickable(
                            format!("{row_prefix}-{i}"),
                            div()
                                .flex_none()
                                .w_full()
                                .min_h(px(component::BUTTON_SMALL))
                                .flex()
                                .items_center()
                                .gap(px(space::S2))
                                .px(px(space::S2))
                                .rounded(px(radius::SM))
                                .when(selected, |row| row.bg(p.wash))
                                .child(div().size(px(component::ICON_SMALL)).flex_none().children(
                                    selected.then(|| {
                                        kit::icon(
                                            Icon::Check,
                                            component::ICON_SMALL,
                                            p.accent_strong,
                                        )
                                    }),
                                ))
                                .child(one_line(text(ty::BODY, p.ink, label.clone()))),
                            enabled,
                            move |this, _, cx| {
                                if hosted {
                                    this.pick_chatgpt_model(id.clone(), cx);
                                } else {
                                    this.pick_local_model(&id, cx);
                                }
                            },
                            cx,
                        )
                        .when(enabled, |row| row.hover(|row| row.bg(p.hover)))
                        .aria_label(label)
                        .aria_toggled(toggled(selected))
                    })),
            )
            .child(scroll.bar.overlay_pinned(&scroll.handle, &p, self.animate))
    }

    pub(super) fn ai_access_state(&self) -> Arc<State> {
        match self.store.session() {
            None => self.store.view(),
            Some(session) => session
                .client()
                .assistant_snapshot()
                .map_or_else(|| Arc::new(State::new()), |snapshot| snapshot.state),
        }
    }

    fn ai_access_controls(&self, setup: &Setup, cx: &mut Context<Self>) -> Div {
        let state = self.ai_access_state();
        let p = self.palette;
        let mut collections: Vec<_> = state
            .collections
            .values()
            .filter(|c| state.is_live(c.id.into()))
            .collect();
        collections.sort_by_key(|c| (&c.name, c.id));
        let mut members = BTreeMap::<_, Vec<_>>::new();
        for list in state.lists.values().filter(|l| state.is_live(l.id.into())) {
            let collection = list.collection.filter(|id| state.is_live((*id).into()));
            members.entry(collection).or_default().push(list);
        }
        let mut rows = Vec::new();
        for group in collections
            .into_iter()
            .map(Some)
            .chain(std::iter::once(None))
        {
            if let Some(group) = group {
                let effective = setup.draft.access.collection(&state, group.id);
                let default = setup
                    .draft
                    .access
                    .collections
                    .get(&group.id)
                    .copied()
                    .unwrap_or_default();
                let inheritance = (effective != Some(default))
                    .then(|| format!("New lists: {}", access_label(Some(default))));
                rows.push(self.ai_access_row(
                    AccessTarget::Collection(group.id),
                    &group.name,
                    None,
                    effective,
                    inheritance,
                    cx,
                ));
            }
            let mut lists = members.remove(&group.map(|c| c.id)).unwrap_or_default();
            lists.sort_by_key(|l| (&l.name, l.id));
            for list in lists {
                let color = theme::list_colors(
                    self.theme,
                    model::lists::hue(list.color.as_deref(), list.id.as_uuid().as_bytes()),
                );
                rows.push(
                    self.ai_access_row(
                        AccessTarget::List(list.id),
                        &list.name,
                        Some(
                            kit::glyph(model::lists::list_glyph(&list.name), color.glyph)
                                .size(px(component::ICON_SMALL))
                                .into_any_element(),
                        ),
                        Some(setup.draft.access.list(&state, list.id)),
                        None,
                        cx,
                    ),
                );
            }
        }
        let len = rows.len();
        let inner = px(radius::LG - elevation::HAIRLINE);
        box_panel(&p)
            .flex_none()
            .children(rows.into_iter().enumerate().map(|(i, row)| {
                row.when(i == 0, |row| row.rounded_tl(inner).rounded_tr(inner))
                    .when(i + 1 == len, |row| row.rounded_bl(inner).rounded_br(inner))
            }))
    }

    fn ai_access_row(
        &self,
        target: AccessTarget,
        name: &str,
        glyph: Option<gpui::AnyElement>,
        access: Option<ListAccess>,
        inheritance: Option<String>,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let group = matches!(target, AccessTarget::Collection(_));
        let id = match target {
            AccessTarget::Collection(id) => id.as_uuid(),
            AccessTarget::List(id) => id.as_uuid(),
        };
        let name = model::labels::one_line(name);
        let label = access_label(access);
        let row = div()
            .w_full()
            .min_h(px(component::BUTTON))
            .flex()
            .items_center()
            .gap(px(space::S2))
            .px(px(space::S3))
            .children(glyph)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .py(px(space::S1))
                    .child(one_line(text(
                        if group { ty::NAV_COLLECTION } else { ty::BODY },
                        p.ink,
                        name.clone(),
                    )))
                    .children(inheritance.map(|label| one_line(text(ty::META, p.ink_3, label)))),
            )
            .children(access.map(|value| {
                kit::icon(
                    if value == ListAccess::Hidden {
                        Icon::EyeOff
                    } else {
                        Icon::Eye
                    },
                    component::ICON_SMALL,
                    if value == ListAccess::PrivateOnly {
                        p.sun_ink
                    } else {
                        p.ink_3
                    },
                )
            }))
            .child(text(ty::META, p.ink_2, label).flex_none())
            .child(kit::icon(Icon::ChevronDown, space::S3, p.ink_3));
        div()
            .flex_none()
            .border_b_1()
            .border_color(p.line)
            .when(group, |row| row.bg(p.canvas))
            .child(
                self.ai_action(
                    format!("assistant-access-{id}"),
                    row,
                    true,
                    move |this, _, cx| {
                        let state = this.ai_access_state();
                        this.ai.cloud.signed_in = None;
                        if let Some(setup) = this.ai.settings.as_mut() {
                            let policy = &mut setup.draft.access;
                            let current = match target {
                                AccessTarget::Collection(id) => policy.collection(&state, id),
                                AccessTarget::List(id) => Some(policy.list(&state, id)),
                            };
                            let next = match current {
                                Some(ListAccess::Hidden) => ListAccess::PrivateOnly,
                                Some(ListAccess::PrivateOnly) => ListAccess::AllModels,
                                Some(ListAccess::AllModels) | None => ListAccess::Hidden,
                            };
                            match target {
                                AccessTarget::Collection(id) => {
                                    policy.set_collection(&state, id, next)
                                }
                                AccessTarget::List(id) if state.is_live(id.into()) => {
                                    policy.lists.insert(id, next);
                                }
                                AccessTarget::List(_) => {}
                            }
                            cx.notify();
                        }
                    },
                    cx,
                )
                .aria_label(format!(
                    "{name}: {label}. Activate to change access{}.",
                    if group { " for the collection" } else { "" }
                )),
            )
    }

    fn ai_setting_field(
        &self,
        label: &'static str,
        input: Entity<TextInput>,
        trailing: Option<gpui::AnyElement>,
        window: &Window,
        cx: &App,
    ) -> Div {
        let p = self.palette;
        let focused = input.read(cx).focus_handle.is_focused(window)
            || (trailing.is_some()
                && self
                    .ai
                    .focus_handle("assistant-catalog-toggle", cx)
                    .is_focused(window));
        div()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(space::S2))
            .child(
                text(ty::META, p.ink_2, label)
                    .w(px(space::S10 + space::S5))
                    .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(px(component::ICON_BUTTON))
                    .px(px(space::S2))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .border_1()
                    .border_color(if focused { p.accent_strong } else { p.control })
                    .rounded(px(radius::MD))
                    .bg(p.pane)
                    .gap(px(space::S1))
                    .style(ty::COUNT)
                    .child(div().flex_1().min_w_0().child(input))
                    .children(trailing),
            )
    }
}

fn stage(status: WorkflowStatus) -> &'static str {
    match status {
        WorkflowStatus::Backlog => "Backlog",
        WorkflowStatus::Todo => "To do",
        WorkflowStatus::InProgress => "In progress",
    }
}

fn position(placement: Placement) -> &'static str {
    match placement {
        Placement::First => "Top of the list",
        Placement::Last => "End of the list",
        _ => "Relative to another task",
    }
}

fn toggled(on: bool) -> accesskit::Toggled {
    if on {
        accesskit::Toggled::True
    } else {
        accesskit::Toggled::False
    }
}

fn change_count(count: usize) -> String {
    format!("{count} {}", if count == 1 { "change" } else { "changes" })
}

fn effect_task(effect: &Effect) -> &TaskRef {
    match effect {
        Effect::Created { task, .. } | Effect::Edited { task, .. } | Effect::Deleted { task } => {
            task
        }
    }
}
