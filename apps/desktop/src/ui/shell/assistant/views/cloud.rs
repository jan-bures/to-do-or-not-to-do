use super::*;
use crate::chatgpt::USAGE;

impl ShellView {
    pub(super) fn ai_provider_choices(
        &self,
        cloud: bool,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        div()
            .flex_none()
            .flex()
            .gap(px(space::S2))
            .children([(false, "Your own server"), (true, "ChatGPT account")].map(
                |(option, label)| {
                    self.ai_button(
                        format!("assistant-provider-{option}"),
                        label,
                        if option == cloud {
                            Tone::Primary
                        } else {
                            Tone::Outline
                        },
                        enabled,
                        move |this, _, cx| this.choose_ai_provider(option, cx),
                        cx,
                    )
                    .aria_toggled(toggled(option == cloud))
                },
            ))
            .text_color(p.ink)
    }

    pub(super) fn ai_chatgpt_settings(&self, setup: &Setup, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let running = matches!(setup.check, Check::Running(_));
        let status = match &setup.check {
            Check::Running(label) => Some((*label).into()),
            Check::Failed(label) => Some(label.clone()),
            _ => None,
        };
        let active = self.ai.cloud.active();
        let available = self.ai.cloud.available();
        let mut card = box_panel(&p).flex_none().border_color(p.accent).gap(px(space::S2))
            .p(px(space::S3))
            .child(div().flex().items_center().justify_between().gap(px(space::S2))
                .child(text(ty::NAV_COLLECTION, p.ink, "ChatGPT account"))
                .child(text(ty::CAPS_LABEL, p.ink_3, "CLOUD · OPENAI")))
            .child(text(ty::META, p.ink_2, "Use your ChatGPT plan. Eligible requests may use plan allowance or available credits. No separate To-do charge."))
            .children(self.ai.cloud.error.clone().map(|error| text(ty::META, p.danger_ink, error)))
            .children(status.map(|status| text(ty::META, p.ink_2, status)));
        if !available {
            return card.child(text(
                ty::META,
                p.ink_3,
                "ChatGPT sign-in requires a paired device and protected local storage.",
            ));
        }
        let signed_in = active.as_ref().is_some_and(|a| a.signed_in);
        if let Some(account) = active.filter(|a| a.signed_in) {
            let label = model::labels::one_line(&account.label);
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(space::S2))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(space::S2))
                            .min_w_0()
                            .child(kit::icon(
                                Icon::CircleCheck,
                                component::ICON_SMALL,
                                p.success,
                            ))
                            .child(one_line(text(ty::META, p.ink, label))),
                    )
                    .child(self.ai_button(
                        "assistant-chatgpt-signout",
                        "Sign out",
                        Tone::Outline,
                        !running,
                        |this, _, cx| this.sign_out_chatgpt(cx),
                        cx,
                    )),
            );
            if account.plan_allowed {
                let model = account
                    .model
                    .as_deref()
                    .map_or("No model selected".into(), model::labels::one_line);
                card = card
                    .child(
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
                                    let slug = m.id.clone();
                                    self.ai_button(
                                        format!("assistant-chatgpt-model-{i}"),
                                        model::labels::one_line(&m.label),
                                        if account.model.as_ref() == Some(&m.id) {
                                            Tone::Primary
                                        } else {
                                            Tone::Quiet
                                        },
                                        !running,
                                        move |this, _, cx| {
                                            this.pick_chatgpt_model(slug.clone(), cx)
                                        },
                                        cx,
                                    )
                                })),
                        )
                    })
                    .child(
                        self.ai_button(
                            "assistant-chatgpt-enabled",
                            if setup.draft.enabled {
                                "Assistant enabled on this device"
                            } else {
                                "Enable assistant on this device"
                            },
                            if setup.draft.enabled {
                                Tone::Primary
                            } else {
                                Tone::Outline
                            },
                            !running && account.model.is_some(),
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
                    .child(self.ai_manage_usage(
                        "assistant-chatgpt-settings-usage",
                        Tone::Quiet,
                        cx,
                    ))
                    .when(self.ai.cloud.usage_paused, |card| {
                        card.child(self.ai_button(
                            "assistant-chatgpt-settings-resume",
                            "Resume after reviewing usage",
                            Tone::Outline,
                            !running,
                            |this, _, cx| {
                                this.ai.cloud.usage_paused = false;
                                cx.notify();
                            },
                            cx,
                        ))
                    });
            } else {
                card = card
                    .child(text(
                        ty::META,
                        p.ink_2,
                        "You’re signed in, but haven’t enabled ChatGPT plan usage.",
                    ))
                    .child(self.ai_button(
                        "assistant-chatgpt-plan",
                        "Enable ChatGPT plan usage",
                        Tone::Outline,
                        !running,
                        move |this, _, cx| {
                            this.sign_in_chatgpt(Some(account.id.clone()), true, cx);
                        },
                        cx,
                    ));
            }
        }
        for (i, account) in self
            .ai
            .cloud
            .saved()
            .into_iter()
            .filter(|a| {
                self.ai
                    .cloud
                    .active()
                    .is_none_or(|b| b.id != a.id || !b.signed_in)
            })
            .enumerate()
        {
            let label = model::labels::one_line(&account.label);
            card = card.child(self.ai_button(
                format!("assistant-chatgpt-account-{i}"),
                label,
                Tone::Quiet,
                !running,
                move |this, _, cx| this.choose_chatgpt_account(account.clone(), cx),
                cx,
            ));
        }
        if signed_in {
            // Signed in, the brand button would read as a second sign-in; adding an account is secondary.
            return card.child(self.ai_button(
                "assistant-chatgpt-continue",
                "Add another ChatGPT account",
                Tone::Quiet,
                !running,
                |this, _, cx| this.sign_in_chatgpt(None, false, cx),
                cx,
            ));
        }
        card.child(text(ty::META, p.ink_2,
            "To-do keeps the account name or email and sign-in credentials protected on this PC. ChatGPT is a cloud service."))
            .child(
            self.ai_action(
                "assistant-chatgpt-continue",
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SM))
                    .gap(px(space::S2))
                    .h(px(component::BUTTON))
                    .bg(gpui::black())
                    .opacity(if running { 0.6 } else { 1.0 })
                    .child(
                        gpui::svg()
                            .path(crate::assets::CHATGPT_LOGO)
                            .size(px(component::ICON))
                            .text_color(gpui::white()),
                    )
                    .child(text(ty::BUTTON, gpui::white(), "Continue with ChatGPT")),
                !running,
                |this, _, cx| this.sign_in_chatgpt(None, false, cx),
                cx,
            )
            .aria_label("Continue with ChatGPT"),
        )
    }

    pub(super) fn ai_chatgpt_welcome(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let focus = self.ai.focus_handle("assistant-chatgpt-got-it", cx);
        if !focus.is_focused(window) {
            cx.defer_in(window, |this, window, cx| {
                if this.ai.settings.as_ref().is_some_and(|s| s.draft.cloud)
                    && this.ai.cloud.welcome_pending()
                {
                    window.focus(&this.ai.focus_handle("assistant-chatgpt-got-it", cx), cx);
                }
            });
        }
        let dialog = div().id("assistant-chatgpt-welcome-dialog").role(Role::Dialog).aria_label("You’re using your ChatGPT plan")
            .w(px(desktop::AI_SETTINGS_WIDTH)).max_w_full().rounded(px(radius::XL)).bg(p.pane)
            .shadow(kit::overlay_shadow(&p)).border_1().border_color(p.line_strong).occlude()
            .track_focus(&self.ai.dialog).tab_group().flex().flex_col()
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if super::super::super::navigation_tab(event) {
                    window.focus(&this.ai.focus_handle("assistant-chatgpt-got-it", cx), cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "escape" && !event.is_held {
                    this.welcome_chatgpt(cx);
                    window.focus(&this.ai.dialog, cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().flex().flex_col().gap(px(space::S3)).p(px(space::S5))
                .child(text(ty::DETAIL_TITLE, p.ink, "You’re using your ChatGPT plan"))
                .child(text(ty::BODY, p.ink_2, "Eligible usage in To-do uses your ChatGPT plan. You can manage usage in your ChatGPT settings."))
                .children(self.ai.cloud.error.clone().map(|error| text(ty::META, p.danger_ink, error))))
            .child(kit::dialog_footer(&p, radius::XL).flex().justify_end().px(px(space::S5)).py(px(space::S3))
                .child(self.ai_button("assistant-chatgpt-got-it", "Got it", Tone::Primary, true, |this, window, cx| {
                    this.welcome_chatgpt(cx);
                    window.focus(&this.ai.dialog, cx);
                }, cx)));
        div()
            .id("assistant-chatgpt-welcome-overlay")
            .absolute()
            .inset_0()
            .p(px(space::S5))
            .bg(p.scrim)
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .child(dialog)
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
