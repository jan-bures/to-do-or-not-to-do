//! First run: connect this computer to the owner's server.
//!
//! The owner types the server's address; the app asks the server for a pairing code
//! (`todo_client::pairing::start`), opens the approval page in the browser and polls until the owner
//! approves it there with a passkey. The token the server then sends is sealed with DPAPI and saved
//! (`config::save`), and the view hands the pairing to the root view, which starts the session.
//!
//! All requests run on the network runtime; the view only awaits their results. Nothing spins while it
//! waits: the code and a line of text say what is happening.

use std::time::{Duration, Instant};

use gpui::{
    App, AsyncApp, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    WeakEntity, Window, div, prelude::*, px,
};
use todo_client::device_pairing::{self, Qr};
use todo_client::native::HttpTransport;
use todo_client::pairing::{self, PairError, Polled, Started};
use todo_core::DeviceId;

use super::input::{InputEvent, TextInput};
use super::kit;
use super::landscape::{LandscapeCache, Sky, landscape};
use super::shell::quick_add_colors;
use crate::config::{self, Config, Pairing, Paths};
use crate::model::Smart;
use crate::session::{self, Runtime};
use crate::theme::{
    self, Icon, Material, Palette, TextStyled, Theme, component, desktop, elevation, radius, space,
    ty,
};

/// The width of the pairing column.
const COLUMN: f32 = space::S10 * 13.;

/// Transient poll failures in a row before the view gives up and asks the owner to try again.
const MAX_POLL_FAILURES: u32 = 10;

/// What the view reports.
#[derive(Clone, Debug)]
pub enum PairingEvent {
    /// Approved and saved.
    Paired(Pairing),
}

/// Where pairing is.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    /// Waiting for an address. `error` says why the last attempt failed.
    Enter { error: Option<SharedString> },
    /// Confirm the host taken from a link before making any request.
    ConfirmInvite { host: String },
    /// Contacting the selected origin.
    Starting { server: String, redeeming: bool },
    /// The code is shown; waiting for the owner to approve it.
    Waiting(WaitingStep),
}

#[derive(Clone, Debug, PartialEq)]
struct WaitingStep {
    server: String,
    code: SharedString,
    verify: String,
    expires: Instant,
    /// A transient problem, shown under the code while polling continues.
    note: Option<SharedString>,
}

/// What to do after a poll.
#[derive(Debug, PartialEq)]
enum Next {
    /// Poll again after this long, with an optional note for the owner.
    Again(Duration, Option<SharedString>),
    Approved(DeviceId, String),
    /// Stop and go back to the address, with this message.
    Stop(SharedString),
}

/// Decides what follows a poll. `failures` counts transient failures in a row.
fn after_poll(result: Result<Polled, PairError>, interval: Duration, failures: &mut u32) -> Next {
    match result {
        Ok(Polled::Pending { interval: secs }) => {
            *failures = 0;
            Next::Again(Duration::from_secs(u64::from(secs.max(1))), None)
        }
        Ok(Polled::Approved { device_id, token }) => Next::Approved(device_id, token),
        Ok(Polled::Denied) => Next::Stop("The request was declined on the server.".into()),
        Err(PairError::Expired) => Next::Stop(
            "The code expired before it was approved. Connect again for a new one.".into(),
        ),
        Err(PairError::Refused {
            retry_after: Some(wait),
            ..
        }) => Next::Again(wait.max(interval), None),
        Err(PairError::Refused { status, code, .. }) if status < 500 => {
            Next::Stop(refused(status, &code))
        }
        Err(e) => {
            *failures += 1;
            if *failures >= MAX_POLL_FAILURES {
                Next::Stop(format!("Lost touch with the server ({e}).").into())
            } else {
                // Back off a little with each failure, up to half a minute.
                let wait = interval * (*failures + 1);
                Next::Again(
                    wait.min(Duration::from_secs(30)),
                    Some("Can't reach the server right now; still trying.".into()),
                )
            }
        }
    }
}

/// The approval page to open: built from the address the owner typed (already checked by
/// `config::normalize_server`), never taken from the server's answer, which could name another site or a
/// non-web handler. "Open the page again" reuses it.
fn approval_page(server: &str, started: &Started) -> String {
    pairing::approval_url(server, &started.user_code)
}

fn refused(status: u16, code: &str) -> SharedString {
    match (status, code) {
        (_, "not_configured") => {
            "Pairing isn't set up on that server yet (it needs its public address).".into()
        }
        (404, _) => "That server doesn't offer pairing. Check the address.".into(),
        (429, _) => "Too many pairing requests. Wait a minute and try again.".into(),
        (503, _) => "The server is busy. Try again in a minute.".into(),
        _ => format!("The server refused the request (HTTP {status}).").into(),
    }
}

/// The message for a failed start.
fn start_error(e: &PairError) -> SharedString {
    match e {
        PairError::Transport(_) => {
            "Can't reach that server. Check the address and your connection.".into()
        }
        PairError::Refused {
            status, message, ..
        } if (*status == 400 || *status == 422) && !message.is_empty() => {
            format!("The server said: {message}").into()
        }
        PairError::Refused { status, code, .. } => refused(*status, code),
        PairError::Expired => "The server lost the request. Try again.".into(),
        PairError::Protocol(_) => {
            "That address answered, but not like a to-do server. Check the address.".into()
        }
    }
}

/// Remaining time, rounded up to a whole second.
fn expires_in(left: Duration) -> String {
    let seconds = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    format!("expires in {}:{:02}", seconds / 60, seconds % 60)
}

/// Where the network and the saved pairing live. The screenshot fixture has neither.
#[derive(Clone)]
struct Wiring {
    rt: Runtime,
    paths: Paths,
}

#[allow(missing_debug_implementations)] // Holds GPUI entities.
pub struct PairingView {
    wiring: Option<Wiring>,
    palette: Palette,
    landscape: LandscapeCache,
    input: Entity<TextInput>,
    invite: Entity<TextInput>,
    invite_target: Option<device_pairing::RedeemTarget>,
    confirm_focus: FocusHandle,
    waiting_focus: [FocusHandle; 3],
    step: Step,
    scroll: kit::ScrollPane,
    /// The name the owner sees when approving.
    device_name: SharedString,
    /// Bumped by every new attempt and by Cancel, so a stale poll loop stops.
    attempt: u64,
}

impl EventEmitter<PairingEvent> for PairingView {}

impl PairingView {
    fn show_step(&mut self, step: Step) {
        self.scroll.reset_to_top();
        self.step = step;
    }

    /// The live view. `server` prefills the address (the last one used), `error` explains why pairing
    /// is shown again (the server no longer accepts the token, say).
    pub fn new(
        rt: Runtime,
        paths: Paths,
        theme: Theme,
        server: Option<String>,
        error: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(theme, config::device_name().into(), window, cx);
        this.wiring = Some(Wiring { rt, paths });
        this.show_step(Step::Enter { error });
        if let Some(s) = server {
            this.input.update(cx, |i, cx| i.set_text(s, cx));
        }
        let focus = this.input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        this
    }

    /// The screenshot fixtures: `waiting` shows the code, otherwise the address being typed. No network.
    pub fn fixture(
        theme: Theme,
        server: &str,
        code: &str,
        waiting: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::build(theme, "To-do on WORKSTATION".into(), window, cx);
        let host = server.trim_start_matches("https://").to_string();
        if waiting {
            this.show_step(Step::Waiting(WaitingStep {
                server: server.into(),
                code: code.to_string().into(),
                verify: pairing::approval_url(server, code),
                expires: Instant::now() + Duration::from_secs(600),
                note: None,
            }));
        } else {
            this.input.update(cx, |i, cx| i.set_text(host, cx));
        }
        this
    }

    fn build(
        theme: Theme,
        device_name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let palette = theme::palette_with_material(theme, Smart::MyDay.hue(), Material::system());
        let input = cx.new(|cx| TextInput::new("todo.example.com", quick_add_colors(&palette), cx));
        cx.subscribe_in(
            &input,
            window,
            |this, _, event: &InputEvent, _window, cx| match event {
                InputEvent::Submit => this.connect(cx),
                InputEvent::Changed => {
                    if let Step::Enter { error } = &mut this.step {
                        *error = None;
                    }
                    cx.notify();
                }
                InputEvent::Cancel => {}
            },
        )
        .detach();
        let invite =
            cx.new(|cx| TextInput::new("Invite code or link", quick_add_colors(&palette), cx));
        cx.subscribe_in(&invite, window, |this, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Submit) {
                this.redeem(cx);
            }
        })
        .detach();
        Self {
            wiring: None,
            palette,
            landscape: LandscapeCache::default(),
            input,
            invite,
            invite_target: None,
            confirm_focus: cx.focus_handle(),
            waiting_focus: std::array::from_fn(|_| cx.focus_handle()),
            step: Step::Enter { error: None },
            scroll: kit::ScrollPane::default(),
            device_name,
            attempt: 0,
        }
    }

    pub fn set_material(&mut self, material: Material, cx: &mut Context<Self>) {
        if self.palette.material != material {
            self.palette =
                theme::palette_with_material(self.palette.theme, Smart::MyDay.hue(), material);
            let colors = quick_add_colors(&self.palette);
            for input in [&self.input, &self.invite] {
                input.update(cx, |i, cx| {
                    i.colors = colors;
                    cx.notify();
                });
            }
            cx.notify();
        }
    }

    fn redeem(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.step, Step::Enter { .. }) {
            return;
        }
        let input = self.invite.read(cx).content.to_string();
        let typed = self.input.read(cx).content.to_string();
        let target = match device_pairing::redeem_target(&typed, &input, config::normalize_server) {
            Ok(target) => target,
            Err(error) => {
                self.show_step(Step::Enter {
                    error: Some(error.to_string().into()),
                });
                cx.notify();
                return;
            }
        };
        if let Some(host) = target.confirm_host.clone() {
            self.invite_target = Some(target);
            self.show_step(Step::ConfirmInvite { host });
            cx.notify();
        } else {
            self.send_invite(target, cx);
        }
    }

    fn confirm_invite(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.invite_target.take() {
            self.send_invite(target, cx);
        }
    }

    fn send_invite(&mut self, target: device_pairing::RedeemTarget, cx: &mut Context<Self>) {
        let Some(wiring) = self.wiring.clone() else {
            return;
        };
        let device_pairing::RedeemTarget { server, secret, .. } = target;
        let Ok(transport) = HttpTransport::anonymous(&server) else {
            self.show_step(Step::Enter {
                error: Some("That server address can't be used.".into()),
            });
            cx.notify();
            return;
        };
        self.attempt += 1;
        let attempt = self.attempt;
        self.show_step(Step::Starting {
            server: server.clone(),
            redeeming: true,
        });
        let name = self.device_name.to_string();
        self.invite
            .update(cx, |i, cx| i.set_text(String::new(), cx));
        cx.notify();
        let result = session::spawn(&wiring.rt, async move {
            device_pairing::redeem(&transport, &secret, &name).await
        });
        cx.spawn(async move |this, cx| {
            let next = match result.await {
                Ok(Ok(credential)) => Next::Approved(credential.device_id, credential.token),
                Ok(Err(PairError::Refused { status: 404, .. })) => {
                    Next::Stop("This invite is invalid, expired or already used.".into())
                }
                Ok(Err(e)) => Next::Stop(start_error(&e)),
                Err(_) => Next::Stop("Couldn't reach the server.".into()),
            };
            let _ = this.update(cx, |v, cx| {
                v.after(attempt, next, &server, &wiring.paths, cx)
            });
        })
        .detach();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.step, Step::Enter { .. }) {
            return;
        }
        let Some(wiring) = self.wiring.clone() else {
            return;
        };
        let typed = self.input.read(cx).content.to_string();
        let server = match config::normalize_server(&typed) {
            Ok(s) => s,
            Err(why) => {
                self.show_step(Step::Enter {
                    error: Some(why.into()),
                });
                cx.notify();
                return;
            }
        };
        let transport = match HttpTransport::anonymous(&server) {
            Ok(t) => t,
            Err(e) => {
                self.show_step(Step::Enter {
                    error: Some(format!("That address can't be used: {e}").into()),
                });
                cx.notify();
                return;
            }
        };
        self.attempt += 1;
        let attempt = self.attempt;
        self.show_step(Step::Starting {
            server: server.clone(),
            redeeming: false,
        });
        cx.notify();
        let name = self.device_name.to_string();
        let started = {
            let transport = transport.clone();
            session::spawn(&wiring.rt, async move {
                pairing::start(&transport, &name).await
            })
        };
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let started = match started.await {
                Ok(r) => r,
                Err(e) => Err(PairError::Protocol(e.to_string())),
            };
            let started: Started = match started {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("pairing start failed");
                    let _ = this.update(cx, |v, cx| {
                        if v.attempt == attempt {
                            v.show_step(Step::Enter {
                                error: Some(start_error(&e)),
                            });
                            cx.notify();
                        }
                    });
                    return;
                }
            };
            let interval = started.interval();
            let poll_token = started.poll_token.clone();
            let ok = this.update(cx, |v, cx| {
                if v.attempt != attempt {
                    return false;
                }
                // Never the server's `verification_uri`: the page comes from the checked origin.
                let verify = approval_page(&server, &started);
                v.show_step(Step::Waiting(WaitingStep {
                    server: server.clone(),
                    code: started.user_code.clone().into(),
                    verify,
                    expires: Instant::now() + Duration::from_secs(u64::from(started.expires_in)),
                    note: None,
                }));
                cx.notify();
                true
            });
            if !matches!(ok, Ok(true)) {
                return;
            }
            let ticker = this.clone();
            cx.spawn(async move |cx: &mut AsyncApp| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(u64::from(
                            theme::motion::PAIRING_TICK,
                        )))
                        .await;
                    if !ticker
                        .update(cx, |view, cx| {
                            if view.attempt == attempt && matches!(view.step, Step::Waiting(_)) {
                                cx.notify();
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false)
                    {
                        return;
                    }
                }
            })
            .detach();
            let mut wait = interval;
            let mut failures = 0;
            loop {
                cx.background_executor().timer(wait).await;
                let live = this
                    .update(cx, |v, _| v.attempt == attempt)
                    .unwrap_or(false);
                if !live {
                    return;
                }
                let polled = {
                    let transport = transport.clone();
                    let token = poll_token.clone();
                    session::spawn(&wiring.rt, async move {
                        pairing::poll(&transport, &token).await
                    })
                    .await
                };
                let polled = polled.unwrap_or_else(|e| Err(PairError::Protocol(e.to_string())));
                let next = after_poll(polled, interval, &mut failures);
                let done = this
                    .update(cx, |v, cx| {
                        v.after(attempt, next, &server, &wiring.paths, cx)
                    })
                    .unwrap_or(None);
                match done {
                    Some(w) => wait = w,
                    None => return,
                }
            }
        })
        .detach();
    }

    /// Applies a poll's outcome. Returns how long to wait before the next poll, or `None` to stop.
    fn after(
        &mut self,
        attempt: u64,
        next: Next,
        server: &str,
        paths: &Paths,
        cx: &mut Context<Self>,
    ) -> Option<Duration> {
        if self.attempt != attempt {
            return None;
        }
        let wait = match next {
            Next::Again(wait, note) => {
                if let Step::Waiting(waiting) = &mut self.step {
                    waiting.note = note;
                }
                Some(wait)
            }
            Next::Approved(device, token) => {
                let pairing = Pairing {
                    // A fresh pairing ID: this pairing's data starts empty.
                    config: Config::new(server, device),
                    token,
                };
                match config::save(paths, &pairing) {
                    Ok(()) => cx.emit(PairingEvent::Paired(pairing)),
                    Err(e) => {
                        tracing::error!("saving the pairing failed: {e:#}");
                        self.show_step(Step::Enter {
                            error: Some(
                                format!("Approved, but saving the sign-in failed: {e}").into(),
                            ),
                        });
                    }
                }
                None
            }
            Next::Stop(why) => {
                self.show_step(Step::Enter { error: Some(why) });
                None
            }
        };
        cx.notify();
        wait
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.invite_target = None;
        self.attempt += 1;
        self.show_step(Step::Enter { error: None });
        let focus = self.input.read(cx).focus_handle.clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn enter(&self, error: Option<&SharedString>, busy: bool, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let focused = !busy;
        let field = div()
            .h(px(desktop::QUICK_ADD))
            .flex()
            .items_center()
            .gap(px(space::S3))
            .pl(px(space::S3 + 2.))
            .pr(px(space::S2 + 2.))
            .rounded(px(radius::QUICK_ADD))
            .border(px(elevation::HAIRLINE))
            .border_color(if focused { p.accent } else { p.control })
            .bg(p.raised)
            .style(ty::QUICK_ADD)
            .child(kit::icon(Icon::Link, component::ICON, p.accent_strong))
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .child(kit::kbd(&p, "Enter"));
        let field = div()
            .mx(px(-elevation::FOCUS_RING))
            .p(px(elevation::FOCUS_RING))
            .rounded(px(radius::QUICK_ADD + elevation::FOCUS_RING))
            .bg(if focused {
                p.tint
            } else {
                gpui::transparent_black()
            })
            .child(field);
        let button = if busy {
            kit::button(&p, "Connecting…").opacity(0.6)
        } else {
            kit::button(&p, "Connect")
        };
        div()
            .flex()
            .flex_col()
            .child(kit::label(&p, "Server").mb(px(space::S2)))
            .child(field)
            .child(
                div()
                    .mt(px(space::S3))
                    .style(ty::BODY)
                    .text_color(p.ink_3)
                    .child("The address your server runs at. You'll approve this computer there with your passkey."),
            )
            .children(error.map(|e| {
                div()
                    .mt(px(space::S3))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .style(ty::BODY)
                    .text_color(p.danger_ink)
                    .child(kit::icon(Icon::CircleAlert, component::ICON_SMALL, p.danger))
                    .child(e.clone())
            }))
            .child(
                div()
                    .mt(px(space::S6))
                    .flex()
                    .items_center()
                    .gap(px(space::S4))
                    .child(
                        div()
                            .id("connect")
                            .child(button)
                            .when(!busy, |b| b.on_click(cx.listener(|this, _, _, cx| this.connect(cx)))),
                    )
                    .child(
                        div()
                            .style(ty::META)
                            .text_color(p.ink_3)
                            .child(format!("This computer will appear as “{}”.", self.device_name)),
                    ),
            )
            .when(!busy, |body| body.child(div().mt(px(space::S6)).child(kit::label(&p, "Or use an invite"))
                .child(div().flex().items_center().gap(px(space::S2)).mt(px(space::S2))
                    .child(div().flex_1().min_w_0().px(px(space::S3)).py(px(space::S2)).rounded(px(radius::BUTTON)).border(px(elevation::HAIRLINE)).border_color(p.control).child(self.invite.clone()))
                    .child(div().id("redeem-invite").child(kit::button_quiet(&p, "Use invite")).on_click(cx.listener(|this, _, _, cx| this.redeem(cx)))))))
    }

    fn waiting_button(
        &self,
        id: &'static str,
        label: &'static str,
        index: usize,
        window: &Window,
        activate: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focus = &self.waiting_focus[index];
        let key_activate = activate.clone();
        div()
            .id(id)
            .track_focus(focus)
            .role(gpui::Role::Button)
            .aria_label(label)
            .rounded(px(radius::BUTTON))
            .border(px(elevation::HAIRLINE))
            .border_color(if focus.is_focused(window) {
                self.palette.accent
            } else {
                self.palette.pane
            })
            .child(kit::button_quiet(&self.palette, label))
            .on_click(cx.listener(move |this, _, window, cx| activate(this, window, cx)))
            .on_key_down(
                cx.listener(move |this, e: &gpui::KeyDownEvent, window, cx| {
                    if !e.keystroke.modifiers.modified()
                        && matches!(e.keystroke.key.as_str(), "enter" | "space")
                    {
                        cx.stop_propagation();
                        if !e.is_held {
                            key_activate(this, window, cx);
                        }
                    }
                }),
            )
    }

    fn waiting(&self, waiting: &WaitingStep, window: &Window, cx: &mut Context<Self>) -> Div {
        let WaitingStep {
            server,
            code,
            verify,
            expires,
            note,
        } = waiting;
        let p = self.palette;
        let host = server
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string();
        let left = expires.saturating_duration_since(Instant::now());
        let status = match note {
            Some(n) => n.clone(),
            None => format!("Waiting for approval · {}", expires_in(left)).into(),
        };
        let qr = Qr::encode(verify);
        let copy = verify.to_owned();
        let verify = verify.to_string();
        div()
            .flex()
            .flex_col()
            .child(kit::label(&p, "Your code"))
            .child(
                div().flex().items_center().gap(px(space::S4)).mt(px(space::S2))
                    .children(qr.map(super::pairing_qr::tile))
                    .child(div().style(ty::PAIRING_CODE).text_color(p.ink).child(code.clone())),
            )
            .child(
                div()
                    .mt(px(space::S3))
                    .style(ty::BODY)
                    .text_color(p.ink_2)
                    .child(format!(
                        "Scan with your signed-in phone or enter the code on a signed-in device. You can also approve on {host} in your browser."
                    )),
            )
            .child(
                div()
                    .mt(px(space::S3))
                    .flex()
                    .items_start()
                    .gap(px(space::S2))
                    .style(ty::SUMMARY_FIGURES)
                    .text_color(if note.is_some() { p.danger_ink } else { p.ink_3 })
                    .child(kit::status_light(
                        if note.is_some() { p.danger } else { p.accent },
                        ty::SUMMARY_FIGURES,
                    ))
                    .child(div().min_w_0().child(status)),
            )
            .child(
                div()
                    .mt(px(space::S6))
                    .flex()
                    .items_center()
                    .gap(px(space::S3))
                    .child(self.waiting_button("open-again", "Open approval page", 0, window, move |_, _, cx| cx.open_url(&verify), cx))
                    .child(self.waiting_button("copy-pairing-link", "Copy link", 1, window, move |_, _, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone())), cx))
                    .child(self.waiting_button("cancel", "Use another address", 2, window, Self::cancel, cx)),
            )
    }
}

impl Focusable for PairingView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle.clone()
    }
}

impl Render for PairingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let (title, body) = match self.step.clone() {
            Step::Enter { error } => ("Connect", self.enter(error.as_ref(), false, cx)),
            Step::ConfirmInvite { host } => {
                if !self.confirm_focus.is_focused(window) {
                    window.focus(&self.confirm_focus, cx);
                }
                (
                    "Confirm server",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(space::S3))
                        .child(format!(
                            "This invite is for {host}. Only continue if you trust this server."
                        ))
                        .child(
                            div()
                                .id("confirm-invite")
                                .track_focus(&self.confirm_focus)
                                .on_key_down(cx.listener(
                                    |this, e: &gpui::KeyDownEvent, window, cx| {
                                        match e.keystroke.key.as_str() {
                                            "enter" | "space" if !e.is_held => {
                                                this.confirm_invite(cx)
                                            }
                                            "escape" => this.cancel(window, cx),
                                            _ => return,
                                        }
                                        cx.stop_propagation();
                                    },
                                ))
                                .child(kit::button(&p, format!("Pair with {host}")))
                                .on_click(cx.listener(|this, _, _, cx| this.confirm_invite(cx))),
                        )
                        .child(
                            div()
                                .id("cancel-invite-confirmation")
                                .child(kit::button_quiet(&p, "Go back"))
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.cancel(window, cx)),
                                ),
                        ),
                )
            }
            Step::Starting { server, redeeming } => (
                "Connect",
                div().child(if redeeming {
                    format!("Finishing pairing with {server}…")
                } else {
                    format!("Connecting to {server}…")
                }),
            ),
            Step::Waiting(waiting) => {
                // The address field has unmounted; without a focused child, Tab would not reach us.
                if self.wiring.is_some() && !self.waiting_focus.iter().any(|f| f.is_focused(window))
                {
                    window.focus(&self.waiting_focus[0], cx);
                }
                ("Approve", self.waiting(&waiting, window, cx))
            }
        };
        // While waiting, every poll re-renders, which keeps the expiry line true.
        let bar = || {
            div()
                .w(px(elevation::HAIRLINE))
                .h(px(ty::CAPS_LABEL.size - 0.5))
                .bg(p.line_strong)
        };
        div()
            .id("pairing")
            .on_key_down(cx.listener(|this, e: &gpui::KeyDownEvent, window, cx| {
                let mods = e.keystroke.modifiers;
                if e.keystroke.key != "tab"
                    || e.is_held
                    || mods.control
                    || mods.alt
                    || mods.platform
                {
                    return;
                }
                let fields;
                let order = match this.step {
                    Step::Enter { .. } => {
                        fields = [
                            this.input.read(cx).focus_handle.clone(),
                            this.invite.read(cx).focus_handle.clone(),
                        ];
                        fields.as_slice()
                    }
                    Step::Waiting(_) => this.waiting_focus.as_slice(),
                    _ => return,
                };
                let current = order.iter().position(|f| f.is_focused(window));
                let next = if mods.shift {
                    current.map_or(order.len() - 1, |i| (i + order.len() - 1) % order.len())
                } else {
                    current.map_or(0, |i| (i + 1) % order.len())
                };
                window.focus(&order[next], cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .size_full()
            .flex()
            .flex_col()
            .style(ty::BODY)
            .text_color(p.ink)
            .child(
                div()
                    .h(px(desktop::TITLEBAR))
                    .flex_none()
                    .flex()
                    .items_center()
                    .bg(p.surface_sky())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .pl(px(space::S4))
                            .h_full()
                            .window_control_area(gpui::WindowControlArea::Drag)
                            .child(kit::icon(
                                Icon::CircleCheck,
                                component::ICON,
                                p.accent_strong,
                            ))
                            .child(
                                div()
                                    .style(ty::NAV_COLLECTION)
                                    .text_color(p.ink)
                                    .child("To-do"),
                            ),
                    )
                    .child(kit::window_controls(&p, window, kit::CaptionPreview::Live)),
            )
            .child(
                div()
                    .flex_none()
                    .child(
                        div().bg(p.surface_sky()).child(
                            div()
                                .w(px(COLUMN))
                                .mx_auto()
                                .pt(px(space::S10 * 2.))
                                .pb(px(space::S4))
                                .child(
                                    div()
                                        .h(px(desktop::EYEBROW))
                                        .flex()
                                        .items_center()
                                        .gap(px(space::S2 + 2.))
                                        .child(
                                            div()
                                                .style(ty::EYEBROW_DAY)
                                                .text_color(p.accent_strong)
                                                .child("Welcome"),
                                        )
                                        .child(bar())
                                        .child(
                                            div()
                                                .style(ty::EYEBROW_META)
                                                .text_color(p.ink_3)
                                                .child("THIS COMPUTER ISN'T CONNECTED YET"),
                                        ),
                                )
                                .child(div().style(ty::ROOM_TITLE).text_color(p.ink).child(title)),
                        ),
                    )
                    .child(landscape(&self.landscape, Sky::Slim(p), None)),
            )
            .child(div().h(px(elevation::HAIRLINE)).bg(p.line))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .bg(p.surface_pane())
                    .child(
                        self.scroll
                            .viewport("pairing-content")
                            .size_full()
                            .px(px(kit::SCROLLBAR_GUTTER))
                            .child(
                                div()
                                    .w(px(COLUMN))
                                    .max_w_full()
                                    .mx_auto()
                                    .py(px(space::S10))
                                    .child(body),
                            ),
                    )
                    .child(
                        self.scroll
                            .bar
                            .overlay(&self.scroll.handle, &p, self.wiring.is_some()),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: Duration = Duration::from_secs(1);

    #[test]
    fn the_waiting_buttons_are_reachable_and_usable_by_keyboard() {
        let app_url = "https://todo.example.com";
        let code = "ABCD-EFGH";
        let link = pairing::approval_url(app_url, code);
        let mut app = gpui::TestApp::new();
        let mut window = app.open_window(|window, cx| {
            let view = PairingView::fixture(Theme::Light, app_url, code, true, window, cx);
            window.focus(&view.waiting_focus[0], cx);
            view
        });
        window.draw();
        window.simulate_keystroke("shift-tab");
        window.update(|view, window, _| assert!(view.waiting_focus[2].is_focused(window)));
        window.simulate_keystroke("tab");
        window.update(|view, window, _| assert!(view.waiting_focus[0].is_focused(window)));
        window.simulate_keystroke("enter");
        assert_eq!(app.opened_url().as_deref(), Some(link.as_str()));
        window.simulate_keystrokes("tab space");
        assert_eq!(
            app.read_from_clipboard().and_then(|item| item.text()),
            Some(link)
        );
        window.simulate_keystrokes("tab enter");
        window.update(|view, window, cx| {
            assert!(matches!(view.step, Step::Enter { .. }));
            assert!(view.input.read(cx).focus_handle.is_focused(window));
        });
    }

    #[test]
    fn the_approval_page_ignores_the_servers_link() {
        let started: Started = serde_json::from_str(
            r#"{"user_code":"ABCD-EFGH","poll_token":"p","verification_uri":"ms-settings:privacy","expires_in":300,"interval":2}"#,
        )
        .expect("started");
        assert_eq!(
            approval_page("https://todo.example.com", &started),
            "https://todo.example.com/pair?code=ABCD-EFGH"
        );
        let hostile: Started = serde_json::from_str(
            r#"{"user_code":"x\"&next=https://evil.example","poll_token":"p","verification_uri":"https://evil.example/pair","expires_in":300,"interval":2}"#,
        )
        .expect("started");
        let page = approval_page("https://todo.example.com", &hostile);
        assert!(page.starts_with("https://todo.example.com/pair?code="));
        assert!(!page.contains("&next") && !page.contains("evil.example/"));
    }

    #[test]
    fn polling_follows_the_server() {
        let mut f = 0;
        assert_eq!(
            after_poll(Ok(Polled::Pending { interval: 5 }), SEC, &mut f),
            Next::Again(Duration::from_secs(5), None)
        );
        let device = DeviceId::new("d1").expect("id");
        assert_eq!(
            after_poll(
                Ok(Polled::Approved {
                    device_id: device.clone(),
                    token: "t".into()
                }),
                SEC,
                &mut f
            ),
            Next::Approved(device, "t".into())
        );
        assert!(matches!(
            after_poll(Ok(Polled::Denied), SEC, &mut f),
            Next::Stop(_)
        ));
        assert!(matches!(
            after_poll(Err(PairError::Expired), SEC, &mut f),
            Next::Stop(_)
        ));
    }

    #[test]
    fn a_rate_limit_waits_and_a_flaky_network_retries_then_gives_up() {
        let mut f = 0;
        let limited = PairError::Refused {
            status: 429,
            code: "slow_down".into(),
            message: String::new(),
            retry_after: Some(Duration::from_secs(9)),
        };
        assert_eq!(
            after_poll(Err(limited), SEC, &mut f),
            Next::Again(Duration::from_secs(9), None)
        );
        let flaky = || PairError::Protocol("bad gateway".into());
        for i in 1..MAX_POLL_FAILURES {
            match after_poll(Err(flaky()), SEC, &mut f) {
                Next::Again(wait, Some(_)) => assert!(wait <= Duration::from_secs(30), "{i}"),
                other => panic!("expected a retry, got {other:?}"),
            }
        }
        assert!(matches!(
            after_poll(Err(flaky()), SEC, &mut f),
            Next::Stop(_)
        ));
        // A pending answer resets the count.
        let mut f = 3;
        let _ = after_poll(Ok(Polled::Pending { interval: 5 }), SEC, &mut f);
        assert_eq!(f, 0);
    }

    #[test]
    fn a_client_error_stops_at_once() {
        let mut f = 0;
        let gone = PairError::Refused {
            status: 404,
            code: "not_found".into(),
            message: String::new(),
            retry_after: None,
        };
        assert!(matches!(after_poll(Err(gone), SEC, &mut f), Next::Stop(_)));
    }

    #[test]
    fn start_errors_say_what_to_check() {
        let unconfigured = PairError::Refused {
            status: 503,
            code: "not_configured".into(),
            message: String::new(),
            retry_after: None,
        };
        assert!(start_error(&unconfigured).contains("public address"));
        assert!(start_error(&PairError::Protocol("x".into())).contains("not like a to-do server"));
    }

    #[test]
    fn expiry_rounds_up_to_minutes() {
        assert_eq!(expires_in(Duration::from_secs(600)), "expires in 10:00");
        assert_eq!(expires_in(Duration::from_secs(61)), "expires in 1:01");
        assert_eq!(expires_in(Duration::from_millis(29_001)), "expires in 0:30");
    }
}
