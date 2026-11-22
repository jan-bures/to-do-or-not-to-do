//! Owner settings for pairing; both actions use the same CSRF-aware transport as sync.

use super::*;
use std::time::Duration;
use todo_client::device_pairing::{self as pair, Invitation, InviteStatus, PendingDevice, Qr};
use todo_client::pairing::PairError;
use todo_client::web::sleep;
use wasm_bindgen::JsCast as _;
use wasm_bindgen_futures::spawn_local;

#[component]
pub(super) fn DeviceSettings() -> impl IntoView {
    let dialog = NodeRef::<html::Dialog>::new();
    let mode = RwSignal::new(None::<bool>); // true: invite; false: approve
    let busy = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let verify = RwSignal::new(false);
    let invite = RwSignal::new_local(None::<(Invitation, Qr)>);
    let code = RwSignal::new(String::new());
    let pending = RwSignal::new(None::<(String, PendingDevice)>);
    let left = RwSignal::new(0_u32);
    let notice = RwSignal::new(None::<String>);
    let browser_origin = || {
        web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default()
    };
    let report = move |e: PairError| {
        let message = match e {
            PairError::Refused { code, .. } if code == "step_up_required" => {
                verify.set(true);
                "Verify with your passkey to continue."
            }
            PairError::Refused { status: 429, .. } => {
                "Too many requests. Wait a moment and try again."
            }
            PairError::Refused { status: 404, .. } | PairError::Expired => {
                "This code is invalid, expired or already used."
            }
            _ => "Couldn’t reach pairing. Try again.",
        };
        error.set(Some(message.into()));
    };
    let create = move |_| {
        if busy.get_untracked() || invite.with_untracked(Option::is_some) {
            return;
        }
        let transport = backend::pairing_transport();
        busy.set(true);
        error.set(None);
        notice.set(None);
        spawn_local(async move {
            let result = if let Some(t) = &transport {
                pair::create(t, &browser_origin()).await
            } else {
                #[cfg(feature = "fixtures")]
                {
                    Ok(Invitation {
                        id: "fixture".into(),
                        code: "ABCDEF-GHJKMN".into(),
                        link: format!("https://todo.example.com/pair/invite#{}", "A".repeat(43)),
                        expires_in: 300,
                    })
                }
                #[cfg(not(feature = "fixtures"))]
                {
                    Err(PairError::Expired)
                }
            };
            busy.set(false);
            let created = match result {
                Ok(created) => created,
                Err(e) => {
                    report(e);
                    return;
                }
            };
            let Some(qr) = Qr::encode(&created.link) else {
                return;
            };
            let deadline = js_sys::Date::now() + f64::from(created.expires_in) * 1000.0;
            left.set(created.expires_in);
            invite.set(Some((created.clone(), qr)));
            let mut next_status = 0.0;
            loop {
                sleep(Duration::from_millis(u64::from(
                    todo_design::motion::PAIRING_TICK,
                )))
                .await;
                if !invite.with_untracked(|i| i.as_ref().is_some_and(|(i, _)| i.id == created.id)) {
                    break;
                }
                let now = js_sys::Date::now();
                if now >= deadline {
                    invite.set(None);
                    notice.set(Some("Invite expired. Make a new one if needed.".into()));
                    break;
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                left.set(((deadline - now) / 1000.0).ceil() as u32);
                let Some(t) = &transport else { continue };
                if now < next_status {
                    continue;
                }
                next_status = now + pair::STATUS_INTERVAL.as_secs_f64() * 1000.0;
                let status = pair::status(t, &created.id).await;
                if !invite.with_untracked(|i| i.as_ref().is_some_and(|(i, _)| i.id == created.id)) {
                    break;
                }
                match status {
                    Ok(InviteStatus::Used { device_name }) => {
                        invite.set(None);
                        notice.set(Some(format!("{device_name} signed in.")));
                        break;
                    }
                    Ok(InviteStatus::Cancelled) => {
                        invite.set(None);
                        break;
                    }
                    _ => {}
                }
            }
        });
    };
    let cancel = move |_| {
        let Some(id) = invite.with_untracked(|i| i.as_ref().map(|(i, _)| i.id.clone())) else {
            return;
        };
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        spawn_local(async move {
            let result = match backend::pairing_transport() {
                Some(t) => pair::cancel(&t, &id).await,
                None => Ok(()),
            };
            busy.set(false);
            if let Err(e) = result {
                report(e);
            } else {
                invite.set(None);
                notice.set(Some("Invite cancelled.".into()));
            }
        });
    };
    let review = move |approve: bool| {
        if busy.get_untracked() {
            return;
        }
        let input = if approve {
            pending
                .get_untracked()
                .map(|(code, d)| (code, Some(d.created_at)))
        } else {
            pair::approval_code(&code.get_untracked(), &browser_origin())
                .ok()
                .map(|code| (code, None))
        };
        let Some((entered, at)) = input else {
            error.set(Some("Enter the code shown on the new device.".into()));
            return;
        };
        error.set(None);
        notice.set(None);
        busy.set(true);
        spawn_local(async move {
            let result = match backend::pairing_transport() {
                Some(t) => pair::review(&t, &entered, at).await,
                None => Ok(PendingDevice {
                    device_name: "New phone".into(),
                    created_at: 0,
                }),
            };
            busy.set(false);
            match result {
                Ok(d) if approve => {
                    pending.set(None);
                    code.set(String::new());
                    notice.set(Some(format!("Approved {}.", d.device_name)));
                }
                Ok(d) => pending.set(Some((entered, d))),
                Err(e) => {
                    pending.set(None);
                    report(e);
                }
            }
        });
    };
    view! {
        <button id="device-settings" type="button" class="icon-button" aria-label="Device settings" title="Device settings"
            on:click=move |_| {
                mode.set(None);
                if let Some(d) = dialog.get_untracked() { let _ = d.show_modal(); }
            }>{icon(Icon::Settings)}</button>
        <dialog class="room-dialog pair-dialog" node_ref=dialog aria-labelledby="device-settings-heading"
            on:close=move |_| { platform::focus("device-settings"); }>
            <h2 id="device-settings-heading">{move || match mode.get() { Some(true) => "Pair a new device", Some(false) => "Approve a device", None => "Device settings" }}</h2>
            <Show when=move || mode.get().is_none()>
                <div class="pair-actions">
                    <button class="button" on:click=move |_| { mode.set(Some(true)); error.set(None); notice.set(None); }>
                        "Pair a new device"
                    </button>
                    <button class="button" on:click=move |_| { mode.set(Some(false)); error.set(None); notice.set(None); }>
                        "Approve a device"
                    </button>
                    <a href="/admin/devices" class="button">"Devices"</a>
                </div>
            </Show>
            <Show when=move || mode.get() == Some(true)>
                {move || match invite.get() {
                    Some((i, qr)) => {
                        let link = i.link;
                        let view_box = format!("0 0 {0} {0}", qr.width);
                        view! {
                            <p>"Scan on your new device, type the code in its app, or open the link in its browser."</p>
                            <div class="pair-qr" data-theme="light" data-room-hue="my-day">
                                <svg viewBox=view_box role="img" aria-label="Invite QR code" shape-rendering="crispEdges">
                                    <path d=qr.path() />
                                </svg>
                            </div>
                            <p class="pair-code">{i.code}</p>
                            <label>"Invite link" <input type="text" readonly value=link on:focus=move |e| {
                                if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) { input.select(); }
                            } /></label>
                            <p>{move || format!("Expires in {}:{:02}", left.get() / 60, left.get() % 60)}</p>
                            <button class="button" disabled=move || busy.get() on:click=cancel>"Cancel invite"</button>
                        }.into_any()
                    }
                    None => view! { <button class="button primary" disabled=move || busy.get() on:click=create>"Create invite"</button> }.into_any(),
                }}
            </Show>
            <Show when=move || mode.get() == Some(false)>
                {move || match pending.get() {
                    Some((_, d)) => view! {
                        <p>{format!("Approve “{}”? Only continue if the code matches your own device.", d.device_name)}</p>
                        <div class="pair-actions">
                            <button class="button primary" disabled=move || busy.get() on:click=move |_| review(true)>"Approve"</button>
                            <button class="button" disabled=move || busy.get() on:click=move |_| {
                                pending.set(None);
                                code.set(String::new());
                                request_animation_frame(|| { platform::focus("pending-device-code"); });
                            }>"Use another code"</button>
                        </div>
                    }.into_any(),
                    None => view! {
                        <form on:submit=move |e| { e.prevent_default(); review(false); }>
                            <label>"Code shown on the device" <input id="pending-device-code" type="text" required autocomplete="off" spellcheck="false"
                                prop:value=move || code.get() on:input=move |e| code.set(event_target_value(&e)) /></label>
                            <button class="button primary" disabled=move || busy.get()>"Review"</button>
                        </form>
                    }.into_any(),
                }}
            </Show>
            <p role="status">{move || notice.get()}</p>
            <p role="alert" class="action-error">{move || error.get()}</p>
            <Show when=move || verify.get() && mode.get().is_some()>
                <a class="button" href="/auth/verify?next=/app">"Verify with passkey"</a>
            </Show>
            <div class="dialog-actions">
                <Show when=move || mode.get().is_some()>
                    <button class="button" on:click=move |_| { mode.set(None); error.set(None); notice.set(None); }>"Back"</button>
                </Show>
                <button class="button" on:click=move |_| room_actions::close_dialog(dialog)>"Close"</button>
            </div>
        </dialog>
    }
}
