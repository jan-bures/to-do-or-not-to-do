//! Shared relay settings with no upstream credentials in the browser.

use super::{CHAT, Ctx, Hosted, Ui, ctx, icon, relay, settings};
use crate::app::assistant_provider::{PATH, encoded, relay_json};
use leptos::prelude::*;
use std::{collections::BTreeSet, time::Duration};
use todo_client::{
    assistant::{AccessRules, Controller, default_chatgpt_account, provider::ProviderError},
    platform::Method,
    web::sleep,
};
use todo_core::ids::ListId;
use todo_design::Icon;
use uuid::Uuid;

const VERIFY_URL: &str = "https://auth.openai.com/codex/device";
const USAGE_URL: &str = "https://chatgpt.com/settings/usage";

#[derive(Clone)]
pub(super) struct ConsentPrompt {
    account: Uuid,
    prompt: String,
    lists: BTreeSet<ListId>,
    names: Vec<String>,
    version: Uuid,
}

impl Ui {
    pub(super) async fn refresh_hosted(self) -> Result<relay::Status, ProviderError> {
        let status: relay::Status = relay_json(Method::Get, format!("{PATH}/status"), None).await?;
        if self.signed_out.get_value() {
            return Err(ProviderError::Unauthorized);
        }
        let profile = self.profile.get_value();
        let old = self.relay_status.get_untracked();
        let changed = match profile.hosted {
            Some(Hosted::Private) => old.as_ref().is_none_or(|old| {
                old.private_models != status.private_models || old.access != status.access
            }),
            Some(Hosted::ChatGpt(id)) => old.as_ref().is_none_or(|old| {
                old.access != status.access
                    || old.accounts.iter().find(|a| a.id == id)
                        != status.accounts.iter().find(|a| a.id == id)
            }),
            None => false,
        };
        self.relay_status.set(Some(status.clone()));
        if changed {
            self.route.set(settings::configure(
                self.scope.get_value(),
                &profile,
                &self.credential.get_value(),
                Some(&status),
            ));
            self.consent.set(None);
            self.approvals.set(Default::default());
            self.refresh();
        }
        Ok(status)
    }

    pub(super) fn prepare_hosted_send(self, c: Ctx, hosted: Hosted, prompt: String) {
        self.preparing.set(true);
        self.consent.set(None);
        leptos::task::spawn_local(async move {
            let result = self.refresh_hosted().await;
            self.preparing.set(false);
            if self.signed_out.get_value()
                || !self.open.get_untracked()
                || self.settings.get_untracked().is_some()
                || self.profile.with_value(|p| p.hosted != Some(hosted))
                || self.draft.with_untracked(|p| p != &prompt)
            {
                return;
            }
            let status = match result {
                Ok(status) => status,
                Err(err) => {
                    self.error.set(Some(err.to_string()));
                    return;
                }
            };
            self.sync(false);
            if let Hosted::ChatGpt(id) = hosted {
                let Some(account) = status.accounts.iter().find(|a| a.id == id) else {
                    self.error
                        .set(Some("This ChatGPT account is no longer connected.".into()));
                    return;
                };
                let (state, lists) = CHAT.with(Controller::list_scope);
                if !account.approved || !lists.is_subset(&account.approved_lists) {
                    let names = state.map_or_else(Vec::new, |s| {
                        lists
                            .iter()
                            .filter_map(|id| s.lists.get(id).map(|l| l.name.clone()))
                            .collect()
                    });
                    self.consent.set(Some(ConsentPrompt {
                        account: id,
                        prompt,
                        lists,
                        names,
                        version: status.access_version,
                    }));
                    return;
                }
            }
            self.start_turn(c, prompt);
        });
    }

    fn approve_cloud_scope(self, c: Ctx, prompt: ConsentPrompt) {
        if self.preparing.get_untracked() {
            return;
        }
        let (_state, latest) = CHAT.with(Controller::list_scope);
        if latest != prompt.lists
            || self.draft.with_untracked(|text| text != &prompt.prompt)
            || self
                .profile
                .with_value(|p| p.hosted != Some(Hosted::ChatGpt(prompt.account)))
        {
            self.consent.set(None);
            self.error.set(Some(
                "The message or list scope changed. Send again to review it.".into(),
            ));
            return;
        }
        self.preparing.set(true);
        leptos::task::spawn_local(async move {
            let result: Result<relay::Account, _> = async {
                let body = relay::Consent {
                    lists: prompt.lists,
                    access_version: prompt.version,
                };
                relay_json(
                    Method::Put,
                    format!("{PATH}/chatgpt/accounts/{}/consent", prompt.account),
                    encoded(&body)?,
                )
                .await
            }
            .await;
            self.preparing.set(false);
            self.consent.set(None);
            if self.signed_out.get_value() {
                return;
            }
            match result {
                Ok(_) if self.draft.with_untracked(|text| text == &prompt.prompt) => self.send(c),
                Ok(_) => {}
                Err(_) => self.error.set(Some(
                    "Couldn’t save that approval. Send again to review the current list scope."
                        .into(),
                )),
            }
        });
    }
}

#[component]
pub(super) fn ConsentNotice() -> impl IntoView {
    let c = ctx();
    let ai = c.ai;
    move || {
        ai.consent.get().map(|prompt| {
        let names = if prompt.names.is_empty() { "No task lists".to_owned() } else { prompt.names.join(", ") };
        view! {
            <section class="ai-provider ai-cloud-consent" aria-label="ChatGPT sharing approval">
                <strong>"Share with ChatGPT?"</strong>
                <p>"This uses your ChatGPT plan or available credits. ChatGPT can read these lists: "{names}"."</p>
                <div class="ai-provider-actions">
                    <button type="button" class="button" on:click=move |_| ai.consent.set(None)>"Cancel"</button>
                    <button type="button" class="button primary" disabled=move || ai.preparing.get()
                        on:click=move |_| ai.approve_cloud_scope(c, prompt.clone())>"Allow and send"</button>
                </div>
            </section>
        }
    })
    }
}

pub(super) async fn save_access(
    update: relay::AccessUpdate,
) -> Result<relay::Status, ProviderError> {
    relay_json(Method::Put, format!("{PATH}/access"), encoded(&update)?).await
}

#[derive(Clone)]
struct DeviceCode {
    attempt: Uuid,
    user_code: String,
}

pub(super) fn choose(
    draft: RwSignal<settings::Profile>,
    status: RwSignal<Option<relay::Status>>,
    local: StoredValue<AccessRules>,
    provider: Hosted,
) {
    let Some(status) = status.get_untracked() else {
        return;
    };
    draft.update(|p| {
        if p.hosted.is_none() {
            local.set_value(p.access.clone());
            p.access = status.access;
        }
        if p.hosted != Some(provider) {
            p.hosted_model.clear();
        }
        p.hosted = Some(provider);
    });
}

pub(super) fn select_chatgpt_account(
    draft: RwSignal<settings::Profile>,
    status: RwSignal<Option<relay::Status>>,
    local_access: StoredValue<AccessRules>,
    signed_in: Option<Uuid>,
) {
    let current = draft.with_untracked(|p| p.hosted);
    let selected = match current {
        Some(Hosted::ChatGpt(id)) => Some(id),
        None if signed_in.is_some() => None,
        _ => return,
    };
    let account = status.with_untracked(|s| {
        s.as_ref()
            .and_then(|s| default_chatgpt_account(&s.accounts, selected, signed_in))
    });
    if let Some(id) = account.filter(|id| Some(*id) != selected) {
        choose(draft, status, local_access, Hosted::ChatGpt(id));
    }
}

#[component]
pub(super) fn HostedCards(
    draft: RwSignal<settings::Profile>,
    status: RwSignal<Option<relay::Status>>,
    error: RwSignal<Option<String>>,
    local_access: StoredValue<AccessRules>,
    tab: RwSignal<settings::ProviderTab>,
    signed_in: RwSignal<Option<String>>,
) -> impl IntoView {
    use settings::ProviderTab;

    let ai = ctx().ai;
    let code = RwSignal::new(None::<DeviceCode>);
    let busy = RwSignal::new(false);
    Effect::new(move |_| {
        if ai.settings.get().is_none() || tab.get() != ProviderTab::ChatGpt {
            code.set(None);
        }
    });
    let refresh = move |signed_in| async move {
        match ai.refresh_hosted().await {
            Ok(next) if ai.settings.get_untracked().is_some() => {
                let changed = status.with_untracked(|old| {
                    old.as_ref()
                        .is_some_and(|old| old.access_version != next.access_version)
                });
                if !changed {
                    status.set(Some(next));
                } else {
                    // Account changes are live, but never substitute unreviewed list access.
                    status.update(|reviewed| {
                        if let Some(reviewed) = reviewed {
                            let mut next = next;
                            next.access.clone_from(&reviewed.access);
                            next.access_version = reviewed.access_version;
                            *reviewed = next;
                        }
                    });
                    error.set(Some(
                        "List access changed on another device. Reopen settings to review it."
                            .into(),
                    ));
                }
                if tab.get_untracked() == ProviderTab::ChatGpt {
                    select_chatgpt_account(draft, status, local_access, signed_in);
                }
            }
            Ok(_) => {}
            Err(err) => error.set(Some(err.to_string())),
        }
    };
    let start = move |_| {
        if busy.get_untracked() {
            return;
        }
        busy.set(true);
        error.set(None);
        signed_in.set(None);
        leptos::task::spawn_local(async move {
            let result = async {
                relay_json(
                    Method::Post,
                    format!("{PATH}/chatgpt/start"),
                    encoded(&relay::SignIn::Device)?,
                )
                .await
            }
            .await;
            busy.set(false);
            if ai.settings.get_untracked().is_none() || tab.get_untracked() != ProviderTab::ChatGpt
            {
                return;
            }
            let device = match result {
                Ok(relay::SignInStarted::Device {
                    attempt,
                    user_code,
                    verification_url,
                }) if verification_url == VERIFY_URL => DeviceCode { attempt, user_code },
                _ => {
                    error.set(Some("Couldn’t start ChatGPT sign-in. Check that device-code sign-in is available.".into()));
                    return;
                }
            };
            code.set(Some(device.clone()));
            loop {
                sleep(Duration::from_secs(1)).await;
                if !code.with_untracked(|c| c.as_ref().is_some_and(|c| c.attempt == device.attempt))
                {
                    return;
                }
                let result = relay_json(
                    Method::Post,
                    format!("{PATH}/chatgpt/poll/{}", device.attempt),
                    None,
                )
                .await;
                if !code.with_untracked(|c| c.as_ref().is_some_and(|c| c.attempt == device.attempt))
                {
                    return;
                }
                match result {
                    Ok(relay::Poll::Pending) => continue,
                    Ok(relay::Poll::SignedIn { account }) => {
                        code.set(None);
                        refresh(Some(account.id)).await;
                        if ai.settings.get_untracked().is_some()
                            && tab.get_untracked() == ProviderTab::ChatGpt
                        {
                            signed_in.set(Some(account.label));
                        }
                    }
                    _ => {
                        code.set(None);
                        error.set(Some(
                            "ChatGPT sign-in didn’t finish. Start a new sign-in to try again."
                                .into(),
                        ));
                    }
                }
                break;
            }
        });
    };
    let account_actions_disabled = move || busy.get() || code.with(Option::is_some);
    let available = move || status.with(|s| s.as_ref().is_some_and(|s| s.private_models));
    let private_selected = move || draft.with(|p| p.hosted == Some(Hosted::Private));
    let cloud_selected = move || {
        let selected = draft.with(|p| p.hosted);
        status.with(|s| {
            s.as_ref().is_some_and(|s| {
                s.accounts
                    .iter()
                    .any(|a| selected == Some(Hosted::ChatGpt(a.id)))
            })
        })
    };
    let cloud_available = move || status.with(|s| ProviderTab::ChatGpt.available(s.as_ref()));
    view! {
        <section class="ai-provider" aria-label="ChatGPT" data-selected="true" hidden=move || tab.get() != ProviderTab::ChatGpt>
            <div class="ai-provider-heading"><h3>"ChatGPT"</h3><span>"Cloud"</span></div>
            <p>"Sign in once to use your ChatGPT plan or available credits on paired devices."</p>
            {move || status.with(|s| s.as_ref().and_then(|s| s.disabled_reason.clone())).map(|reason| view! {
                <p role="status">{reason}</p>
            })}
            <div class="ai-accounts" role="group" aria-label="ChatGPT accounts">
            <For each=move || status.with(|s| s.as_ref().map_or_else(Vec::new, |s| s.accounts.clone())) key=|a| (a.id, a.label.clone())
                children=move |account| {
                    let id = account.id;
                    let active = move || draft.with(|p| p.hosted == Some(Hosted::ChatGpt(id)));
                    let label = account.label.split_whitespace().collect::<Vec<_>>().join(" ");
                    let title = label.clone();
                    let sign_out = format!("Sign out of {label}");
                    view! {
                        <div class="ai-account">
                            <button type="button" class="ai-account-choice" aria-pressed=move || active().to_string()
                                disabled=account_actions_disabled on:click=move |_| {
                                    if !active() { choose(draft, status, local_access, Hosted::ChatGpt(id)); }
                                }>
                                <span class="ai-account-check" aria-hidden="true"><Show when=active>{icon(Icon::Check)}</Show></span>
                                <span class="ai-truncate" title=title>{label}</span>
                            </button>
                            <button type="button" class="ai-link" aria-label=sign_out disabled=account_actions_disabled on:click=move |_| {
                                busy.set(true);
                                leptos::task::spawn_local(async move {
                                    let result: Result<relay::SignOut, _> = relay_json(Method::Delete, format!("{PATH}/chatgpt/accounts/{id}"), None).await;
                                    busy.set(false);
                                    match result {
                                        Ok(signed_out) => {
                                            signed_in.set(None);
                                            if draft.with_untracked(|p| p.hosted == Some(Hosted::ChatGpt(id))) {
                                                draft.update(|p| p.hosted_model.clear());
                                            }
                                            refresh(None).await;
                                            if !signed_out.revoked { error.set(Some("Signed out on your server. Revocation wasn’t confirmed. You can disconnect the app in ChatGPT settings.".into())); }
                                        }
                                        Err(err) => error.set(Some(err.to_string())),
                                    }
                                });
                            }>"Sign out"</button>
                        </div>
                    }
                }/>
            </div>
            {move || signed_in.get().map(|label| view! {
                <p class="ai-meta ai-truncate" role="status">{format!("Connected {label}.")}</p>
            })}
            <Show when=cloud_available>
                <a class="ai-link ai-usage" href=USAGE_URL target="_blank" rel="noopener noreferrer">"Manage usage"{icon(Icon::ExternalLink)}</a>
            </Show>
            <Show when=move || cloud_available() && status.with(|s| s.as_ref().is_some_and(|s| s.chatgpt_device_code)) && code.with(Option::is_none)>
                <button type="button" class="button primary" on:click=start disabled=move || busy.get()>
                    {move || if status.with(|s| s.as_ref().is_some_and(|s| !s.accounts.is_empty())) { "Add another account" } else { "Sign in with ChatGPT" }}
                </button>
            </Show>
            <Show when=move || status.with(|s| s.as_ref().is_some_and(|s| !s.chatgpt_device_code && s.disabled_reason.is_none()))>
                <p class="ai-meta">{move || if cloud_available() { "Sign in from the desktop or Android app. The account then works here too." } else { "Not configured on your server." }}</p>
            </Show>
            {move || code.get().map(|code_value| view! {
                <p>"Open ChatGPT and enter this one-time code. Continue only if you started this sign-in."</p>
                <code class="ai-device-code">{code_value.user_code}</code>
                <div class="ai-provider-actions">
                    <a class="button" href=VERIFY_URL target="_blank" rel="noopener noreferrer">"Open ChatGPT"</a>
                    <button type="button" class="ai-link" on:click=move |_| code.set(None)>"Cancel sign-in"</button>
                </div>
                <p role="status">"Waiting for ChatGPT…"</p>
            })}
            <Show when=move || tab.get() == ProviderTab::ChatGpt && cloud_selected()><HostedModels draft cloud=true/></Show>
        </section>
        <section class="ai-provider" aria-label="Your private models" data-selected="true" hidden=move || tab.get() != ProviderTab::Private>
            <div class="ai-provider-heading"><h3>"Your private models"</h3></div>
            <p>{move || if available() { "Your To-do server connects to the private model. The key stays on your server." } else { "Not configured on your server." }}</p>
            <Show when=move || tab.get() == ProviderTab::Private && available() && private_selected()><HostedModels draft cloud=false/></Show>
        </section>
    }
}

#[component]
fn HostedModels(draft: RwSignal<settings::Profile>, cloud: bool) -> impl IntoView {
    let route = Memo::new(move |_| draft.with(|p| p.hosted));
    let models = RwSignal::new(Vec::<relay::Model>::new());
    let note = RwSignal::new("Loading models…");
    let picking = RwSignal::new(false);
    Effect::new(move |_| {
        let Some(hosted) = route.get() else {
            return;
        };
        if matches!(hosted, Hosted::ChatGpt(_)) != cloud {
            return;
        }
        models.set(Vec::new());
        picking.set(false);
        note.set("Loading models…");
        leptos::task::spawn_local(async move {
            let result = hosted.models().await;
            if route.try_get_untracked().flatten() != Some(hosted) {
                return;
            }
            match result {
                Ok(items) if !items.is_empty() => {
                    draft.update(|p| {
                        if !items.iter().any(|item| item.id == p.hosted_model) {
                            p.hosted_model.clear();
                        }
                    });
                    models.set(items);
                }
                Ok(_) => note.set("No models are available."),
                Err(_) => note.set("Couldn’t load models."),
            }
        });
    });
    let id = if cloud {
        "ai-chatgpt-model"
    } else {
        "ai-private-model"
    };
    view! {
        <div class="ai-model-row">
            <span class="ai-meta">"Model"</span>
            <span class="ai-model-value ai-truncate">{move || draft.with(|p| {
                if p.hosted_model.is_empty() { "No model selected".into() }
                else { p.hosted_model.clone() }
            })}</span>
            <button type="button" class="button" aria-controls=id aria-expanded=move || picking.get().to_string()
                disabled=move || models.with(Vec::is_empty) on:click=move |_| picking.update(|open| *open = !*open)>"Choose model"</button>
        </div>
        <Show when=move || models.with(Vec::is_empty)><p class="ai-meta" role="status">{move || note.get()}</p></Show>
        <select id=id aria-label="Model" hidden=move || !picking.get()
            prop:value=move || draft.with(|p| p.hosted_model.clone())
            on:change=move |ev| { draft.update(|p| p.hosted_model = event_target_value(&ev)); picking.set(false); }>
            <option value="" disabled=true>"Choose a model"</option>
            {move || models.get().into_iter().map(|model| view! { <option value=model.id>{model.label}</option> }).collect_view()}
        </select>
    }
}
