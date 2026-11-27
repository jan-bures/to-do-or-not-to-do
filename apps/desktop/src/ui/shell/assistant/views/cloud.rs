use super::*;
use crate::chatgpt::USAGE;
use todo_client::assistant::native_relay::{Hosted, VERIFY_URL};
use todo_design::layout::elevation;

// Hang the padded hit box and focus border into the card gutters, not the label.
const USAGE_INSET: f32 = space::S1 + elevation::HAIRLINE;
const QUIET_INSET: f32 = space::S2 + elevation::HAIRLINE;

impl ShellView {
    pub(super) fn ai_provider_choices(
        &self,
        setup: &Setup,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        div()
            .flex_none()
            .flex()
            .flex_wrap()
            .gap(px(space::S2))
            .children(
                [
                    (None, "This device", !setup.draft.uses_relay()),
                    (
                        Some(Hosted::Private),
                        "Your private models",
                        setup.draft.private_relay,
                    ),
                    (
                        Some(Hosted::ChatGpt(uuid::Uuid::nil())),
                        "ChatGPT",
                        setup.draft.cloud,
                    ),
                ]
                .map(|(provider, label, chosen)| {
                    self.ai_button(
                        format!("assistant-provider-{label}"),
                        label,
                        if chosen { Tone::Primary } else { Tone::Outline },
                        enabled,
                        move |this, _, cx| this.choose_ai_provider(provider, cx),
                        cx,
                    )
                    .aria_toggled(toggled(chosen))
                }),
            )
            .text_color(p.ink)
    }

    pub(super) fn ai_hosted_settings(&self, setup: &Setup, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let running = matches!(setup.check, Check::Running(_));
        let status = match &setup.check {
            Check::Running(label) => Some((*label).into()),
            Check::Failed(label) => Some(label.clone()),
            _ => None,
        };
        let cloud = setup.draft.cloud;
        let available = if cloud {
            self.ai.cloud.available()
        } else {
            self.ai
                .cloud
                .status
                .as_ref()
                .is_some_and(|s| s.private_models)
        };
        let mut card = box_panel(&p)
            .flex_none()
            .border_color(p.accent_strong)
            .gap(px(space::S2))
            .p(px(space::S3))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::S2))
                    .child(text(
                        ty::NAV_COLLECTION,
                        p.ink,
                        if cloud {
                            "ChatGPT"
                        } else {
                            "Your private models (via your server)"
                        },
                    ))
                    .children(cloud.then(|| text(ty::CAPS_LABEL, p.ink_3, "CLOUD"))),
            )
            .child(text(
                ty::META,
                p.ink_2,
                if cloud {
                    "Sign in once to use your ChatGPT plan or available credits on paired devices."
                } else {
                    "Your To-do server connects to the private model. The key stays on your server."
                },
            ))
            .children(
                self.ai
                    .cloud
                    .error
                    .clone()
                    .map(|error| text(ty::META, p.danger_ink, error)),
            )
            .children(status.map(|status| text(ty::META, p.ink_2, status)));
        if !available {
            return card.child(text(
                ty::META,
                p.ink_3,
                self.ai
                    .cloud
                    .status
                    .as_ref()
                    .map_or(
                        "Waiting for assistant settings from your server…",
                        |status| {
                            status
                                .disabled_reason
                                .as_deref()
                                .unwrap_or("Not configured on your server.")
                        },
                    )
                    .to_owned(),
            ));
        }
        let active = self.ai.cloud.active(&setup.draft);
        if cloud {
            let accounts = self.ai.cloud.saved();
            for (index, account) in accounts.iter().enumerate() {
                let id = account.id;
                let selected = Some(id) == setup.draft.chatgpt_account;
                let label = model::labels::one_line(&account.label);
                let choice = div()
                    .flex_1()
                    .min_w_0()
                    .min_h(px(component::BUTTON_SMALL))
                    .flex()
                    .items_center()
                    .gap(px(space::S2))
                    .px(px(space::S1))
                    .rounded(px(radius::MD))
                    .when(selected, |row| row.bg(p.wash))
                    .child(div().size(px(component::ICON_SMALL)).flex_none().children(
                        selected.then(|| {
                            kit::icon(Icon::Check, component::ICON_SMALL, p.accent_strong)
                        }),
                    ))
                    .child(one_line(text(ty::BODY, p.ink, label.clone())));
                card = card.child(line(&p)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(QUIET_INSET))
                        .child(
                            self.ai_action(
                                format!("assistant-chatgpt-account-{index}"),
                                choice,
                                !running,
                                move |this, _, cx| this.choose_chatgpt_account(id, cx),
                                cx,
                            )
                            .aria_toggled(toggled(selected))
                            .aria_label(label.clone())
                            .tooltip(kit::tooltip(p, label.clone())),
                        )
                        .child(
                            self.ai_button(
                                format!("assistant-chatgpt-signout-{index}"),
                                "Sign out",
                                Tone::Quiet,
                                !running,
                                move |this, _, cx| this.sign_out_chatgpt(id, cx),
                                cx,
                            )
                            .mr(px(-QUIET_INSET))
                            .aria_label(format!("Sign out of {label}")),
                        ),
                );
            }
            if let Some((_, label)) = &self.ai.cloud.signed_in {
                card = card.child(one_line(text(
                    ty::META,
                    p.ink_2,
                    format!("Connected {}.", model::labels::one_line(label)),
                )));
            }
            card = card.child(
                self.ai_manage_usage("assistant-chatgpt-settings-usage", Tone::Quiet, cx)
                    .ml(px(-USAGE_INSET)),
            );
            if let Some(code) = &self.ai.cloud.code {
                return card.child(text(ty::META, p.ink_2, "Open ChatGPT and enter this one-time code. Continue only if you started this sign-in."))
                    .child(text(ty::COUNT, p.ink, code.clone()))
                    .child(div().flex().gap(px(space::S2))
                        .child(self.ai_button("assistant-chatgpt-open-code", "Open ChatGPT", Tone::Primary, true, |_, _, cx| cx.open_url(VERIFY_URL), cx))
                        .child(self.ai_button("assistant-chatgpt-cancel-code", "Cancel sign-in", Tone::Quiet, true, |this, _, cx| {
                            if let Some(setup) = &mut this.ai.settings { setup.invalidate(); }
                            this.ai.cloud.code = None;
                            cx.notify();
                        }, cx)));
            }
            let device = self
                .ai
                .cloud
                .status
                .as_ref()
                .is_some_and(|s| s.chatgpt_device_code);
            card = card.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(QUIET_INSET))
                    .child(self.ai_button(
                        "assistant-chatgpt-continue",
                        if accounts.is_empty() {
                            "Sign in with ChatGPT"
                        } else {
                            "Add another account"
                        },
                        Tone::Primary,
                        !running,
                        |this, _, cx| this.sign_in_chatgpt(true, cx),
                        cx,
                    ))
                    .children(device.then(|| {
                        self.ai_button(
                            "assistant-chatgpt-start-device",
                            "Use a code instead",
                            Tone::Quiet,
                            !running,
                            |this, _, cx| this.sign_in_chatgpt(false, cx),
                            cx,
                        )
                        .ml(px(-QUIET_INSET))
                    })),
            );
            if active.is_none() {
                return card;
            }
        }
        let model = if setup.draft.model.is_empty() {
            "No model selected".into()
        } else {
            model::labels::one_line(&setup.draft.model)
        };
        card.child(line(&p))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::S2))
                    .child(text(ty::META, p.ink_2, "Model").flex_none())
                    .child(one_line(text(ty::COUNT, p.ink, model)).flex_1())
                    .child(self.ai_button(
                        "assistant-chatgpt-models",
                        "Choose model",
                        Tone::Outline,
                        !running,
                        |this, _, cx| this.list_chatgpt_models(cx),
                        cx,
                    )),
            )
            .when(setup.pick_model, |card| {
                card.child(self.ai_model_picker(setup, cx))
            })
    }

    pub(super) fn ai_manage_usage(
        &self,
        id: &'static str,
        tone: Tone,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        self.ai_clickable(
            id,
            div()
                .self_start()
                .flex_none()
                .h(px(component::BUTTON_SMALL))
                .flex()
                .items_center()
                .gap(px(space::S1))
                .rounded(px(radius::MD))
                .border_1()
                .border_color(if matches!(tone, Tone::Quiet) {
                    gpui::transparent_black()
                } else {
                    p.line_strong
                })
                .px(px(space::S1))
                .child(text(ty::BUTTON_SMALL, p.accent_strong, "Manage usage"))
                .child(kit::icon(
                    Icon::ExternalLink,
                    component::ICON_SMALL,
                    p.accent_strong,
                )),
            true,
            |this, _, cx| {
                this.ai.cloud.signed_in = None;
                cx.open_url(USAGE);
                cx.notify();
            },
            cx,
        )
        .when(matches!(tone, Tone::Quiet), |d| d.quiet_button(p))
        .when(!matches!(tone, Tone::Quiet), |d| {
            d.hover(|s| s.opacity(0.85))
        })
        .role(Role::Link)
        .aria_label("Manage usage")
    }
}
