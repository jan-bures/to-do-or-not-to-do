//! Account invite/approval card. Only the non-secret status crosses into the rest of the shell.

use super::*;
use crate::session::{self, Runtime};
use crate::ui::pairing_qr;
use std::time::SystemTime;
use todo_client::device_pairing::{self as pair, Invitation, PendingDevice, Qr};
use todo_client::native::HttpTransport;
use todo_client::pairing::PairError;

#[allow(missing_debug_implementations)]
pub(super) struct Panel {
    invite_mode: bool,
    net: Option<(Runtime, HttpTransport)>,
    focus: FocusHandle,
    buttons: [FocusHandle; 7],
    input: Entity<TextInput>,
    // The server's lifetime keeps running while Windows is suspended.
    invite: Option<(Invitation, Qr, SystemTime)>,
    pending: Option<(String, PendingDevice)>,
    busy: bool,
    message: Option<SharedString>,
    p: Palette,
}

#[derive(Clone)]
pub(super) struct Closed;
impl EventEmitter<Closed> for Panel {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    Create,
    Copy,
    Cancel,
    Review,
    Approve,
    Another,
    Close,
}

fn tab_index(current: Option<usize>, forward: bool, len: usize) -> usize {
    match (current, forward) {
        (Some(i), true) => (i + 1) % len,
        (Some(i), false) => (i + len - 1) % len,
        (None, true) => 0,
        (None, false) => len - 1,
    }
}

fn until_expiry_ms(deadline: SystemTime) -> i64 {
    let millis = |duration: Duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
    match deadline.duration_since(SystemTime::now()) {
        Ok(left) if !left.is_zero() => millis(left).max(1),
        Ok(_) => 0,
        Err(elapsed) => -millis(elapsed.duration()),
    }
}

impl Panel {
    fn new(
        invite_mode: bool,
        net: Option<(Runtime, HttpTransport)>,
        p: Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input =
            cx.new(|cx| TextInput::new("Code shown on the device", quick_add_colors(&p), cx));
        cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _, cx| match event {
                InputEvent::Submit if this.pending.is_none() => this.review(false, cx),
                InputEvent::Submit => {}
                InputEvent::Cancel => this.close(cx),
                InputEvent::Changed => {}
            },
        )
        .detach();
        let focus = cx.focus_handle();
        if !invite_mode {
            let handle = input.read(cx).focus_handle.clone();
            window.focus(&handle, cx);
        }
        let mut panel = Self {
            invite_mode,
            net,
            focus,
            buttons: std::array::from_fn(|_| cx.focus_handle()),
            input,
            invite: None,
            pending: None,
            busy: false,
            message: None,
            p,
        };
        if panel.net.is_none() && invite_mode {
            panel.show(Invitation {
                id: String::new(),
                code: "ABCDEF-GHJKMN".into(),
                link: format!("https://todo.example.com/pair/invite#{}", "A".repeat(43)),
                expires_in: 300,
            });
        } else if panel.net.is_none() {
            panel.pending = Some((
                "ABCDEFGH".into(),
                PendingDevice {
                    device_name: "New phone".into(),
                    created_at: 0,
                    source: Some("203.0.113.7".into()),
                },
            ));
        }
        panel
    }

    fn show(&mut self, i: Invitation) {
        let expires = SystemTime::now() + Duration::from_secs(u64::from(i.expires_in));
        self.invite = Qr::encode(&i.link).map(|qr| (i, qr, expires));
    }

    fn error(&mut self, error: PairError) {
        self.message = Some(
            match error {
                PairError::Expired | PairError::Refused { status: 404, .. } => {
                    "This code is invalid, expired or already used."
                }
                PairError::Refused { code, .. } if code == "too_many_codes" => {
                    "Too many wrong codes. Wait five minutes and try again."
                }
                PairError::Refused { status: 429, .. } => {
                    "Too many requests. Wait a moment and try again."
                }
                _ => "Couldn't reach pairing. Try again.",
            }
            .into(),
        );
    }

    fn create(&mut self, cx: &mut Context<Self>) {
        let Some((rt, t)) = self.net.clone() else {
            return;
        };
        if self.busy || self.invite.is_some() {
            return;
        }
        self.busy = true;
        self.message = None;
        cx.notify();
        let start = session::spawn(&rt, {
            let t = t.clone();
            async move { pair::create(&t, t.base_url()).await }
        });
        cx.spawn(async move |this, cx| {
            let result = start.await.unwrap_or(Err(PairError::Expired));
            let id = this
                .update(cx, |v, cx| {
                    v.busy = false;
                    let id = match result {
                        Ok(i) => {
                            v.show(i);
                            v.invite
                                .as_ref()
                                .map(|(i, _, deadline)| (i.id.clone(), *deadline))
                        }
                        Err(e) => {
                            v.error(e);
                            None
                        }
                    };
                    cx.notify();
                    id
                })
                .ok()
                .flatten();
            let Some((id, deadline)) = id else { return };
            let _ = this.update(cx, |_, cx| {
                let id = id.clone();
                cx.spawn(async move |this, cx| {
                    loop {
                        let now = SystemTime::now();
                        let message = pair::status_message(None, false, until_expiry_ms(deadline));
                        let current = this
                            .update(cx, |v, cx| {
                                if !v.current(&id) {
                                    return false;
                                }
                                if let Some(message) = message {
                                    v.finish(&id, message, cx);
                                    return false;
                                }
                                cx.notify();
                                true
                            })
                            .unwrap_or(false);
                        if !current {
                            break;
                        }
                        let wait = match deadline.duration_since(now) {
                            Ok(left) if !left.is_zero() => left.min(Duration::from_millis(
                                u64::from(theme::motion::PAIRING_TICK),
                            )),
                            _ => (deadline + pair::STATUS_GRACE)
                                .duration_since(now)
                                .unwrap_or_default(),
                        };
                        cx.background_executor().timer(wait).await;
                    }
                })
                .detach();
            });
            loop {
                let left = deadline
                    .duration_since(SystemTime::now())
                    .unwrap_or_default();
                let wait = if left.is_zero() {
                    pair::STATUS_INTERVAL
                } else {
                    left.min(pair::STATUS_INTERVAL)
                };
                cx.background_executor().timer(wait).await;
                if !this.update(cx, |v, _| v.current(&id)).unwrap_or(false) {
                    break;
                }
                if let Some(message) = pair::status_message(None, false, until_expiry_ms(deadline))
                {
                    let _ = this.update(cx, |v, cx| v.finish(&id, message, cx));
                    break;
                }
                let poll = {
                    let t = t.clone();
                    let id = id.clone();
                    session::spawn(&rt, async move { pair::status(&t, &id).await })
                };
                let message = match poll.await {
                    Ok(result) => pair::status_result_message(&result, until_expiry_ms(deadline)),
                    Err(_) => pair::status_message(None, false, until_expiry_ms(deadline)),
                };
                let Some(message) = message else { continue };
                let _ = this.update(cx, |v, cx| v.finish(&id, message, cx));
                break;
            }
        })
        .detach();
    }

    fn current(&self, id: &str) -> bool {
        self.invite.as_ref().is_some_and(|(i, _, _)| i.id == id)
    }

    fn finish(&mut self, id: &str, message: String, cx: &mut Context<Self>) {
        if self.current(id) {
            self.invite = None;
            self.message = Some(message.into());
            cx.notify();
        }
    }

    fn close(&self, cx: &mut Context<Self>) {
        cx.emit(Closed);
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        let Some((rt, t)) = self.net.clone() else {
            return;
        };
        if self.busy {
            return;
        }
        let Some((i, _, _)) = &self.invite else {
            return;
        };
        let id = i.id.clone();
        self.busy = true;
        cx.notify();
        let cancel = session::spawn(&rt, {
            let id = id.clone();
            async move { pair::cancel(&t, &id).await }
        });
        cx.spawn(async move |this, cx| {
            let result = cancel.await.unwrap_or(Err(PairError::Expired));
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                if !v.current(&id) {
                    cx.notify();
                    return;
                }
                match result {
                    Ok(()) => {
                        v.invite = None;
                        v.message = Some("Invite cancelled.".into());
                    }
                    Err(PairError::Refused { status: 404, .. }) => {} // The status check may still report a join.
                    Err(e) => v.error(e),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn review(&mut self, approve: bool, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some((rt, t)) = self.net.clone() else {
            return;
        };
        let input = if approve {
            self.pending
                .as_ref()
                .map(|(code, d)| (code.clone(), Some(d.created_at)))
        } else {
            pair::approval_code(self.input.read(cx).content.as_ref(), t.base_url())
                .ok()
                .map(|code| (code, None))
        };
        let Some((code, at)) = input else {
            self.message = Some("Enter the code shown on the new device.".into());
            cx.notify();
            return;
        };
        self.busy = true;
        self.message = None;
        cx.notify();
        let sent = session::spawn(
            &rt,
            async move { (pair::review(&t, &code, at).await, code) },
        );
        cx.spawn(async move |this, cx| {
            let (result, code) = sent
                .await
                .unwrap_or_else(|_| (Err(PairError::Expired), String::new()));
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match result {
                    Ok(d) if approve => {
                        v.pending = None;
                        v.input
                            .update(cx, |field, cx| field.set_text(String::new(), cx));
                        v.message = Some(format!("Approved {}.", d.device_name).into());
                    }
                    Ok(d) => v.pending = Some((code, d)),
                    Err(e) => {
                        v.pending = None;
                        v.error(e);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn live_invite(&self) -> bool {
        self.invite
            .as_ref()
            .is_some_and(|(_, _, deadline)| SystemTime::now() < *deadline)
    }

    fn focus_order(&self, cx: &App) -> Vec<FocusHandle> {
        let handle = |control: Control| self.buttons[control as usize].clone();
        let mut order = if self.invite_mode {
            if self.live_invite() {
                vec![handle(Control::Copy), handle(Control::Cancel)]
            } else if self.invite.is_none() {
                vec![handle(Control::Create)]
            } else {
                vec![]
            }
        } else if self.pending.is_some() {
            vec![handle(Control::Approve), handle(Control::Another)]
        } else {
            vec![
                self.input.read(cx).focus_handle.clone(),
                handle(Control::Review),
            ]
        };
        order.push(handle(Control::Close));
        order
    }

    fn activate(&mut self, control: Control, window: &mut Window, cx: &mut Context<Self>) {
        match control {
            Control::Create => self.create(cx),
            Control::Copy if self.live_invite() => {
                if let Some((i, _, _)) = &self.invite {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(i.link.clone()));
                }
            }
            Control::Copy => {}
            Control::Cancel => self.cancel(cx),
            Control::Review => self.review(false, cx),
            Control::Approve => self.review(true, cx),
            Control::Another if !self.busy => {
                self.pending = None;
                self.input.update(cx, |i, cx| i.set_text(String::new(), cx));
                let handle = self.input.read(cx).focus_handle.clone();
                window.focus(&handle, cx);
                cx.notify();
            }
            Control::Another => {}
            Control::Close => self.close(cx),
        }
    }

    fn control(
        &self,
        control: Control,
        id: &'static str,
        label: &'static str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let handle = &self.buttons[control as usize];
        let button = match control {
            Control::Create | Control::Approve | Control::Review => kit::button(&self.p, label),
            _ => kit::button_quiet(&self.p, label),
        };
        div()
            .id(id)
            .track_focus(handle)
            .role(gpui::Role::Button)
            .aria_label(label)
            .rounded(px(radius::BUTTON))
            .border(px(elevation::HAIRLINE))
            .border_color(if handle.is_focused(window) {
                self.p.accent
            } else {
                self.p.pane
            })
            .child(button)
            .on_click(cx.listener(move |this, _, window, cx| {
                window.focus(&this.buttons[control as usize], cx);
                this.activate(control, window, cx);
                cx.notify();
            }))
            .on_key_down(
                cx.listener(move |this, e: &gpui::KeyDownEvent, window, cx| {
                    if !e.keystroke.modifiers.modified()
                        && matches!(e.keystroke.key.as_str(), "enter" | "space")
                    {
                        cx.stop_propagation();
                        if !e.is_held {
                            this.activate(control, window, cx);
                        }
                    }
                }),
            )
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let order = self.focus_order(cx);
        if self.net.is_some() && !order.iter().any(|handle| handle.is_focused(window)) {
            window.focus(&order[0], cx);
        }
        let p = self.p;
        let mut body =
            kit::overlay(&p)
                .id("device-pairing-card")
                .track_focus(&self.focus)
                .role(gpui::Role::Dialog)
                .aria_label(if self.invite_mode {
                    "Pair a new device"
                } else {
                    "Approve a device"
                })
                .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                    if super::navigation_tab(e) {
                        let order = this.focus_order(cx);
                        let focused = order.iter().position(|handle| handle.is_focused(window));
                        window.focus(
                            &order[tab_index(focused, !e.keystroke.modifiers.shift, order.len())],
                            cx,
                        );
                        cx.notify();
                        cx.stop_propagation();
                    } else if e.keystroke.key == "escape" && !e.is_held {
                        this.close(cx);
                        cx.stop_propagation();
                    }
                    // Windows needs unhandled printable KeyDown events to produce WM_CHAR.
                }))
                .w(px(desktop::DETAIL + space::S10))
                .max_w_full()
                .max_h_full()
                .overflow_y_scroll()
                .occlude()
                .p(px(space::S6))
                .flex()
                .flex_col()
                .gap(px(space::S3))
                .child(div().style(ty::DETAIL_TITLE).text_color(p.ink).child(
                    if self.invite_mode {
                        "Pair a new device"
                    } else {
                        "Approve a device"
                    },
                ));
        if self.invite_mode {
            if let Some((invite, qr, expires)) = &self.invite {
                let left = expires
                    .duration_since(SystemTime::now())
                    .unwrap_or_default();
                if left.is_zero() {
                    body = body.child(pair::CHECKING_STATUS);
                } else {
                    let remaining = left.as_secs() + u64::from(left.subsec_nanos() > 0);
                    body = body
                        .child("Scan on your new device, type the code in its app, or open the link in its browser.")
                        .child(pairing_qr::tile(qr.clone()))
                        .child(div().style(ty::PAIRING_CODE).child(invite.code.clone()))
                        .child(div().style(ty::META).overflow_hidden().text_ellipsis().whitespace_nowrap().child(invite.link.clone()))
                        .child(div().style(ty::SUMMARY_FIGURES).child(format!("Expires in {}:{:02}", remaining / 60, remaining % 60)))
                        .child(div().flex().gap(px(space::S2))
                            .child(self.control(Control::Copy, "copy-invite", "Copy link", window, cx))
                            .child(self.control(Control::Cancel, "cancel-invite", "Cancel invite", window, cx)));
                }
            } else {
                body = body.child(self.control(
                    Control::Create,
                    "create-invite",
                    "Create invite",
                    window,
                    cx,
                ));
            }
        } else if let Some((code, d)) = &self.pending {
            body = body
                .child(format!(
                    "Approve “{}”? Only continue if the code matches your own device.",
                    d.device_name
                ))
                .child(
                    div()
                        .style(ty::PAIRING_CODE)
                        .child(pair::grouped_approval_code(code)),
                )
                .child(format!(
                    "Source address: {}",
                    d.source.as_deref().unwrap_or("Unknown")
                ))
                .child(
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .child(self.control(
                            Control::Approve,
                            "approve-device",
                            "Approve",
                            window,
                            cx,
                        ))
                        .child(self.control(
                            Control::Another,
                            "another-pairing-code",
                            "Use another code",
                            window,
                            cx,
                        )),
                );
        } else {
            body = body
                .child(kit::label(&p, "Code shown on the device"))
                .child(
                    div()
                        .rounded(px(radius::BUTTON))
                        .border(px(elevation::HAIRLINE))
                        .border_color(p.control)
                        .p(px(space::S2))
                        .child(self.input.clone()),
                )
                .child(self.control(Control::Review, "review-device", "Review", window, cx));
        }
        body.children(self.message.clone())
            .when(self.busy, |s| s.child("Contacting server…"))
            .child(self.control(Control::Close, "close-device-pairing", "Close", window, cx))
    }
}

impl ShellView {
    pub(super) fn open_pairing_panel(
        &mut self,
        invite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = if invite {
            self.invite_panel.clone()
        } else {
            None
        };
        let panel = saved.unwrap_or_else(|| {
            let net = self
                .store
                .session()
                .map(crate::session::Session::pairing_transport);
            let panel = cx.new(|cx| Panel::new(invite, net, self.palette, window, cx));
            cx.subscribe_in(&panel, window, |this, source, _: &Closed, window, cx| {
                if this.pairing_panel.as_ref() == Some(source) {
                    this.pairing_panel = None;
                    window.focus(&this.account_focus, cx);
                    cx.notify();
                }
            })
            .detach();
            if invite {
                self.invite_panel = Some(panel.clone());
            }
            panel
        });
        panel.update(cx, |p, cx| {
            p.p = self.palette;
            cx.notify();
        });
        if invite {
            let handle = panel.read(cx).focus.clone();
            window.focus(&handle, cx);
        }
        self.pairing_panel = Some(panel);
        cx.notify();
    }

    pub(super) fn pairing_overlay(&self) -> Option<Div> {
        let panel = self.pairing_panel.as_ref()?;
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .p(px(space::S4))
                .bg(self.palette.scrim)
                .child(panel.clone()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestApp, TestAppWindow};
    use std::rc::Rc;

    fn shell(app: &mut TestApp) -> TestAppWindow<ShellView> {
        app.update(|cx| {
            cx.bind_keys(crate::ui::input::key_bindings());
            cx.bind_keys(super::super::key_bindings());
        });
        app.open_window(|window, cx| {
            let fixture = crate::fixtures::my_day();
            let mut shell = ShellView::new(
                Rc::new(Store::fixture(&fixture)),
                fixture.ui,
                Theme::Light,
                None,
                window,
                cx,
            );
            shell.still();
            shell
        })
    }

    fn offline_transport() -> (Runtime, HttpTransport) {
        let rt = session::runtime().expect("runtime");
        // This server never handles an HTTP request; its listener only reserves the test address.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("local port");
        let url = format!("http://{}", listener.local_addr().expect("address"));
        let transport = HttpTransport::new(&url, "unused").expect("transport");
        drop(listener);
        (rt, transport)
    }

    #[test]
    fn typed_characters_reach_the_approval_input_and_enter_submits() {
        let mut app = TestApp::new();
        let mut window = shell(&mut app);
        let panel = window.update(|shell, window, cx| {
            shell.open_pairing_panel(false, window, cx);
            let panel = shell.pairing_panel.clone().expect("panel");
            panel.update(cx, |p, cx| {
                p.pending = None;
                p.net = Some(offline_transport());
                cx.notify();
            });
            panel
        });
        window.draw();
        // GPUI injects each text character only if the corresponding KeyDown propagates,
        // matching the Windows backend's KeyDown / TranslateMessage split.
        window.simulate_input("abcdefgh");
        app.read_entity(&panel, |p, cx| {
            assert_eq!(p.input.read(cx).content.as_ref(), "abcdefgh")
        });
        window.simulate_keystroke("enter");
        app.read_entity(&panel, |p, _| {
            assert!(p.busy || p.message.as_deref() == Some("Couldn't reach pairing. Try again."))
        });
    }

    #[test]
    fn pairing_modal_blocks_shell_shortcuts_without_moving_focus() {
        let mut app = TestApp::new();
        let mut window = shell(&mut app);
        let task = window.update(|shell, window, cx| {
            let id = shell.ui.task.expect("selected task");
            shell.toggle_complete(id, cx);
            assert!(shell.last_undo.is_some());
            shell.open_pairing_panel(true, window, cx);
            id
        });
        window.draw();
        let before = window.update(|shell, window, cx| {
            (
                shell.ui.room,
                shell.store.view().tasks[&task].is_done(),
                window.focused(cx),
            )
        });
        window.simulate_keystrokes("ctrl-z ctrl-h ctrl-k ctrl-n");
        window.update(|shell, window, cx| {
            assert_eq!(shell.ui.room, before.0);
            assert_eq!(shell.store.view().tasks[&task].is_done(), before.1);
            assert_eq!(window.focused(cx), before.2);
            assert!(shell.last_undo.is_some());
        });
    }

    #[test]
    fn close_and_escape_keep_a_live_invite_even_when_offline() {
        let mut app = TestApp::new();
        let mut window = shell(&mut app);
        let panel = window.update(|shell, window, cx| {
            shell.open_pairing_panel(true, window, cx);
            let panel = shell.pairing_panel.clone().expect("panel");
            panel.update(cx, |p, cx| {
                p.net = Some(offline_transport());
                cx.notify();
            });
            panel
        });
        window.draw();
        window.simulate_keystroke("escape");
        window.update(|shell, window, cx| {
            assert!(shell.pairing_panel.is_none());
            assert!(shell.account_focus.is_focused(window));
            shell.open_pairing_panel(true, window, cx);
            assert_eq!(shell.pairing_panel.as_ref(), Some(&panel));
        });
        window.draw();
        window.update(|_, window, cx| {
            window.focus(&panel.read(cx).buttons[Control::Close as usize].clone(), cx)
        });
        window.draw();
        window.simulate_keystroke("enter");
        window.read(|shell, _| assert!(shell.pairing_panel.is_none()));
        app.read_entity(&panel, |p, _| {
            assert!(p.live_invite());
            assert!(!p.busy);
        });
    }

    #[test]
    fn tab_and_shift_tab_wrap_inside_invite_controls() {
        // Copy, Cancel, Close.
        assert_eq!(tab_index(None, true, 3), 0);
        assert_eq!(tab_index(Some(0), true, 3), 1);
        assert_eq!(tab_index(Some(2), true, 3), 0);
        assert_eq!(tab_index(Some(0), false, 3), 2);
        assert_eq!(tab_index(Some(1), false, 3), 0);
    }
}
