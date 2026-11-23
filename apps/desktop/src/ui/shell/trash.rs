//! The Trash body uses the task list's fixed-height slots and scroll handle.

use gpui::{
    AnyElement, Context, Div, IntoElement, Role, Window, div, prelude::*, px, uniform_list,
};
use todo_client::trash::{self, Row};
use todo_core::Command;
use todo_core::ids::EntityId;

use super::{Icon, RoomKey, ShellView, component, desktop, elevation, kit, space, ty};
use crate::theme::TextStyled;
use todo_design::Glyph;

impl ShellView {
    pub(super) fn trash_feed(&self, cx: &mut Context<Self>) -> AnyElement {
        let n = self.shell.room.trash.len();
        if n == 0 {
            return self
                .empty_list(&trash::EMPTY.into(), &"".into())
                .into_any_element();
        }
        uniform_list(
            "trash-rows",
            n,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let rows = this.shell.room.trash[range].to_vec();
                rows.iter()
                    .map(|row| this.trash_row(row, Some(cx)))
                    .collect()
            }),
        )
        .track_scroll(&self.rows)
        .size_full()
        .into_any_element()
    }

    pub(super) fn trash_row(&self, row: &Row, cx: Option<&mut Context<Self>>) -> Div {
        let p = self.palette;
        let selected = cx.is_some() && self.trash_selected == Some(row.id);
        let lead = match row.id {
            EntityId::Collection(_) => kit::glyph(Glyph::Layers, p.ink_3).into_any_element(),
            EntityId::List(_) => kit::icon(Icon::List, component::ICON, p.ink_3).into_any_element(),
            _ => kit::icon(Icon::CircleCheck, component::ICON, p.ink_3).into_any_element(),
        };
        let restore = kit::button_quiet(&p, "Restore").border_color(if selected {
            p.accent
        } else {
            p.control
        });
        let action = match (row.blocked, cx) {
            (Some(_), _) => None,
            (None, Some(cx)) => {
                let row = row.clone();
                Some(
                    restore
                        .id(gpui::SharedString::from(format!("restore-{}", row.id)))
                        .role(Role::Button)
                        .aria_label(format!("Restore {}", row.title))
                        .cursor_pointer()
                        .hover(|d| d.bg(p.hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.list_focus, cx);
                            this.restore_from_trash(&row, cx);
                        }))
                        .into_any_element(),
                )
            }
            (None, None) => Some(restore.into_any_element()),
        };
        div()
            .relative()
            .w_full()
            .h(px(desktop::ROW))
            .px(px(desktop::LIST_PAD_X))
            .border_b(px(elevation::HAIRLINE))
            .border_color(p.line)
            .flex()
            .items_center()
            .gap(px(space::S3))
            .when(selected, |d| {
                d.bg(p.wash)
                    .child(kit::leading_rail(p.accent_strong, elevation::SELECTION_BAR))
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .w(px(component::ICON))
                    .items_center()
                    .justify_center()
                    .child(lead),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .style(ty::TASK_TITLE)
                            .child(row.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .style(ty::META)
                            .text_color(p.ink_3)
                            .child(row.metadata.clone()),
                    )
                    .when_some(row.blocked, |d, reason| {
                        d.child(
                            div()
                                .truncate()
                                .style(ty::META)
                                .text_color(p.ink_3)
                                .child(reason),
                        )
                    }),
            )
            .children(action)
    }

    pub(super) fn trash_step(&mut self, by: isize, cx: &mut Context<Self>) {
        let mut available = self
            .shell
            .room
            .trash
            .iter()
            .enumerate()
            .filter(|(_, row)| row.blocked.is_none());
        let next = if let Some(id) = self.trash_selected {
            let at = self.shell.room.trash.iter().position(|r| r.id == id);
            if by > 0 {
                available.find(|(i, _)| at.is_none_or(|at| *i > at))
            } else {
                available.rfind(|(i, _)| at.is_none_or(|at| *i < at))
            }
        } else if by > 0 {
            available.next()
        } else {
            available.next_back()
        };
        if let Some((index, row)) = next {
            self.trash_selected = Some(row.id);
            self.rows
                .scroll_to_item(index, gpui::ScrollStrategy::Center);
            cx.notify();
        }
    }

    pub(super) fn trash_key(
        &mut self,
        e: &gpui::KeyDownEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.list_focus.is_focused(window) || e.keystroke.modifiers.modified() {
            return;
        }
        let key = e.keystroke.key.as_str();
        match key {
            "enter" | "space" if !e.is_held => {
                if let Some(row) = self
                    .shell
                    .room
                    .trash
                    .iter()
                    .find(|r| Some(r.id) == self.trash_selected)
                    .cloned()
                {
                    self.restore_from_trash(&row, cx);
                }
            }
            "enter" | "space" => {}
            _ => return,
        }
        cx.stop_propagation();
    }

    fn restore_from_trash(&mut self, row: &Row, cx: &mut Context<Self>) {
        if self.shell.room.key != RoomKey::Trash {
            return;
        }
        match self.submit_if(Command::Restore { id: row.id }, |s| {
            trash::restore(s, row.id, row.deleted_at).is_some()
        }) {
            Ok(true) => {
                self.trash_selected = Some(row.id);
                self.trash_step(1, cx);
                if self.trash_selected == Some(row.id) {
                    self.trash_step(-1, cx);
                }
            }
            Ok(false) => self.show_notice(trash::CHANGED, true, cx),
            Err(()) => {}
        }
        self.refresh(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, store::Store, theme};
    use std::rc::Rc;

    #[gpui::test]
    fn trash_keyboard_restores_and_undo_brings_the_same_row_back(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
        let fixture = fixtures::by_name("trash").expect("populated Trash");
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let store = Rc::new(Store::fixture(&fixture));
            let mut view = ShellView::new(store, fixture.ui, theme::Theme::Light, None, window, cx);
            view.still();
            window.focus(&view.list_focus, cx);
            view
        });
        let original = cx.update(|_, cx| shell.read(cx).shell.room.trash.clone());
        let item = original[0].id;
        cx.simulate_keystrokes("down");
        cx.update(|_, cx| assert_eq!(shell.read(cx).trash_selected, Some(item)));
        cx.simulate_keystrokes("enter");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.shell.room.trash, original[1..]);
            assert!(view.store.view().is_live(item));
            assert_eq!(view.trash_selected, Some(original[1].id));
        });
        cx.simulate_keystrokes("ctrl-z");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.shell.room.trash, original);
            assert!(!view.store.view().is_live(item));
        });
    }
}
