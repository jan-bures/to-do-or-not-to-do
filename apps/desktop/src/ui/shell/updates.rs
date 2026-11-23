//! The Updates page of the sidebar Settings menu and its quiet, non-modal entry point.

use gpui::{AsyncApp, ClickEvent, Context, Div, Role, Window, div, prelude::*, px};
use todo_core::time::Timestamp;

use super::ShellView;
use super::menu::{MenuKind, ring};
use crate::model::hints::Stop;
use crate::model::menus::{MenuAction, MenuEntry, item};
use crate::theme::{Icon, TextStyled, component, desktop, radius, space, ty};
use crate::ui::kit;
use crate::updates::{Controller, Failure, Snapshot, State};

impl ShellView {
    pub fn set_updates(&mut self, controller: Controller, cx: &mut Context<Self>) {
        let mut updates = controller.subscribe();
        self.updates = Some(controller);
        self.watchers
            .push(cx.spawn(async move |this, cx: &mut AsyncApp| {
                while updates.changed().await.is_ok() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
    }

    pub fn open_updates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_menu(MenuKind::Updates, None, Some(Stop::Account), window, cx);
    }

    pub(super) fn update_snapshot(&self) -> Snapshot {
        self.updates
            .as_ref()
            .map_or_else(Snapshot::default, Controller::snapshot)
    }

    pub(super) fn update_entries(snapshot: &Snapshot) -> Vec<MenuEntry> {
        let mut entries = Vec::new();
        if snapshot.ready_version().is_some() {
            entries.push(item(
                "restart-update",
                Icon::ArrowUp,
                "Restart to update",
                MenuAction::RestartToUpdate,
            ));
        } else if snapshot.can_check {
            entries.push(item(
                "check-update",
                Icon::RefreshCw,
                "Check now",
                MenuAction::CheckUpdates,
            ));
        }
        entries.extend([
            MenuEntry::Separator,
            item(
                "updates-back",
                Icon::ChevronLeft,
                "Back to Settings",
                MenuAction::CloseUpdates,
            ),
        ]);
        entries
    }

    pub(super) fn update_header(&self, snapshot: &Snapshot) -> Div {
        let p = self.palette;
        let state = &snapshot.state;
        let (icon, ink, title, detail) = match state {
            State::NotInstalled => (
                Icon::ArrowUp,
                p.ink_3,
                "Automatic updates unavailable",
                "Updates are available in the installed app.".into(),
            ),
            State::Disabled => (
                Icon::ArrowUp,
                p.ink_3,
                "Automatic updates disabled",
                "This build has no update feed.".into(),
            ),
            State::Idle => (
                Icon::Clock,
                p.ink_3,
                "Waiting to check",
                "Updates check automatically in the background.".into(),
            ),
            State::UpToDate { checked_at } => (
                Icon::CircleCheck,
                p.success,
                "Up to date",
                relative_status("Checked", *checked_at, self.store.now().at),
            ),
            State::Checking => (
                Icon::RefreshCw,
                p.ink_2,
                "Checking for updates…",
                "Looking for a newer version.".into(),
            ),
            State::Downloading { progress } => (
                Icon::Download,
                p.ink_2,
                "Preparing the update…",
                progress.map_or_else(
                    || "Downloading in the background.".into(),
                    |percent| format!("Preparing package · {percent}%"),
                ),
            ),
            State::Ready { version } => (
                Icon::ArrowUp,
                p.accent_strong,
                "Update ready",
                format!("{version} is ready to install."),
            ),
            State::Failed { kind, at } => {
                let title = match kind {
                    Failure::Check => "Couldn’t check for updates",
                    Failure::Download => "Couldn’t prepare the update",
                    Failure::Apply => "Couldn’t apply the update",
                };
                let detail = relative_status("Last attempt", *at, self.store.now().at);
                (Icon::CircleAlert, p.danger_ink, title, detail)
            }
        };
        let version = snapshot.installed_version.as_ref().map_or_else(
            || format!("{} · development build", env!("CARGO_PKG_VERSION")),
            |version| format!("v{version}"),
        );
        div()
            .px(px(space::S3))
            .pt(px(space::S3))
            .pb(px(space::S2))
            .flex()
            .flex_col()
            .gap(px(space::S3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .style(ty::NAV_COLLECTION)
                            .text_color(p.ink)
                            .child("Updates"),
                    )
                    .child(div().style(ty::COUNT).text_color(p.ink_3).child(version)),
            )
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    .child(div().pt(px(space::S1)).flex_none().child(kit::icon(
                        icon,
                        component::ICON_SMALL,
                        ink,
                    )))
                    .child(
                        div()
                            .id("update-status")
                            .role(Role::Status)
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(space::S1))
                            .child(div().style(ty::NAV_SUB).text_color(p.ink).child(title))
                            .child(div().style(ty::META).text_color(p.ink_3).child(detail))
                            .when(snapshot.restart_failed, |d| {
                                d.child(
                                    div()
                                        .style(ty::META)
                                        .text_color(p.danger_ink)
                                        .child("Couldn’t start the update. Try restarting again."),
                                )
                            }),
                    ),
            )
    }

    pub(super) fn update_busy_button(&self, state: &State) -> Option<Div> {
        let p = self.palette;
        matches!(state, State::Checking | State::Downloading { .. }).then(|| {
            div().px(px(space::S3)).pb(px(space::S2)).child(
                kit::button_quiet(&p, "Check now")
                    .h(px(component::BUTTON_SMALL))
                    .px(px(space::S3))
                    .bg(p.canvas)
                    .border_color(p.line)
                    .text_color(p.ink_3)
                    .style(ty::BUTTON_SMALL),
            )
        })
    }

    pub(super) fn update_indicator(&self, window: &Window, cx: &mut Context<Self>) -> Option<Div> {
        let version = self.update_snapshot().indicator_version()?.to_owned();
        let p = self.palette;
        let label = format!("Update {version} ready. Open Updates");
        let open =
            div()
                .id("update-indicator")
                .role(Role::Button)
                .aria_label(label.clone())
                .aria_expanded(self.menu.as_ref().is_some_and(|m| {
                    m.kind == MenuKind::Updates && m.overlay.look(self.frame).open
                }))
                .tooltip(kit::tooltip(p, label))
                .h(px(desktop::NAV_SUB_ROW))
                .px(px(space::S2))
                .rounded(px(radius::BUTTON))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .cursor_pointer()
                .track_focus(&self.update_focus)
                .hover(|d| d.bg(p.hover))
                .child(kit::icon(
                    Icon::ArrowUp,
                    component::ICON_SMALL,
                    p.accent_strong,
                ))
                .child(
                    div()
                        .style(ty::NAV_SUB)
                        .text_color(p.ink)
                        .child("Update ready"),
                )
                .on_click(cx.listener(|this, e: &ClickEvent, window, cx| {
                    this.toggle_menu(MenuKind::Updates, Stop::UpdateIndicator, e, window, cx);
                }))
                .on_key_down(cx.listener(|this, e, window, cx| this.control_key(e, window, cx)));
        let dismiss = div()
            .id("dismiss-update-indicator")
            .role(Role::Button)
            .aria_label("Dismiss update indicator")
            .tooltip(kit::tooltip(p, "Dismiss update indicator"))
            .size(px(component::ICON_BUTTON))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::BUTTON))
            .cursor_pointer()
            .track_focus(&self.dismiss_update_focus)
            .hover(|d| d.bg(p.hover))
            .child(kit::icon(Icon::X, component::ICON_SMALL, p.ink_3))
            .on_click(cx.listener(move |this, _, window, cx| {
                if let Some(updates) = &this.updates {
                    updates.dismiss(&version);
                    if this.dismiss_update_focus.is_focused(window) {
                        this.focus_stop(Stop::Account, window, cx);
                    }
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(|this, e, window, cx| this.control_key(e, window, cx)));
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .justify_between()
                .mx(px(space::S2))
                .mb(px(space::S2))
                .rounded(px(radius::MD))
                .bg(p.wash)
                .child(ring(
                    &p,
                    self.update_focus.is_focused(window),
                    radius::BUTTON,
                    open,
                ))
                .child(ring(
                    &p,
                    self.dismiss_update_focus.is_focused(window),
                    radius::BUTTON,
                    dismiss,
                )),
        )
    }
}

fn relative_status(label: &str, at: Timestamp, now: Timestamp) -> String {
    let age = todo_client::activity::relative_time(at, now);
    format!("{label} {}", age.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::shell::ShellEvent;
    use std::{cell::Cell, rc::Rc};

    #[gpui::test]
    fn the_indicator_opens_updates_and_dismissing_keeps_restart_available(
        cx: &mut gpui::TestAppContext,
    ) {
        let controller = Controller::fixture(State::Ready {
            version: "0.1.0-build.147".into(),
        });
        let fixture = crate::fixtures::my_day();
        let (shell, cx) = cx.add_window_view(|window, cx| {
            let store = Rc::new(crate::store::Store::fixture(&fixture));
            let mut shell = ShellView::new(
                store,
                fixture.ui,
                crate::theme::Theme::Light,
                None,
                window,
                cx,
            );
            shell.set_updates(controller.clone(), cx);
            shell.still();
            shell
        });
        let restart_requests = Rc::new(Cell::new(0));
        cx.update(|window, cx| {
            let count = restart_requests.clone();
            cx.subscribe(&shell, move |_, event: &ShellEvent, _| {
                if event == &ShellEvent::RestartToUpdate {
                    count.set(count.get() + 1);
                }
            })
            .detach();
            window.focus(&shell.read(cx).update_focus.clone(), cx);
        });
        let enter = |cx: &mut gpui::VisualTestContext| {
            cx.simulate_keystrokes("enter");
            cx.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("enter").expect("key"),
            });
        };
        enter(cx);
        cx.update(|_, cx| {
            let menu = shell.read(cx).menu.as_ref().expect("Updates is open");
            assert_eq!(menu.kind, MenuKind::Updates);
            assert_eq!(menu.highlight.as_deref(), Some("restart-update"));
        });
        enter(cx);
        assert_eq!(restart_requests.get(), 1);
        cx.simulate_keystrokes("escape");
        cx.update(|window, cx| {
            assert!(shell.read(cx).update_focus.is_focused(window));
            window.focus(&shell.read(cx).dismiss_update_focus.clone(), cx);
        });
        enter(cx);
        cx.update(|window, cx| {
            let shell = shell.read(cx);
            assert!(shell.account_focus.is_focused(window));
            assert!(!shell.has_stop(Stop::UpdateIndicator));
            assert!(!shell.has_stop(Stop::DismissUpdate));
        });
        assert_eq!(
            controller.snapshot().ready_version(),
            Some("0.1.0-build.147")
        );
        assert_eq!(controller.snapshot().indicator_version(), None);
    }
}
