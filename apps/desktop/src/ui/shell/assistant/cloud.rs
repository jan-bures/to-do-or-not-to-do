use super::*;
use crate::chatgpt::{AccountView, Attempt};
use std::sync::Arc;
use todo_client::assistant::{
    Selection,
    native_relay::{Hosted, Relay, VERIFY_URL},
    relay,
};
use todo_core::{State, ids::ListId};

pub(super) struct Consent {
    account: Uuid,
    prompt: String,
    pub lists: BTreeSet<ListId>,
    pub previously_allowed: Option<BTreeSet<ListId>>,
    pub state: Option<Arc<State>>,
}

impl Consent {
    pub fn new(
        account: AccountView,
        prompt: String,
        (state, lists): (Option<Arc<State>>, BTreeSet<ListId>),
    ) -> Self {
        Self {
            account: account.id,
            prompt,
            lists,
            previously_allowed: account.consented_lists,
            state,
        }
    }
}

#[derive(Default)]
pub(super) struct Cloud {
    pub(super) relay: Option<Relay>,
    pub(super) sending: bool,
    pub status: Option<relay::Status>,
    pub error: Option<String>,
    pub consent: Option<Consent>,
    pub fixture: Option<AccountView>,
    pub fixture_enabled: bool,
    pub usage_paused: bool,
    pub code: Option<String>,
    pub signed_in: Option<(Uuid, String)>,
    loading: bool,
    consenting: bool,
}

impl Cloud {
    pub fn load(store: &Store) -> Self {
        Self {
            relay: store.session().map(|s| Relay::new(s.pairing_transport().1)),
            ..Self::default()
        }
    }
    pub fn available(&self) -> bool {
        self.fixture_enabled || self.status.as_ref().is_some_and(|s| s.chatgpt)
    }
    pub fn active(&self, profile: &Profile) -> Option<AccountView> {
        self.fixture
            .as_ref()
            .filter(|a| Some(a.id) == profile.chatgpt_account)
            .cloned()
            .or_else(|| {
                self.status
                    .as_ref()?
                    .accounts
                    .iter()
                    .find(|a| Some(a.id) == profile.chatgpt_account)
                    .map(|account| AccountView {
                        id: account.id,
                        label: account.label.clone(),
                        consented_lists: account.approved.then(|| account.approved_lists.clone()),
                    })
            })
    }
    pub fn saved(&self) -> Vec<relay::Account> {
        self.status
            .as_ref()
            .map_or_else(Vec::new, |s| s.accounts.clone())
    }
    pub fn close(&mut self) {
        *self = Self::default();
    }
    pub fn selection(&self, profile: &Profile) -> Option<Selection<DesktopProvider>> {
        let Some(hosted) = profile.hosted() else {
            return profile.selection();
        };
        if !valid_model(&profile.model) {
            return None;
        }
        let status = self.status.as_ref()?;
        let approved = match hosted {
            Hosted::Private if status.private_models => None,
            Hosted::ChatGpt(_) if status.chatgpt => self.active(profile)?.consented_lists,
            _ => return None,
        };
        Some(Selection {
            provider: Arc::new(self.relay.as_ref()?.provider(hosted)),
            model: profile.model.clone(),
            allow_billed: approved.is_some(),
            billed_list_scope: approved,
            access: status.access.clone(),
        })
    }
}

impl ShellView {
    pub(super) fn refresh_ai_relay(&mut self, cx: &mut Context<Self>) {
        let (Some(relay), Some(session)) = (self.ai.cloud.relay.clone(), self.store.session())
        else {
            return;
        };
        if self.ai.cloud.loading {
            return;
        }
        self.ai.cloud.loading = true;
        let work = session::spawn(
            &session.assistant_runtime(),
            async move { relay.status().await },
        );
        cx.spawn(async move |this, cx| {
            let result = work.await.unwrap_or(Err(ProviderError::Unavailable));
            let _ = this.update(cx, |this, cx| {
                this.ai.cloud.loading = false;
                if this.ai.stopped {
                    return;
                }
                match result {
                    Ok(status) => this.apply_ai_relay_status(status, cx),
                    Err(error) => this.ai.cloud.error = Some(cloud_problem(error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn apply_ai_relay_status(&mut self, status: relay::Status, cx: &mut Context<Self>) {
        let old_status = self.ai.cloud.status.as_ref();
        let changed = old_status.is_none_or(|s| {
            s.access != status.access
                || s.chatgpt != status.chatgpt
                || s.private_models != status.private_models
                || s.accounts != status.accounts
        });
        if let Some(setup) = &mut self.ai.settings
            && setup.draft.uses_relay()
            && setup.access_version.is_none()
        {
            setup.draft.access = status.access.clone();
            setup.access_version = Some(status.access_version);
        }
        self.ai.cloud.status = Some(status);
        self.ai.cloud.error = None;
        self.default_chatgpt_account(None, cx);
        if changed && self.ai.profile.uses_relay() {
            self.ai
                .controller
                .configure(self.ai.cloud.selection(&self.ai.profile));
            self.ai.clear_chat_state();
            self.ai.cloud.consent = None;
            self.ai.rate_until = None;
            self.ai.connection = ConnectionHealth::Unknown;
        }
        cx.notify();
    }

    pub(super) fn choose_ai_provider(&mut self, route: Option<Hosted>, cx: &mut Context<Self>) {
        let Some(setup) = self.ai.settings.as_mut() else {
            return;
        };
        let (cloud, private) = (
            matches!(route, Some(Hosted::ChatGpt(_))),
            route == Some(Hosted::Private),
        );
        self.ai.cloud.signed_in = None;
        if setup.draft.cloud == cloud && setup.draft.private_relay == private {
            cx.notify();
            return;
        }
        setup.invalidate();
        setup.draft.cloud = cloud;
        setup.draft.private_relay = private;
        setup.draft.model = if self.ai.profile.cloud == cloud
            && self.ai.profile.private_relay == private
            && (!cloud || setup.draft.chatgpt_account == self.ai.profile.chatgpt_account)
        {
            self.ai.profile.model.clone()
        } else {
            String::new()
        };
        setup.models.clear();
        setup.pick_model = false;
        self.ai.model_scroll.reset_to_top();
        self.ai.cloud.code = None;
        setup.access_version = None;
        if cloud || private {
            if let Some(status) = &self.ai.cloud.status {
                setup.draft.access = status.access.clone();
                setup.access_version = Some(status.access_version);
            }
        } else {
            setup.draft.access = if self.ai.profile.uses_relay() {
                Default::default()
            } else {
                self.ai.profile.access.clone()
            };
        }
        self.ai
            .model
            .update(cx, |i, cx| i.set_text(setup.draft.model.clone(), cx));
        cx.notify();
    }

    pub(super) fn sign_in_chatgpt(&mut self, loopback: bool, cx: &mut Context<Self>) {
        let (Some(relay), Some(session), Some(setup)) = (
            self.ai.cloud.relay.clone(),
            self.store.session(),
            self.ai.settings.as_mut(),
        ) else {
            return;
        };
        if !setup.draft.cloud || setup.abort.is_some() || session.client().revocation().is_revoked()
        {
            return;
        }
        setup.invalidate();
        let revision = setup.revision;
        let runtime = session.assistant_runtime();
        let (abort, cancel) = AbortHandle::new_pair();
        setup.abort = Some(abort);
        setup.check = Check::Running("Starting ChatGPT sign-in…");
        self.ai.cloud.code = None;
        self.ai.cloud.signed_in = None;
        let start_relay = relay.clone();
        cx.spawn(async move |this, cx| {
            let started = session::spawn(&runtime, async move {
                if loopback {
                    Attempt::start(&start_relay)
                        .await
                        .map(|(attempt, url)| SignIn::Loopback(attempt, url))
                } else {
                    let response = start_relay
                        .start(relay::SignIn::Device)
                        .await
                        .map_err(|_| chatgpt::Error::Network)?;
                    match response {
                        relay::SignInStarted::Device {
                            attempt,
                            user_code,
                            verification_url,
                        } if verification_url == VERIFY_URL => {
                            Ok(SignIn::Device(attempt, user_code))
                        }
                        _ => Err(chatgpt::Error::Callback),
                    }
                }
            })
            .await
            .unwrap_or(Err(chatgpt::Error::Network));
            let started = match started {
                Ok(started) => started,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.cloud_job_failed(revision, error.to_string(), cx)
                    });
                    return;
                }
            };
            let Ok(true) = this.update(cx, |this, cx| {
                let Some(setup) = &mut this.ai.settings else {
                    return false;
                };
                if setup.revision != revision || setup.abort.is_none() {
                    return false;
                }
                setup.check = Check::Running("Waiting for ChatGPT sign-in…");
                match &started {
                    SignIn::Loopback(_, url) if !crate::platform::win::open_authorization(url) => {
                        this.cloud_job_failed(revision, chatgpt::Error::Callback.to_string(), cx);
                        return false;
                    }
                    SignIn::Device(_, code) => this.ai.cloud.code = Some(code.clone()),
                    _ => {}
                }
                cx.notify();
                true
            }) else {
                return;
            };
            let completed = session::spawn(
                &runtime,
                Abortable::new(
                    async move {
                        let account = match started {
                            SignIn::Loopback(attempt, _) => attempt.finish(&relay).await?,
                            SignIn::Device(attempt, _) => loop {
                                tokio::time::sleep(Duration::from_secs(1)).await;
                                match relay
                                    .poll(attempt)
                                    .await
                                    .map_err(|_| chatgpt::Error::Network)?
                                {
                                    relay::Poll::Pending => continue,
                                    relay::Poll::SignedIn { account } => break account,
                                    relay::Poll::Expired => return Err(chatgpt::Error::TimedOut),
                                    relay::Poll::Failed => return Err(chatgpt::Error::Declined),
                                }
                            },
                        };
                        let status = relay.status().await.map_err(|_| chatgpt::Error::Network)?;
                        Ok::<_, chatgpt::Error>((account, status))
                    },
                    cancel,
                ),
            )
            .await;
            let _ = this.update(cx, |this, cx| {
                if !this
                    .ai
                    .settings
                    .as_ref()
                    .is_some_and(|s| s.revision == revision)
                {
                    return;
                }
                this.ai.cloud.code = None;
                match completed {
                    Ok(Ok(Ok((account, status)))) => {
                        this.finish_chatgpt_sign_in(revision, account, status, cx);
                    }
                    Ok(Ok(Err(error))) => this.cloud_job_failed(revision, error.to_string(), cx),
                    Ok(Err(_)) => {}
                    Err(_) => {
                        this.cloud_job_failed(revision, chatgpt::Error::Network.to_string(), cx)
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn finish_chatgpt_sign_in(
        &mut self,
        revision: Uuid,
        account: relay::Account,
        status: relay::Status,
        cx: &mut Context<Self>,
    ) {
        self.cloud_job_finished(revision, Vec::new(), cx);
        let had_selection = self.chatgpt_selection_valid();
        self.apply_ai_relay_status(status, cx);
        if !had_selection {
            self.default_chatgpt_account(Some(account.id), cx);
        }
        self.ai.cloud.signed_in = Some((account.id, account.label));
        cx.notify();
    }

    fn chatgpt_selection_valid(&self) -> bool {
        let saved = self.ai.cloud.saved();
        self.ai.settings.as_ref().is_some_and(|setup| {
            setup
                .draft
                .chatgpt_account
                .is_some_and(|id| saved.iter().any(|account| account.id == id))
        })
    }

    /// Without a valid account the model row stays hidden, so pick one: the preferred (newly
    /// signed-in) account, else the first saved one. Only for the ChatGPT provider; an existing
    /// valid choice is never replaced.
    fn default_chatgpt_account(&mut self, prefer: Option<Uuid>, cx: &mut Context<Self>) {
        let cloud = self
            .ai
            .settings
            .as_ref()
            .is_some_and(|setup| setup.draft.cloud);
        if !cloud {
            return;
        }
        let saved = self.ai.cloud.saved();
        let current = self
            .ai
            .settings
            .as_ref()
            .and_then(|setup| setup.draft.chatgpt_account);
        // Sign-in passes a preference only when the selection before the refresh was invalid.
        let keep = current.filter(|_| prefer.is_none());
        let pick = todo_client::assistant::default_chatgpt_account(&saved, keep, prefer);
        if pick == current {
            return;
        }
        if let Some(id) = pick {
            let signed_in = self.ai.cloud.signed_in.take();
            self.choose_chatgpt_account(id, cx);
            self.ai.cloud.signed_in = signed_in;
        }
    }

    pub(super) fn choose_chatgpt_account(&mut self, id: Uuid, cx: &mut Context<Self>) {
        self.ai.cloud.signed_in = None;
        if let Some(setup) = &mut self.ai.settings
            && setup.draft.chatgpt_account != Some(id)
        {
            setup.invalidate();
            self.ai.cloud.code = None;
            setup.draft.chatgpt_account = Some(id);
            setup.draft.model.clear();
            setup.models.clear();
            setup.pick_model = false;
        }
        cx.notify();
    }

    pub(super) fn list_chatgpt_models(&mut self, cx: &mut Context<Self>) {
        let (Some(relay), Some(session), Some(setup)) = (
            self.ai.cloud.relay.clone(),
            self.store.session(),
            self.ai.settings.as_mut(),
        ) else {
            return;
        };
        self.ai.cloud.signed_in = None;
        let Some(route) = setup.draft.hosted() else {
            return;
        };
        if setup.abort.is_some() {
            return;
        }
        setup.invalidate();
        let revision = setup.revision;
        let (abort, cancel) = AbortHandle::new_pair();
        setup.abort = Some(abort);
        setup.check = Check::Running("Loading models…");
        let work = session::spawn(
            &session.assistant_runtime(),
            Abortable::new(async move { relay.models(route).await }, cancel),
        );
        cx.spawn(async move |this, cx| {
            let result = match work.await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => return,
                Err(_) => Err(ProviderError::Unavailable),
            };
            let _ = this.update(cx, |this, cx| match result {
                Ok(models) => this.cloud_job_finished(revision, models, cx),
                Err(error) => {
                    let message = match route {
                        Hosted::ChatGpt(_) => cloud_problem(error),
                        Hosted::Private => error.private_relay_message().into(),
                    };
                    this.cloud_job_failed(revision, message, cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn cloud_job_finished(&mut self, revision: Uuid, models: Vec<Model>, cx: &mut Context<Self>) {
        if let Some(setup) = &mut self.ai.settings
            && setup.revision == revision
        {
            setup.abort = None;
            setup.check = Check::NotTested;
            self.ai.model_scroll.reset_to_top();
            setup.pick_model = !models.is_empty();
            setup.models = models;
            cx.notify();
        }
    }

    fn cloud_job_failed(&mut self, revision: Uuid, message: String, cx: &mut Context<Self>) {
        if let Some(setup) = &mut self.ai.settings
            && setup.revision == revision
        {
            setup.abort = None;
            setup.check = Check::Failed(message);
            self.ai.cloud.code = None;
            cx.notify();
        }
    }

    pub(super) fn pick_chatgpt_model(&mut self, model: String, cx: &mut Context<Self>) {
        self.ai.cloud.signed_in = None;
        if let Some(setup) = &mut self.ai.settings {
            setup.draft.model = model;
            setup.pick_model = false;
        }
        cx.notify();
    }

    pub(super) fn sign_out_chatgpt(&mut self, id: Uuid, cx: &mut Context<Self>) {
        let (Some(relay), Some(session), Some(setup)) = (
            self.ai.cloud.relay.clone(),
            self.store.session(),
            self.ai.settings.as_mut(),
        ) else {
            return;
        };
        setup.invalidate();
        setup.models.clear();
        setup.pick_model = false;
        setup.check = Check::Running("Signing out of ChatGPT…");
        self.ai.cloud.signed_in = None;
        self.ai.cloud.code = None;
        let revision = setup.revision;
        let work = session::spawn(&session.assistant_runtime(), async move {
            let signed_out = relay.sign_out(id).await?;
            Ok::<_, ProviderError>((signed_out, relay.status().await?))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await.unwrap_or(Err(ProviderError::Unavailable));
            let _ = this.update(cx, |this, cx| match result {
                Ok((done, status)) => {
                    this.cloud_job_finished(revision, Vec::new(), cx);
                    this.apply_ai_relay_status(status, cx);
                    if let Some(setup) = &mut this.ai.settings
                        && setup.draft.chatgpt_account == Some(id)
                    {
                        setup.draft.chatgpt_account = None;
                        if setup.draft.cloud {
                            setup.draft.model.clear();
                        }
                    }
                    if !done.revoked { this.ai.cloud.error = Some("Signed out on your server. Revocation wasn’t confirmed. You can disconnect the app in ChatGPT settings.".into()); }
                }
                Err(error) => this.cloud_job_failed(revision, cloud_problem(error), cx),
            });
        }).detach();
        cx.notify();
    }

    pub(super) fn save_relay_access(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let (Some(relay), Some(session), Some(setup), Some(status)) = (
            self.ai.cloud.relay.clone(),
            self.store.session(),
            self.ai.settings.as_mut(),
            self.ai.cloud.status.as_ref(),
        ) else {
            return false;
        };
        if !setup.draft.uses_relay() || setup.draft.access == status.access {
            return false;
        }
        let Some(access_version) = setup.access_version else {
            return true;
        };
        setup.invalidate();
        let revision = setup.revision;
        setup.check = Check::Running("Saving list access…");
        let access = setup.draft.access.clone();
        let work = session::spawn(&session.assistant_runtime(), async move {
            relay
                .access(relay::AccessUpdate {
                    access,
                    access_version,
                })
                .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = work.await.unwrap_or(Err(ProviderError::Unavailable));
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(status) => {
                        let version = status.access_version;
                        let access = status.access.clone();
                        this.apply_ai_relay_status(status, cx);
                        if let Some(setup) = this.ai.settings.as_mut().filter(|s| s.revision == revision) {
                            setup.access_version = Some(version);
                            setup.draft.access = access;
                            setup.check = Check::NotTested;
                            this.save_ai_settings(window, cx);
                        }
                    }
                    Err(_) => this.cloud_job_failed(revision, "Couldn’t save list access. Reopen settings to review the current list rules.".into(), cx),
                }
                cx.notify();
            });
        }).detach();
        cx.notify();
        true
    }

    pub(super) fn require_cloud_consent(&mut self, prompt: &str, cx: &mut Context<Self>) -> bool {
        if !self.ai.profile.cloud {
            return false;
        }
        if self.ai.cloud.consenting {
            return true;
        }
        let Some(account) = self.ai.cloud.active(&self.ai.profile) else {
            return true;
        };
        let scope = self.ai.controller.list_scope();
        if account.covers(&scope.1) {
            return false;
        }
        self.ai.consent_scroll.reset_to_top();
        self.ai.cloud.consent = Some(Consent::new(account, prompt.to_owned(), scope));
        cx.notify();
        true
    }

    pub(super) fn answer_cloud_consent(
        &mut self,
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(question) = self.ai.cloud.consent.take() else {
            return;
        };
        if accept && !self.ai.cloud.consenting {
            self.refresh_ai(cx);
            let scope = self.ai.controller.list_scope();
            if let Some(account) = self
                .ai
                .cloud
                .active(&self.ai.profile)
                .filter(|a| a.id == question.account)
            {
                if !scope.1.is_subset(&question.lists) {
                    self.ai.consent_scroll.reset_to_top();
                    self.ai.cloud.consent = Some(Consent::new(account, question.prompt, scope));
                } else if let (Some(relay), Some(session), Some(status)) = (
                    self.ai.cloud.relay.clone(),
                    self.store.session(),
                    self.ai.cloud.status.as_ref(),
                ) {
                    let lists = scope.1;
                    let consent = relay::Consent {
                        lists: lists.clone(),
                        access_version: status.access_version,
                    };
                    self.ai.cloud.consenting = true;
                    let work = session::spawn(&session.assistant_runtime(), async move {
                        relay.consent(account.id, consent).await
                    });
                    cx.spawn(async move |this, cx| {
                        let result = work.await.unwrap_or(Err(ProviderError::Unavailable));
                        let _ = this.update(cx, |this, cx| {
                            this.ai.cloud.consenting = false;
                            if this.ai.stopped || this.ai.profile.chatgpt_account != Some(question.account) { return; }
                            match result {
                                Ok(account) => {
                                    if let Some(status) = &mut this.ai.cloud.status && let Some(stored) = status.accounts.iter_mut().find(|a| a.id == account.id) { *stored = account; }
                                    this.ai.controller.set_billed_list_scope(lists);
                                    this.send_ai(question.prompt, cx);
                                }
                                Err(_) => {
                                    this.ai.notice = Some("Couldn’t save that approval. Send again to review the current list scope.".into());
                                    this.refresh_ai_relay(cx);
                                }
                            }
                            cx.notify();
                        });
                    }).detach();
                }
            }
        }
        if self.ai.cloud.consent.is_some() {
            window.focus(
                &self.ai.focus_handle("assistant-chatgpt-consent-cancel", cx),
                cx,
            );
        } else {
            self.focus_ai(window, cx);
        }
        cx.notify();
    }
}

enum SignIn {
    Device(Uuid, String),
    Loopback(Attempt, Zeroizing<String>),
}

pub(super) fn cloud_problem(error: ProviderError) -> String {
    error.chatgpt_message().into()
}
