//! The room's multi-selection strip, above the reserved Undo footer.

use super::{MenuKind, Notice, ShellView, hints, kit, undo};
use crate::theme::{Icon, TextStyled, component, desktop, elevation, radius, space, ty};
use gpui::{Context, Div, Modifiers, ScrollStrategy, Stateful, Window, div, point, prelude::*, px};
use todo_client::selection::{self, BulkTaskAction, Selection, Tool};
use todo_core::Command;
use todo_core::ids::TaskId;

impl ShellView {
    pub(super) fn bulk_ids(&self) -> Vec<TaskId> {
        if self.multi.cursor().is_none() {
            return Vec::new();
        }
        self.multi.ordered(&self.nav_tasks())
    }

    pub(super) fn change_selection(
        &mut self,
        op: impl FnOnce(&mut Selection, &[TaskId], Option<TaskId>),
        cx: &mut Context<Self>,
    ) {
        if self.shell.room.key.is_history() {
            return;
        }
        let visible = self.nav_tasks();
        if visible.is_empty() {
            return;
        }
        if !self.commit_edits(cx) {
            self.keep_unsaved(cx);
            return;
        }
        op(&mut self.multi, &visible, self.ui.task);
        self.ui.task = None;
        self.refresh(cx);
        if let Some(cursor) = self.multi.cursor() {
            if let Some(child) = self
                .shell
                .room
                .handover()
                .and_then(|next| next.child(cursor))
            {
                self.handover_scroll.scroll_to_item(child);
            } else if let Some(i) = (0..self.shell.row_count())
                .find(|i| self.shell.row(*i).is_some_and(|row| row.id == cursor))
            {
                self.rows.scroll_to_item(i, ScrollStrategy::Nearest);
            }
        }
    }

    pub(super) fn click_task(&mut self, id: TaskId, mods: Modifiers, cx: &mut Context<Self>) {
        if !self.modified_task_click(id, mods, cx) {
            self.select_task(Some(id), cx);
        }
    }

    pub(super) fn modified_task_click(
        &mut self,
        id: TaskId,
        mods: Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        if mods.shift {
            self.change_selection(|s, visible, previous| s.range(visible, id, previous), cx);
        } else if mods.control || mods.platform {
            self.change_selection(|s, visible, previous| s.toggle(visible, id, previous), cx);
        } else {
            return false;
        }
        true
    }

    pub(super) fn apply_bulk(
        &mut self,
        action: BulkTaskAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let state = self.store.view();
        let (cmd, label) = match selection::batch(&state, &self.bulk_ids(), action) {
            Ok(Some(value)) => value,
            Ok(None) => return true,
            Err(message) => {
                self.notice = Some(Notice::error(message.to_string()));
                cx.notify();
                return false;
            }
        };
        let Command::BulkTasks { tasks, .. } = &cmd else {
            return false;
        };
        let Some(id) = tasks.first().copied() else {
            return false;
        };
        drop(state);
        match self.store.submit_undoable(cmd) {
            Ok(target) => {
                self.notice = None;
                self.remember_undo(
                    undo::Action {
                        entity: id.into(),
                        kind: undo::EditKind::Bulk,
                        target,
                        room: self.shell.room.key,
                    },
                    label.into(),
                );
                self.multi.clear();
                self.edit.due = None;
                window.focus(&self.list_focus, cx);
                self.refresh(cx);
                true
            }
            Err(error) => {
                self.notice = Some(Notice::error(error));
                cx.notify();
                false
            }
        }
    }

    pub(super) fn bulk_stop(&self, index: usize) -> bool {
        let count = self.bulk_ids().len();
        if index == 6 {
            count > 0
        } else {
            count >= 2 && self.edit.due.is_none()
        }
    }

    pub(super) fn bulk_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let ids = self.bulk_ids();
        if ids.is_empty() {
            return None;
        }
        let p = self.palette;
        let count_label = selection::count_label(ids.len());
        let count_width = kit::text_width(window, ty::COUNT, &count_label);
        let clear = self
            .bulk_button(6, Icon::X, "Clear selection", None, window, cx)
            .on_click(cx.listener(|this, _, window, cx| {
                this.select_task(None, cx);
                window.focus(&this.list_focus, cx);
            }));
        let wrap = ids.len() >= 2
            && self.edit.due.is_none()
            && self.room_width(window) < desktop::LIST_MIN;
        let count = div()
            .style(ty::COUNT)
            .flex_none()
            .text_color(p.accent_strong)
            .child(count_label);
        let bar = div()
            .id("bulk-bar")
            .flex_none()
            .when(wrap, |bar| bar.flex_col().items_start().py(px(space::S2)))
            .when(!wrap, |bar| bar.h(px(desktop::TOAST_INSET)).items_center())
            .bg(p.wash)
            .border_t_1()
            .border_color(p.line)
            .px(px(desktop::LIST_PAD_X))
            .flex()
            .gap(px(space::S2))
            .child(count);
        if let Some((input, _)) = self.edit.due.as_ref() {
            return Some(
                bar.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .px(px(space::S2))
                        .py(px(space::S1))
                        .bg(p.pane)
                        .border_1()
                        .border_color(p.accent)
                        .rounded(px(radius::BUTTON))
                        .child(input.clone()),
                )
                .child(
                    div()
                        .style(ty::COUNT)
                        .text_color(p.ink_2)
                        .child("Blank clears due and repeat"),
                )
                .child(clear),
            );
        }
        let tools = selection::tools(&self.store.view(), &ids, self.store.now().today());
        let available = self.room_width(window)
            - 2. * desktop::LIST_PAD_X
            - component::ICON_BUTTON
            - space::S2
            - if wrap { 0. } else { count_width + space::S2 };
        let width = |label: fn(Tool) -> &'static str| {
            tools
                .iter()
                .map(|tool| {
                    let text = kit::text_width(window, ty::BODY, label(*tool)).ceil();
                    (component::ICON + text + 3. * space::S1 + 2. * elevation::HAIRLINE)
                        .max(component::ICON_BUTTON)
                })
                .sum::<f32>()
                + (tools.len().saturating_sub(1) as f32) * space::S1
        };
        let full = width(Tool::label) <= available;
        let compact = !full && width(Tool::short_label) <= available;
        let actions = tools.into_iter().enumerate().map(|(index, tool)| {
            let label = if full || index == 0 {
                Some(tool.label())
            } else if compact {
                Some(tool.short_label())
            } else {
                None
            };
            self.bulk_button(index, tool_icon(tool), tool.label(), label, window, cx)
                .on_click(
                    cx.listener(move |this, e: &gpui::ClickEvent, window, cx| match tool {
                        Tool::Apply(action) => {
                            this.apply_bulk(action, window, cx);
                        }
                        Tool::Move => {
                            if e.mouse_position().is_some()
                                && this.menu_press.swallows(MenuKind::BulkMove)
                            {
                                return;
                            }
                            let pos = this
                                .probes
                                .borrow()
                                .get("bulk-move")
                                .map(|bounds| bounds.origin)
                                .unwrap_or_else(|| {
                                    point(
                                        px(desktop::SIDEBAR),
                                        window.viewport_size().height
                                            - px(desktop::TOAST_INSET * 2.),
                                    )
                                });
                            this.open_menu(
                                MenuKind::BulkMove,
                                Some(pos),
                                Some(hints::Stop::Bulk(index)),
                                window,
                                cx,
                            );
                            this.highlight_first();
                        }
                        Tool::Due => this.start_due_editor(None, window, cx),
                    }),
                )
                .when(tool == Tool::Move, |el| el.child(self.probe("bulk-move")))
        });
        let actions = div()
            .flex_1()
            .flex()
            .items_center()
            .gap(px(space::S1))
            .children(actions);
        if wrap {
            Some(
                bar.child(
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(space::S2))
                        .child(actions)
                        .child(clear),
                ),
            )
        } else {
            Some(bar.child(actions).child(clear))
        }
    }

    fn bulk_button(
        &self,
        index: usize,
        glyph: Icon,
        name: &'static str,
        label: Option<&'static str>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let focus = &self.bulk_focus[index];
        let ink = if glyph == Icon::TrashTwo {
            p.danger_ink
        } else {
            p.ink_2
        };
        div()
            .id(("bulk-action", index))
            .track_focus(focus)
            .role(gpui::Role::Button)
            .aria_label(name)
            .tooltip(kit::tooltip(p, name))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(px(space::S1))
            .min_w(px(component::ICON_BUTTON))
            .h(px(component::ICON_BUTTON))
            .px(px(space::S1))
            .rounded(px(radius::BUTTON))
            .cursor_pointer()
            .hover(move |el| el.bg(p.pane))
            .border_1()
            .border_color(if focus.is_focused(window) {
                p.accent
            } else {
                p.wash
            })
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    this.select_task(None, cx);
                    window.focus(&this.list_focus, cx);
                }
            }))
            .child(kit::icon(glyph, component::ICON, ink))
            .children(label.map(|text| div().style(ty::BODY).text_color(ink).child(text)))
    }

    pub(super) fn handover_selected(&self, row: Stateful<Div>) -> Stateful<Div> {
        let p = self.palette;
        row.relative().bg(p.wash).child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(elevation::SELECTION_BAR))
                .bg(p.accent_strong),
        )
    }
}

#[cfg(test)]
#[test]
fn range_action_in_the_fixture_is_one_store_undo() {
    let fixture = crate::fixtures::by_name("multi-select").expect("fixture");
    let store = crate::store::Store::fixture(&fixture);
    let rows = fixture.shell().nav_tasks();
    let mut selected = Selection::default();
    selected.range(&rows, rows[2], Some(rows[0]));
    let original = store.view();
    let Some((command, label)) =
        selection::batch(&original, &selected.ordered(&rows), BulkTaskAction::Delete)
            .expect("valid batch")
    else {
        panic!("nothing selected");
    };
    assert_eq!(label, "Deleted 3 tasks");
    let tracked = store.submit_undoable(command).expect("one submission");
    assert!(
        rows[..3]
            .iter()
            .all(|id| !store.view().is_live((*id).into()))
    );
    store.undo(&tracked).expect("one undo");
    assert!(original.data_eq(&store.view()));
}

#[cfg(test)]
#[gpui::test]
fn keyboard_range_select_all_and_bulk_toolbar(cx: &mut gpui::TestAppContext) {
    use crate::{fixtures, store::Store, theme::Theme};
    use std::rc::Rc;

    cx.update(|cx| cx.bind_keys(super::key_bindings()));
    let fixture = fixtures::by_name("multi-select").expect("fixture");
    let (shell, cx) = cx.add_window_view(|window, cx| {
        let store = Rc::new(Store::fixture(&fixture));
        let mut view = ShellView::new(store, fixture.ui, Theme::Light, None, window, cx);
        view.still();
        window.focus(&view.list_focus, cx);
        view
    });
    let original = cx.update(|_, cx| shell.read(cx).nav_tasks());
    cx.simulate_keystrokes("down shift-down shift-down shift-up");
    cx.update(|_, cx| assert_eq!(shell.read(cx).bulk_ids(), original[..2]));
    cx.simulate_keystrokes("ctrl-a");
    cx.update(|_, cx| assert_eq!(shell.read(cx).bulk_ids(), original));
    cx.simulate_keystrokes("escape down shift-down tab");
    cx.update(|window, cx| assert_eq!(shell.read(cx).stop_at(window), Some(hints::Stop::Bulk(0))));
    cx.simulate_keystrokes("enter");
    cx.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").expect("Enter"),
    });
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert!(view.bulk_ids().is_empty());
        assert!(
            original[..2]
                .iter()
                .all(|id| view.store.view().tasks[id].is_done())
        );
    });
    cx.simulate_keystrokes("ctrl-z");
    cx.update(|_, cx| {
        let view = shell.read(cx);
        assert!(
            original[..2]
                .iter()
                .all(|id| !view.store.view().tasks[id].is_done())
        );
    });
}

fn tool_icon(tool: Tool) -> Icon {
    match tool {
        Tool::Apply(BulkTaskAction::Completion { done: true }) => Icon::Check,
        Tool::Apply(BulkTaskAction::Completion { done: false }) => Icon::UndoTwo,
        Tool::Apply(BulkTaskAction::Important { .. }) => Icon::Star,
        Tool::Apply(BulkTaskAction::MyDay { .. }) => Icon::Sun,
        Tool::Move => Icon::List,
        Tool::Due => Icon::Calendar,
        Tool::Apply(BulkTaskAction::Delete) => Icon::TrashTwo,
        Tool::Apply(_) => unreachable!("picker tools don't carry a value"),
    }
}
