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
                InputEvent::Cancel => cx.emit(Closed),
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
                            let id = i.id.clone();
                            v.show(i);
                            Some(id)
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
            let Some(id) = id else { return };
            let mut next_poll = Instant::now() + pair::STATUS_INTERVAL;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(u64::from(
                        theme::motion::PAIRING_TICK,
                    )))
                    .await;
                if !this
                    .update(cx, |v, cx| {
                        let Some((i, _, expires)) = &v.invite else {
                            return false;
                        };
                        if i.id != id {
                            return false;
                        }
                        if *expires <= Instant::now() {
                            v.invite = None;
                            v.message = Some("Invite expired. Make a new one if needed.".into());
                            cx.notify();
                            return false;
                        }
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                {
                    break;
                }
                if Instant::now() < next_poll {
                    continue;
                }
                next_poll = Instant::now() + pair::STATUS_INTERVAL;
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
                    Ok(Err(PairError::Refused { status: 404, .. })) => {
                        "The invite is no longer available.".into()
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
        let cancel = session::spawn(&rt, async move { pair::cancel(&t, &id).await });
        cx.spawn(async move |this, cx| {
            let result = cancel.await.unwrap_or(Err(PairError::Expired));
            let _ = this.update(cx, |v, cx| {
                v.busy = false;
                match result {
                    Ok(()) => {
                        v.invite = None;
                        v.message = Some("Invite cancelled.".into());
                    }
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
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending.is_some() && !self.focus.is_focused(window) {
            window.focus(&self.focus, cx);
        }
        let p = self.p;
        let mut body =
            kit::overlay(&p)
                .id("device-pairing-card")
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, _, cx| {
                    match e.keystroke.key.as_str() {
                        "escape" => cx.emit(Closed),
                        "enter" if !e.is_held && this.invite_mode && this.invite.is_none() => {
                            this.create(cx)
                        }
                        "enter" if !e.is_held && this.pending.is_some() => this.review(true, cx),
                        _ => return,
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
                let remaining = expires
                    .saturating_duration_since(Instant::now())
                    .as_secs()
                    .saturating_add(1)
                    .min(u64::from(invite.expires_in));
                let link = invite.link.clone();
                body = body
                    .child("Scan on your new device, type the code in its app, or open the link in its browser.")
                    .child(pairing_qr::tile(qr.clone()))
                    .child(div().style(ty::PAIRING_CODE).child(invite.code.clone()))
                    .child(div().style(ty::META).overflow_hidden().text_ellipsis().whitespace_nowrap().child(invite.link.clone()))
                    .child(div().style(ty::SUMMARY_FIGURES).child(format!("Expires in {}:{:02}", remaining / 60, remaining % 60)))
                    .child(div().flex().gap(px(space::S2))
                        .child(div().id("copy-invite").child(kit::button_quiet(&p, "Copy link")).on_click(move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(link.clone()))))
                        .child(div().id("cancel-invite").child(kit::button_quiet(&p, "Cancel invite")).on_click(cx.listener(|this, _, _, cx| this.cancel(cx)))));
            } else {
                body = body.child(
                    div()
                        .id("create-invite")
                        .child(kit::button(&p, "Create invite"))
                        .on_click(cx.listener(|this, _, _, cx| this.create(cx))),
                );
            }
        } else if let Some((_, d)) = &self.pending {
            body = body
                .child(format!(
                    "Approve “{}”? Only continue if the code matches your own device.",
                    d.device_name
                ))
                .child(
                    div()
                        .flex()
                        .gap(px(space::S2))
                        .child(
                            div()
                                .id("approve-device")
                                .child(kit::button(&p, "Approve"))
                                .on_click(cx.listener(|this, _, _, cx| this.review(true, cx))),
                        )
                        .child(
                            div()
                                .id("another-pairing-code")
                                .child(kit::button_quiet(&p, "Use another code"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if !this.busy {
                                        this.pending = None;
                                        this.input
                                            .update(cx, |i, cx| i.set_text(String::new(), cx));
                                        let handle = this.input.read(cx).focus_handle.clone();
                                        window.focus(&handle, cx);
                                        cx.notify();
                                    }
                                })),
                        ),
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
                .child(
                    div()
                        .id("review-device")
                        .child(kit::button(&p, "Review"))
                        .on_click(cx.listener(|this, _, _, cx| this.review(false, cx))),
                );
        }
        body.children(self.message.clone())
            .when(self.busy, |s| s.child("Contacting server…"))
            .child(
                div()
                    .id("close-device-pairing")
                    .child(kit::button_quiet(&p, "Close"))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Closed))),
            )
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
