//! The compact shell uses the normal task list, quick-add, completion motion and reserved Undo strip.

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Context, Div, FocusHandle, KeyDownEvent,
    MouseButton, Role, SharedString, Task, Toggled, WeakEntity, Window, WindowControlArea, div,
    prelude::*, px,
};
use todo_core::ids::TaskId;

use super::{Blend, FocusQuickAdd, ShellView, UndoTask, header_landscape, kit, win};
use crate::model::popouts::Room;
use crate::theme::{Icon, TextStyled, component, desktop, elevation, space, ty, with_alpha};
use crate::ui::popouts::Settings;

pub(super) struct Compact {
    pinned: bool,
    pin_fill: super::Track,
    focus: FocusHandle,
    unavailable: bool,
    title: SharedString,
    main: Option<(AnyWindowHandle, WeakEntity<ShellView>)>,
    settings: Option<Rc<RefCell<Settings>>>,
    latest_placement_save: Option<Task<()>>,
}

impl Compact {
    fn new(pinned: bool, cx: &App) -> Self {
        Self {
            pinned,
            pin_fill: super::Track::rest(f32::from(pinned)),
            focus: cx.focus_handle(),
            unavailable: false,
            title: SharedString::default(),
            main: None,
            settings: None,
            latest_placement_save: None,
        }
    }
}

impl ShellView {
    pub fn preview_popout(&mut self, pinned: bool, cx: &App) {
        self.compact = Some(Compact::new(pinned, cx));
    }

    pub(in crate::ui) fn wire_popout(
        &mut self,
        room: Room,
        pinned: bool,
        settings: Rc<RefCell<Settings>>,
        main: (AnyWindowHandle, WeakEntity<ShellView>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut compact = Compact::new(pinned, cx);
        compact.main = Some(main);
        compact.settings = Some(settings.clone());
        self.compact = Some(compact);
        let hwnd = win::hwnd_of(window);
        cx.observe_window_bounds(window, move |this, _, cx| {
            let Some(compact) = this.compact.as_mut() else {
                return;
            };
            let Some(settings) = compact.settings.clone() else {
                return;
            };
            if !win::is_shown(hwnd) || !settings.borrow_mut().sample(room, hwnd) {
                return;
            }
            // WM_MOVE/WM_SIZE can arrive for every mouse movement. Don't flush a file on each one.
            compact.latest_placement_save = Some(cx.spawn(async move |_, cx: &mut AsyncApp| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(300))
                    .await;
                settings.borrow_mut().save();
            }));
        })
        .detach();
        window.on_window_should_close(cx, move |_, _| {
            let mut settings = settings.borrow_mut();
            settings.sample(room, hwnd);
            settings.update(room, |prefs| prefs.open = false);
            settings.save();
            true
        });
    }

    pub(in crate::ui) fn popout_pin_changed(&mut self, pinned: bool, cx: &mut Context<Self>) {
        use todo_design::motion as m;
        let now = self.now();
        let pace = self.pace();
        if let Some(compact) = self.compact.as_mut()
            && compact.pinned != pinned
        {
            compact.pinned = pinned;
            let millis = if pace.reduced {
                m::reduced(m::CHECK_FILL, true)
            } else {
                m::CHECK_FILL
            };
            compact.pin_fill.aim(
                f32::from(pinned),
                now,
                std::time::Duration::from_millis(millis.into()),
                m::STANDARD,
            );
            cx.notify();
        }
    }

    pub(super) fn popout_available(&mut self, available: bool, cx: &mut Context<Self>) {
        if let Some(compact) = self.compact.as_mut()
            && compact.unavailable == available
        {
            compact.unavailable = !available;
            cx.notify();
        }
    }

    pub(super) fn close_popout(&self, window: &Window) {
        if self.compact.as_ref().is_some_and(|c| c.settings.is_some()) {
            win::request_close(win::hwnd_of(window));
        }
    }

    pub(super) fn show_task_in_main(&self, id: TaskId, cx: &mut Context<Self>) {
        let Some((window, main)) = self.compact.as_ref().and_then(|c| c.main.clone()) else {
            return;
        };
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let hwnd = cx.update(|cx| {
                cx.update_window(window, |_, window, cx| {
                    main.update(cx, |main, cx| main.reveal_task(id, window, cx))
                        .ok();
                    win::hwnd_of(window)
                })
                .ok()
            });
            if let Some(hwnd) = hwnd {
                win::show_and_focus(hwnd);
            }
        })
        .detach();
    }

    fn toggle_pin(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compact) = &self.compact else { return };
        let Some(settings) = compact.settings.clone() else {
            return;
        };
        let Some(room) = Room::for_key(self.ui.room) else {
            return;
        };
        let hwnd = win::hwnd_of(window);
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let Some(pinned) = cx.update(|cx| {
                this.upgrade()
                    .and_then(|view| view.read(cx).compact.as_ref().map(|c| !c.pinned))
            }) else {
                return;
            };
            if let Err(error) = win::set_topmost(hwnd, pinned) {
                tracing::warn!(%error, "couldn't change list window pin");
                let _ = this.update(cx, |view, cx| {
                    view.show_notice("Couldn't change always on top.", true, cx)
                });
                return;
            }
            let mut settings = settings.borrow_mut();
            settings.update(room, |prefs| prefs.pinned = pinned);
            settings.save();
            let _ = this.update(cx, |view, cx| view.popout_pin_changed(pinned, cx));
        })
        .detach();
    }

    fn compact_keys(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(compact) = &self.compact else { return };
        if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
            if self.cancel_task_drag(cx) {
                cx.stop_propagation();
                return;
            }
            if self.multi.cursor().is_some() {
                self.select_task(None, cx);
                window.focus(&self.list_focus, cx);
            } else {
                self.close_popout(window);
            }
            cx.stop_propagation();
        } else if super::navigation_tab(e) && !self.menu_focus.is_focused(window) {
            let mut stops = vec![compact.focus.clone()];
            if !compact.unavailable {
                let bulk = self
                    .bulk_focus
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| self.bulk_stop(*index))
                    .map(|(_, handle)| handle.clone());
                let quick_add = super::hints::QUICK_ADD_STOPS
                    .into_iter()
                    .filter(|&stop| self.has_stop(stop))
                    .map(|stop| self.stop_handle(stop).clone());
                stops.splice(
                    0..0,
                    std::iter::once(self.list_focus.clone())
                        .chain(bulk)
                        .chain(quick_add),
                );
                if self.has_stop(super::hints::Stop::Undo) {
                    stops.push(self.undo_focus.clone());
                }
            }
            let index = stops.iter().position(|h| h.is_focused(window));
            let len = stops.len();
            let next = if e.keystroke.modifiers.shift {
                index.map_or(len - 1, |i| (i + len - 1) % len)
            } else {
                index.map_or(0, |i| (i + 1) % len)
            };
            window.focus(&stops[next], cx);
            cx.stop_propagation();
            cx.notify();
        } else if e.keystroke.key == "enter"
            && !e.keystroke.modifiers.modified()
            && !e.is_held
            && self.list_focus.is_focused(window)
            && !compact.unavailable
        {
            if let Some(id) = self.ui.task.filter(|&id| self.task_is_visible(id)) {
                self.show_task_in_main(id, cx);
            }
            cx.stop_propagation();
        }
        // Pin and Undo are real GPUI buttons: they already click on Enter/Space release.
    }

    pub(super) fn compact_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let Some(compact) = self.compact.as_mut() else {
            return div();
        };
        let unavailable = compact.unavailable;
        let title = self.shell.room.title.clone();
        if title != compact.title {
            if compact.settings.is_some() {
                window.set_window_title(&title);
            }
            compact.title = title.clone();
        }
        let focused = compact.focus.is_focused(window);
        let pinned = compact.pinned;
        let label = if pinned {
            "Don't keep on top"
        } else {
            "Keep on top"
        };
        let (hover, pressed) = kit::caption_button_tints(&p);
        let fill = compact.pin_fill.at(self.frame);
        if compact.pin_fill.moving(self.frame) {
            window.request_animation_frame();
        }
        let pin = div()
            .id("pin-window")
            .track_focus(&compact.focus)
            .role(Role::Button)
            .aria_label(label)
            .aria_toggled(if pinned {
                Toggled::True
            } else {
                Toggled::False
            })
            .tooltip(kit::tooltip(p, label))
            .cursor_pointer()
            .relative()
            .flex_none()
            .w(px(desktop::CAPTION_WIDTH))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(
                div()
                    .relative()
                    .size(px(component::ICON_SMALL))
                    .when(fill < 1., |d| {
                        d.child(kit::icon(
                            Icon::Pin,
                            component::ICON_SMALL,
                            with_alpha(p.ink_2, 1. - fill),
                        ))
                    })
                    .when(fill > 0., |d| {
                        d.child(
                            kit::icon(
                                Icon::PinFilled,
                                component::ICON_SMALL,
                                with_alpha(p.accent_strong, fill),
                            )
                            .absolute()
                            .inset_0(),
                        )
                    }),
            )
            .when(focused, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border(px(elevation::FOCUS_RING))
                        .border_color(p.tint)
                        .child(
                            div()
                                .size_full()
                                .border(px(elevation::HAIRLINE))
                                .border_color(p.accent),
                        ),
                )
            })
            .on_click(cx.listener(|this, _, window, cx| this.toggle_pin(window, cx)));
        let bar = div()
            .h(px(desktop::TITLEBAR))
            .flex_none()
            .flex()
            .items_center()
            .child(
                div()
                    .h_full()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .pl(px(space::S4))
                    .pr(px(space::S2))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .flex_none()
                            .size(px(component::CHIP_DOT))
                            .rounded_full()
                            .bg(p.accent_strong),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .style(ty::ROOM_TITLE_BAR)
                            .child(title),
                    ),
            )
            .child(pin)
            .child(kit::window_controls(&p, window, self.caption_preview).flex_none());
        let summary = div()
            .px(px(desktop::LIST_PAD_X))
            .pt(px(space::S2))
            .text_color(p.ink_2)
            .style(ty::SUMMARY)
            .child(self.shell.room.score.text());
        let header = div()
            .flex_none()
            .bg(p.sky)
            .border_y_1()
            .border_color(p.line)
            .child(summary)
            .child(header_landscape(
                &self.landscape,
                p,
                super::theme::dusk(self.painted_hue),
                self.motion.sun(self.frame),
                Blend {
                    height: 0.,
                    scene: 0.,
                    night: 0.,
                    sun_alpha: self.motion.sun_alpha(self.frame),
                },
            ));
        let status = self.notice.as_ref().map(|notice| {
            div()
                .flex_none()
                .h(px(desktop::STATUSBAR))
                .px(px(space::S4))
                .flex()
                .items_center()
                .border_t_1()
                .border_color(p.line)
                .style(ty::META)
                .text_color(if notice.error { p.danger_ink } else { p.ink_2 })
                .child(
                    div()
                        .min_w_0()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(notice.text.clone()),
                )
        });
        let contents = if unavailable {
            div()
                .id("list-unavailable")
                .track_focus(&self.list_focus)
                .flex_1()
                .p(px(desktop::LIST_PAD_X))
                .text_color(p.ink_2)
                .child("This list is unavailable.")
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .bg(p.pane)
                .child(header)
                .child(self.quick_add_field(window, cx))
                .child(self.list(window, cx))
                .children(self.bulk_bar(window, cx))
                .child(self.list_footer(true, window, cx))
                .children(status)
                .into_any_element()
        };
        // A native drag area and caption controls keep the original Windows hit tests. Pin lives
        // outside that area, so the click never starts a move or opens Snap Layouts.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.canvas)
            .text_color(p.ink)
            .style(ty::BODY)
            .child(
                div()
                    .id("popout")
                    .key_context("Shell")
                    .size_full()
                    .flex()
                    .flex_col()
                    .capture_key_down(cx.listener(Self::history_key))
                    .on_key_down(cx.listener(Self::compact_keys))
                    .on_action(cx.listener(move |this, _: &FocusQuickAdd, window, cx| {
                        if !unavailable {
                            window.focus(&this.quick_add.read(cx).focus_handle.clone(), cx);
                            cx.notify();
                        }
                    }))
                    .on_action(cx.listener(|this, _: &UndoTask, window, cx| {
                        let focus = this.focus_now(window, cx);
                        if focus.allows_task_undo() || this.undo_focus.is_focused(window) {
                            if focus == super::hints::Focus::Menu {
                                this.close_menu(true, window, cx);
                            }
                            this.request_undo(cx);
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            if !window.default_prevented() {
                                window.focus(&this.list_focus, cx);
                            }
                        }),
                    )
                    .child(bar)
                    .child(contents)
                    .child(self.press_counter(cx))
                    .child(self.history_mouse(cx))
                    .child(self.drag_events(cx))
                    .children(self.drag_preview(window))
                    .children(self.confirmation_scrim())
                    .children(self.popover(
                        |k| {
                            matches!(
                                k,
                                super::MenuKind::Task { .. }
                                    | super::MenuKind::Status { at_row: true, .. }
                                    | super::MenuKind::BulkMove
                            )
                        },
                        cx,
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{fixtures, model, store::Store, theme};

    #[gpui::test]
    fn compact_list_keys_use_the_shared_replica_and_local_completion_motion(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
        let f = fixtures::by_name("popout-pinned").expect("pop-out fixture");
        let store = Rc::new(Store::fixture(&f));
        let main_ui = model::UiState {
            compact: false,
            ..f.ui.clone()
        };
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let mut view =
                ShellView::new(store.clone(), f.ui, theme::Theme::Dark, None, window, cx);
            view.preview_popout(true, cx);
            window.focus(&view.list_focus, cx);
            view
        });
        let initial = cx.update(|_, cx| shell.read(cx).ui.task);
        cx.simulate_keystrokes("j");
        let next = cx.update(|_, cx| shell.read(cx).ui.task);
        assert_ne!(next, initial);
        cx.simulate_keystrokes("k");
        cx.update(|_, cx| assert_eq!(shell.read(cx).ui.task, initial));
        let task = initial.expect("selected task");
        cx.simulate_keystrokes("space");
        // The command is already applied and other windows can derive from it. The originating
        // list still holds the row while the normal completion choreography plays.
        cx.update(|_, cx| {
            let view = shell.read(cx);
            assert!(view.ui.holding.contains(&task));
            assert!(store.view().tasks[&task].is_done());
            let main = model::derive(&store.view(), &main_ui, &store.sync(), &store.now());
            assert!(!main.rows().any(|row| row.id == task));
        });
        cx.simulate_keystrokes("ctrl-n");
        cx.update(|window, cx| {
            assert!(
                shell
                    .read(cx)
                    .quick_add
                    .read(cx)
                    .focus_handle
                    .is_focused(window)
            )
        });
    }

    #[gpui::test]
    fn compact_selection_has_keyboard_actions_and_escape_clears_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.bind_keys(super::super::key_bindings()));
        let f = fixtures::by_name("popout-pinned").expect("pop-out fixture");
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let mut view = ShellView::new(
                Rc::new(Store::fixture(&f)),
                f.ui,
                theme::Theme::Dark,
                None,
                window,
                cx,
            );
            view.preview_popout(true, cx);
            view.still();
            window.focus(&view.list_focus, cx);
            view
        });
        cx.simulate_keystrokes("ctrl-a escape");
        cx.update(|_, cx| assert!(shell.read(cx).bulk_ids().is_empty()));
        cx.simulate_keystrokes("ctrl-a tab");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert_eq!(view.bulk_ids(), view.nav_tasks());
            assert!(view.bulk_ids().len() >= 2);
            assert!(view.bulk_focus[0].is_focused(window));
        });
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            let view = shell.read(cx);
            assert!(view.bulk_ids().is_empty());
            assert!(view.list_focus.is_focused(window));
        });
    }

    #[test]
    fn compact_my_day_does_not_switch_to_the_tomorrow_handover() {
        let mut f = fixtures::by_name("finished").expect("finished fixture");
        let room = f.shell().room;
        assert!(room.finished_day());
        assert!(room.handover().is_some() || room.plan_only().is_some());
        f.ui.compact = true;
        let room = f.shell().room;
        assert!(room.finished_day());
        assert!(room.handover().is_none() && room.plan_only().is_none());
    }
}
