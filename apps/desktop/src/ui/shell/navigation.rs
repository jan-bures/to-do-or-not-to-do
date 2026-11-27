//! Window-local navigation, shared by the main shell and the fixed-room pop-outs.

use gpui::{KeyDownEvent, MouseUpEvent, NavigationDirection};
use todo_client::navigation::{self, Direction, History};
use todo_core::{State, model::ListView};

use super::*;

pub(super) type Location = navigation::Location<RoomKey>;

pub(super) fn initial(ui: &UiState, shell: &Shell) -> History<Location> {
    History::new(Location {
        room: shell.room.key,
        view: if ui.compact {
            ListView::List
        } else {
            model::workflow::effective(shell.room.features, None)
        },
        task: (!ui.compact)
            .then(|| shell.detail.as_ref().map(|d| d.task))
            .flatten(),
    })
}

fn available(state: &State, location: &Location) -> bool {
    let room = model::shared_key(location.room);
    navigation::room_exists(state, room)
        && navigation::view_exists(state, room, location.view)
        && location
            .task
            .is_none_or(|id| navigation::task_exists(state, id))
}

impl ShellView {
    pub(super) fn record_location(&mut self) {
        self.history.push(Location {
            room: self.shell.room.key,
            view: self.view(),
            task: self.ui.task,
        });
    }

    fn history_allowed(&self, window: &Window, cx: &App) -> bool {
        self.pairing_panel.is_none()
            && !self.ai.modal()
            && self.focus_now(window, cx).allows_task_undo()
    }

    pub(super) fn history_hint(&self, window: &Window, cx: &App) -> Option<hints::Hint> {
        if !self.history_allowed(window, cx)
            || self
                .menu
                .as_ref()
                .is_some_and(|m| m.overlay.look(self.frame).open)
        {
            return None;
        }
        let state = self.store.view();
        let can = |dir| self.history.target(dir, |l| available(&state, l)).is_some();
        match (can(Direction::Back), can(Direction::Forward)) {
            (true, true) => Some(("Alt ←/→", "back/forward")),
            (true, false) => Some(("Alt ←", "back")),
            (false, true) => Some(("Alt →", "forward")),
            (false, false) => None,
        }
    }

    fn travel(&mut self, direction: Direction, window: &mut Window, cx: &mut Context<Self>) {
        if !window.is_window_active() {
            return;
        }
        if let Some(panel) = self.pairing_panel.clone() {
            if direction == Direction::Back {
                panel.update(cx, |panel, cx| panel.history_back(window, cx));
            }
            return;
        }
        if !self.history_allowed(window, cx) {
            return;
        }
        if let Some(menu) = self
            .menu
            .as_ref()
            .filter(|m| m.overlay.look(self.frame).open)
        {
            if direction == Direction::Back
                && model::menus::is_confirmation(&self.menu_entries(menu.kind))
            {
                // Confirmations treat Escape as Cancel. Don't also leave the location underneath.
                self.close_menu(true, window, cx);
            }
            return;
        }
        let state = self.store.view();
        let Some(position) = self.history.target(direction, |l| available(&state, l)) else {
            return;
        };
        if !self.commit_edits(cx) {
            self.keep_unsaved(cx);
            return;
        }
        let Some(location) = self.history.visit(position).cloned() else {
            return;
        };
        self.cancel_task_drag(cx);
        self.cancel_step_drag(cx);
        self.multi.clear();
        let revealed = self.ui.select_room(&state, location.room);
        self.ui.task = location.task;
        if self.ai_docked() {
            self.ai.detail_over_room = location.task.is_some();
        }
        self.refresh_location(revealed, Some(location.view), cx);
        self.reveal_detail_selection();
        window.focus(&self.list_focus, cx);
    }

    pub(super) fn history_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(dir) = direction(event) {
            if !event.is_held {
                self.travel(dir, window, cx);
            }
            // In particular, don't let a browser VK fall through to DefWindowProc and get
            // translated into a second WM_APPCOMMAND after handling it here.
            cx.stop_propagation();
        }
    }

    /// Window capture (not tied to a particular hitbox): the pointer can be over a modal scrim,
    /// the caption, or anywhere else in the window. Side-button down must not dismiss a menu.
    pub(super) fn history_mouse(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                window.on_mouse_event(|event: &MouseDownEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture
                        && matches!(event.button, MouseButton::Navigate(_))
                    {
                        cx.stop_propagation();
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture
                        && let MouseButton::Navigate(dir) = event.button
                    {
                        let dir = match dir {
                            NavigationDirection::Back => Direction::Back,
                            NavigationDirection::Forward => Direction::Forward,
                        };
                        let _ = shell.update(cx, |shell, cx| shell.travel(dir, window, cx));
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }
}

fn direction(event: &KeyDownEvent) -> Option<Direction> {
    let m = event.keystroke.modifiers;
    if m.control || m.shift || m.platform || m.function {
        return None;
    }
    match (m.alt, event.keystroke.key.as_str()) {
        (true, "left") | (false, "back") => Some(Direction::Back),
        (true, "right") | (false, "forward") => Some(Direction::Forward),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use gpui::{Modifiers, TestAppContext, VisualTestContext, point};

    fn fixture<'a>(
        name: &str,
        cx: &'a mut TestAppContext,
    ) -> (Entity<ShellView>, &'a mut VisualTestContext) {
        let f = fixtures::by_name(name).expect("fixture");
        cx.update(|cx| cx.bind_keys(key_bindings()));
        cx.add_window_view(|window, cx| {
            let mut view = ShellView::new(
                Rc::new(Store::fixture(&f)),
                UiState::default(),
                Theme::Light,
                None,
                window,
                cx,
            );
            view.still();
            window.activate_window();
            window.focus(&view.list_focus, cx);
            view
        })
    }

    #[gpui::test]
    fn side_button_travels_on_release_and_text_fields_keep_focus(cx: &mut TestAppContext) {
        let (shell, cx) = fixture("my-day", cx);
        let at = point(px(500.), px(300.));
        let modifiers = Modifiers::default();
        let forward = MouseButton::Navigate(NavigationDirection::Forward);
        let back = MouseButton::Navigate(NavigationDirection::Back);
        let detail = cx.update(|_, cx| {
            shell.update(cx, |view, cx| {
                view.select_room(RoomKey::Smart(model::Smart::All), cx);
                let id = view.nav_tasks()[0];
                view.select_task(Some(id), cx);
                id
            })
        });
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.task, None));
        cx.simulate_mouse_down(at, forward, modifiers);
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.task, None));
        cx.simulate_mouse_up(at, forward, modifiers);
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.task, Some(detail)));

        cx.simulate_keystrokes("j");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_ne!(view.ui.task, Some(detail));
            assert_eq!(view.history.current().task, Some(detail));
        });
        cx.simulate_keystrokes("ctrl-k alt-left");
        cx.simulate_mouse_down(at, back, modifiers);
        cx.simulate_mouse_up(at, back, modifiers);
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert_eq!(view.history.position(), 2);
            assert!(view.filter.read(cx).focus_handle.is_focused(window));
            let focus = view.list_focus.clone();
            window.focus(&focus, cx);
        });
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.task, None));
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.room, RoomKey::default()));
    }

    #[gpui::test]
    fn restores_the_recorded_view_and_skips_a_deleted_list(cx: &mut TestAppContext) {
        let f = fixtures::by_name("board").expect("board fixture");
        let RoomKey::List(list) = f.ui.room else {
            panic!("list fixture")
        };
        let (shell, cx) = fixture("board", cx);
        cx.update(|window, cx| {
            shell.update(cx, |view, cx| {
                view.select_room(RoomKey::List(list), cx);
                assert_eq!(view.view(), ListView::Board);
                view.switch_view(ListView::List, window, cx);
                view.select_room(RoomKey::Smart(model::Smart::Important), cx);
            })
        });
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert_eq!(view.ui.room, RoomKey::List(list));
            assert_eq!(view.view(), ListView::List);
        });
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| assert_eq!(shell.read(cx).view(), ListView::Board));
        cx.simulate_keystrokes("alt-right alt-right");
        cx.update(|_, cx| {
            shell.update(cx, |view, cx| {
                assert_eq!(view.ui.room, RoomKey::Smart(model::Smart::Important));
                view.store
                    .submit(todo_client::intent::delete(list))
                    .expect("delete list");
                view.refresh(cx);
            })
        });
        cx.simulate_keystrokes("alt-left");
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.room, RoomKey::default()));
        cx.simulate_keystrokes("alt-right");
        cx.update(|_, cx| {
            assert_eq!(
                shell.read(cx).ui.room,
                RoomKey::Smart(model::Smart::Important)
            )
        });
    }
}
