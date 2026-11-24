use super::*;
use crate::chatgpt::USAGE;
use todo_client::assistant::native_relay::{Hosted, VERIFY_URL};

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
            if let Some((_, label)) = &self.ai.cloud.signed_in {
                card = card.child(text(
                    ty::META,
                    p.ink,
                    format!(
                        "Connected {}. Your selection hasn’t changed.",
                        model::labels::one_line(label)
                    ),
                ));
            }
            for (index, account) in self.ai.cloud.saved().into_iter().enumerate() {
                let id = account.id;
                let selected = Some(id) == setup.draft.chatgpt_account;
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(space::S2))
                        .child(
                            self.ai_button(
                                format!("assistant-chatgpt-account-{index}"),
                                model::labels::one_line(&account.label),
                                if selected {
                                    Tone::Primary
                                } else {
                                    Tone::Outline
                                },
                                !running,
                                move |this, _, cx| this.choose_chatgpt_account(id, cx),
                                cx,
                            )
                            .aria_toggled(toggled(selected)),
                        )
                        .child(self.ai_button(
                            format!("assistant-chatgpt-signout-{index}"),
                            "Sign out",
                            Tone::Quiet,
                            !running,
                            move |this, _, cx| this.sign_out_chatgpt(id, cx),
                            cx,
                        )),
                );
            }
            if self.ai.cloud.fixture_enabled
                && let Some(account) = &active
                && !self
                    .ai
                    .cloud
                    .saved()
                    .iter()
                    .any(|saved| saved.id == account.id)
            {
                card = card.child(text(
                    ty::META,
                    p.ink,
                    model::labels::one_line(&account.label),
                ));
            }
            card = card.child(self.ai_manage_usage(
                "assistant-chatgpt-settings-usage",
                Tone::Quiet,
                cx,
            ));
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
                .is_some_and(|s| s.chatgpt_device_code)
                || self.ai.cloud.fixture_enabled;
            card = card.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(space::S2))
                    .children(device.then(|| {
                        self.ai_button(
                            "assistant-chatgpt-start-device",
                            if active.is_some() || !self.ai.cloud.saved().is_empty() {
                                "Add another ChatGPT account"
                            } else {
                                "Sign in with ChatGPT"
                            },
                            Tone::Outline,
                            !running,
                            |this, _, cx| this.sign_in_chatgpt(false, cx),
                            cx,
                        )
                    }))
                    .child(self.ai_button(
                        "assistant-chatgpt-continue",
                        "Continue in browser",
                        Tone::Quiet,
                        !running,
                        |this, _, cx| this.sign_in_chatgpt(true, cx),
                        cx,
                    )),
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
        card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(space::S2))
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
            card.child(
                div()
                    .id("assistant-chatgpt-model-picker")
                    .max_h(px(desktop::AI_COMPOSER_MAX))
                    .overflow_y_scroll()
                    .children(setup.models.iter().enumerate().map(|(i, m)| {
                        let id = m.id.clone();
                        self.ai_button(
                            format!("assistant-chatgpt-model-{i}"),
                            model::labels::one_line(&m.label),
                            if setup.draft.model == m.id {
                                Tone::Primary
                            } else {
                                Tone::Quiet
                            },
                            !running,
                            move |this, _, cx| this.pick_chatgpt_model(id.clone(), cx),
                            cx,
                        )
                    })),
            )
        })
        .child(
            self.ai_button(
                "assistant-chatgpt-enabled",
                "Enable assistant on this device",
                if setup.draft.enabled {
                    Tone::Primary
                } else {
                    Tone::Outline
                },
                !running && valid_model(&setup.draft.model),
                |this, _, cx| {
                    if let Some(setup) = &mut this.ai.settings {
                        setup.draft.enabled = !setup.draft.enabled;
                        cx.notify();
                    }
                },
                cx,
            )
            .aria_toggled(toggled(setup.draft.enabled)),
        )
    }

    pub(super) fn ai_manage_usage(
        &self,
        id: &'static str,
        tone: Tone,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.ai_button(
            id,
            "Manage usage",
            tone,
            true,
            |_, _, cx| cx.open_url(USAGE),
            cx,
        )
    }

    pub(super) fn ai_chatgpt_consent(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = self.palette;
        let mut scope = Vec::new();
        if let Some(consent) = &self.ai.cloud.consent {
            let mut new = Vec::new();
            let mut previous = Vec::new();
            if let Some(state) = &consent.state {
                for id in &consent.lists {
                    if let Some(list) = state.lists.get(id) {
                        let name = model::labels::one_line(&list.name);
                        if consent
                            .previously_allowed
                            .as_ref()
                            .is_some_and(|lists| lists.contains(id))
                        {
                            previous.push(name);
                        } else {
                            new.push(name);
                        }
                    }
                }
            }
            if new.is_empty() && previous.is_empty() {
                scope.push("No lists are currently shared.".into());
            }
            if !new.is_empty() {
                new.sort();
                let heading = if consent.previously_allowed.is_some() {
                    "New lists to share"
                } else {
                    "Lists set to All models"
                };
                scope.push(format!("{heading}: {}.", new.join(", ")));
            }
            if !previous.is_empty() {
                previous.sort();
                scope.push(format!("Previously allowed: {}.", previous.join(", ")));
            }
        }
        let dialog = div().id("assistant-chatgpt-consent-dialog").role(Role::Dialog).aria_label("Share with ChatGPT?")
            .w(px(desktop::AI_SETTINGS_WIDTH)).max_w_full().max_h_full().rounded(px(radius::XL))
            .bg(p.pane).shadow(kit::overlay_shadow(&p)).border_1().border_color(p.line_strong)
            .flex().flex_col().occlude().track_focus(&self.ai.dialog).tab_group()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" && !event.is_held {
                    this.answer_cloud_consent(false, window, cx);
                    cx.stop_propagation();
                } else if super::super::super::navigation_tab(event) {
                    if event.keystroke.modifiers.shift { window.focus_prev(cx); } else { window.focus_next(cx); }
                    if !this.ai.dialog.contains_focused(window, cx) {
                        window.focus(&this.ai.focus_handle("assistant-chatgpt-consent-cancel", cx), cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(div().p(px(space::S5)).flex().flex_col().gap(px(space::S3))
                .child(text(ty::DETAIL_TITLE, p.ink, "Share with ChatGPT?"))
                .child(text(ty::BODY, p.ink_2, "ChatGPT is a cloud service. OpenAI receives this chat and the allowed task summaries used to answer it. Hidden and Private only lists, task notes, and attachments are not shared."))
                .child(div().id("assistant-chatgpt-consent-scope").max_h(px(desktop::AI_COMPOSER_MAX)).overflow_y_scroll()
                    .flex().flex_col().gap(px(space::S2))
                    .children(scope.into_iter().map(|line| text(ty::BODY, p.ink, line))))
                .child(text(ty::META, p.ink_2, "This uses your ChatGPT plan or available credits. Manage usage and credit use in ChatGPT settings. To stop sharing, switch providers or sign out."))
                .child(self.ai_manage_usage("assistant-chatgpt-consent-usage", Tone::Quiet, cx)))
            .child(kit::dialog_footer(&p, radius::XL).flex().justify_between().items_center().gap(px(space::S2)).px(px(space::S5)).py(px(space::S3))
                .child(self.ai_button("assistant-chatgpt-consent-cancel", "Cancel", Tone::Quiet, true, |this, w, cx| this.answer_cloud_consent(false, w, cx), cx))
                .child(self.ai_button("assistant-chatgpt-consent-send", "Allow and send", Tone::Primary, true, |this, w, cx| this.answer_cloud_consent(true, w, cx), cx)));
        div()
            .id("assistant-chatgpt-consent-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(space::S5))
            .bg(p.scrim)
            .occlude()
            .child(dialog)
    }
}
