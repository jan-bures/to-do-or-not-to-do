//! Account invite/approval card. Only the non-secret status crosses into the rest of the shell.

use super::*;
use crate::session::{self, Runtime};
use crate::ui::pairing_qr;
use std::time::Instant;
use todo_client::device_pairing::{self as pair, Invitation, InviteStatus, PendingDevice, Qr};
use todo_client::native::HttpTransport;
use todo_client::pairing::PairError;

#[allow(missing_debug_implementations)]
pub(super) struct Panel {
    invite_mode: bool,
    net: Option<(Runtime, HttpTransport)>,
    focus: FocusHandle,
    buttons: [FocusHandle; 7],
    input: Entity<TextInput>,
    invite: Option<(Invitation, Qr, Instant)>,
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
        let expires = Instant::now() + Duration::from_secs(u64::from(i.expires_in));
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
                        let left = deadline.saturating_duration_since(Instant::now());
                        cx.background_executor()
                            .timer(left.min(Duration::from_millis(u64::from(
                                theme::motion::PAIRING_TICK,
                            ))))
                            .await;
                        let current = this
                            .update(cx, |v, cx| {
                                let current = v.invite.as_ref().is_some_and(|(i, _, _)| i.id == id);
                                if current {
                                    cx.notify();
                                }
                                current
                            })
                            .unwrap_or(false);
                        if !current || Instant::now() >= deadline {
                            break;
                        }
                    }
                })
                .detach();
            });
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let wait = if left.is_zero() {
                    pair::STATUS_INTERVAL
                } else {
                    left.min(pair::STATUS_INTERVAL)
                };
                cx.background_executor().timer(wait).await;
                if !this
                    .update(cx, |v, _| {
                        v.invite.as_ref().is_some_and(|(i, _, _)| i.id == id)
                    })
                    .unwrap_or(false)
                {
                    break;
                }
                let poll = {
                    let t = t.clone();
                    let id = id.clone();
                    session::spawn(&rt, async move { pair::status(&t, &id).await })
                };
                let message = match poll.await {
                    Ok(Ok(InviteStatus::Used { device_name })) => {
                        format!("{device_name} signed in.")
                    }
                    Ok(Ok(InviteStatus::Cancelled)) => "Invite cancelled.".into(),
                    Ok(Err(PairError::Refused { status: 404, .. }))
                        if Instant::now() >= deadline =>
                    {
                        "Invite expired. Make a new one if needed.".into()
                    }
                    Ok(Err(PairError::Refused { status: 404, .. })) => {
                        "The invite is no longer available.".into()
                    }
                    _ if Instant::now() >= deadline + pair::STATUS_GRACE => {
                        pair::UNKNOWN_STATUS.into()
                    }
                    _ => continue,
                };
                let _ = this.update(cx, |v, cx| {
                    if v.invite.as_ref().is_some_and(|(i, _, _)| i.id == id) {
                        v.invite = None;
                        v.message = Some(message.into());
                        cx.notify();
                    }
                });
                break;
            }
        })
        .detach();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if self.net.is_some()
            && self
                .invite
                .as_ref()
                .is_some_and(|(_, _, deadline)| Instant::now() < *deadline)
        {
            self.cancel(true, cx);
        } else {
            cx.emit(Closed);
        }
    }

    fn cancel(&mut self, close: bool, cx: &mut Context<Self>) {
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
        let cancel = session::spawn(&rt, async move { pair::cancel(&t, &id).await });
        cx.spawn(async move |this, cx| {
            let result = cancel.await.unwrap_or(Err(PairError::Expired));
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match result {
                    Ok(()) => {
                        v.invite = None;
                        v.message = Some("Invite cancelled.".into());
                        if close {
                            cx.emit(Closed);
                        }
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
            .is_some_and(|(_, _, deadline)| Instant::now() < *deadline)
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
            Control::Cancel => self.cancel(false, cx),
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
                    } else if e.keystroke.key == "escape" && !e.is_held {
                        this.close(cx);
                    }
                    cx.stop_propagation();
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
                let left = expires.saturating_duration_since(Instant::now());
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
        let net = self
            .store
            .session()
            .map(crate::session::Session::pairing_transport);
        let panel = cx.new(|cx| Panel::new(invite, net, self.palette, window, cx));
        cx.subscribe_in(&panel, window, |this, _, _: &Closed, window, cx| {
            this.pairing_panel = None;
            window.focus(&this.account_focus, cx);
            cx.notify();
        })
        .detach();
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
    use super::tab_index;

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
