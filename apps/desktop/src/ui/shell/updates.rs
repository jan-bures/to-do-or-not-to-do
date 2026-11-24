//! The Updates page of the sidebar Settings menu and its quiet, non-modal entry point.

use gpui::{AsyncApp, Context, Div, Role, Window, div, prelude::*, px, relative};
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
                "Check for updates",
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
                "Downloading update…",
                progress.map_or_else(
                    || "Downloading in the background.".into(),
                    |percent| format!("{percent}% downloaded"),
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
                    Failure::Verify => "Update couldn’t be verified",
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
                kit::button_quiet(&p, "Check for updates")
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
        let snapshot = self.update_snapshot();
        let p = self.palette;
        let footer = div().flex_none().mx(px(space::S2)).mb(px(space::S2));
        let row = || {
            div()
                .h(px(desktop::NAV_SUB_ROW))
                .px(px(space::S2))
                .flex()
                .items_center()
                .gap(px(space::S2))
                .style(ty::NAV_SUB)
        };
        let ready = snapshot.ready_version();
        if snapshot.footer_action() {
            let retry = snapshot.restart_failed || ready.is_none();
            let (label, icon, ink) = if retry {
                ("Update failed · Retry", Icon::RefreshCw, p.danger_ink)
            } else {
                ("Restart to update", Icon::ArrowUp, p.accent_strong)
            };
            let tip = ready.map_or_else(|| label.into(), |build| format!("{label} ({build})"));
            let button = row()
                .id("update-indicator")
                .role(Role::Button)
                .aria_label(tip.clone())
                .tooltip(kit::tooltip(p, tip))
                .w_full()
                .rounded(px(radius::BUTTON))
                .bg(if retry { p.canvas } else { p.wash })
                .hover(move |d| d.bg(if retry { p.wash } else { p.tint }))
                .text_color(ink)
                .cursor_pointer()
                .track_focus(&self.update_focus)
                .child(kit::icon(icon, component::ICON_SMALL, ink))
                .child(label)
                .on_click(cx.listener(|this, _, window, cx| {
                    if this.update_snapshot().ready_version().is_some() {
                        cx.emit(super::ShellEvent::RestartToUpdate);
                    } else if let Some(updates) = this.updates.clone() {
                        if this.update_focus.is_focused(window) {
                            this.focus_stop(Stop::Account, window, cx);
                        }
                        updates.check_now();
                    }
                }))
                .on_key_down(cx.listener(|this, e, window, cx| this.control_key(e, window, cx)));
            return Some(footer.child(ring(
                &p,
                self.update_focus.is_focused(window),
                radius::BUTTON,
                button,
            )));
        }
        let (label, progress) = match snapshot.state {
            State::Checking => ("Checking for updates…", None),
            State::Downloading { progress } => ("Downloading update…", progress),
            State::Failed { .. } => ("Update failed", None),
            _ => return None,
        };
        Some(
            footer.child(
                div()
                    .id("update-status-footer")
                    .role(Role::Status)
                    .child(
                        row()
                            .text_color(p.ink_3)
                            .justify_between()
                            .child(label)
                            .children(
                                progress.map(|n| div().style(ty::COUNT).child(format!("{n}%"))),
                            ),
                    )
                    .children(progress.map(|n| {
                        div()
                            .h(px(space::S1))
                            .rounded(px(radius::BUTTON))
                            .bg(p.line)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(f32::from(n) / 100.))
                                    .bg(p.accent_strong),
                            )
                    })),
            ),
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
    fn the_footer_requests_restart_directly(cx: &mut gpui::TestAppContext) {
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
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").expect("key"),
        });
        assert_eq!(restart_requests.get(), 1);
        cx.update(|_, cx| assert!(shell.read(cx).menu.is_none()));
    }
}
