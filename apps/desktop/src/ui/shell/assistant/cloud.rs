use super::*;
use crate::chatgpt::{AccountView, Accounts, CloudProvider};
use std::sync::Arc;
use todo_client::assistant::Selection;
use todo_core::{State, ids::ListId};

pub(super) struct Consent {
    account: String,
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
    accounts: Option<Accounts>,
    pub error: Option<String>,
    pub consent: Option<Consent>,
    pub fixture: Option<AccountView>,
    pub fixture_enabled: bool,
    pub usage_paused: bool,
}

impl Cloud {
    pub fn load(store: &Store) -> Self {
        let Some(session) = store.session() else {
            return Self::default();
        };
        match Accounts::load(session.data().clone()) {
            Ok(accounts) => Self {
                accounts: Some(accounts),
                ..Self::default()
            },
            Err(error) => Self {
                error: Some(error.to_string()),
                ..Self::default()
            },
        }
    }

    pub fn available(&self) -> bool {
        self.accounts.is_some() || self.fixture_enabled
    }
    pub fn active(&self) -> Option<AccountView> {
        self.fixture
            .clone()
            .or_else(|| self.accounts.as_ref().and_then(Accounts::active))
    }
    pub fn welcome_pending(&self) -> bool {
        self.active()
            .is_some_and(|account| account.plan_allowed && !account.welcomed)
    }

    pub fn saved(&self) -> Vec<AccountView> {
        self.accounts
            .as_ref()
            .map_or_else(Vec::new, Accounts::saved)
    }
    pub fn close(&mut self) {
        if let Some(accounts) = self.accounts.take() {
            accounts.close();
        }
        self.consent = None;
    }

    pub fn selection(&self, profile: &Profile) -> Option<Selection<DesktopProvider>> {
        if !profile.cloud {
            return profile.selection();
        }
        let active = self
            .active()
            .filter(|a| a.plan_allowed && profile.enabled)?;
        let provider = CloudProvider {
            accounts: self.accounts.clone()?,
            client_id: active.id,
        };
        Some(Selection {
            provider: Arc::new(DesktopProvider::ChatGPT(provider)),
            model: active.model?,
            allow_billed: active.consented_lists.is_some(),
            billed_list_scope: active.consented_lists,
            access: profile.access.clone(),
        })
    }
}

impl ShellView {
    pub(super) fn choose_ai_provider(&mut self, cloud: bool, cx: &mut Context<Self>) {
        if let Some(setup) = self.ai.settings.as_mut()
            && setup.draft.cloud != cloud
        {
            setup.invalidate();
            setup.draft.cloud = cloud;
            setup.draft.enabled = cloud == self.ai.profile.cloud && self.ai.profile.enabled;
            setup.models.clear();
            setup.pick_model = false;
            cx.notify();
        }
    }

    fn cloud_account_changed(&mut self, cx: &mut Context<Self>) {
        if self.ai.profile.cloud {
            self.ai
                .controller
                .configure(self.ai.cloud.selection(&self.ai.profile));
            self.ai.clear_chat_state();
            self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
            self.ai.rate_until = None;
            self.ai.connection = ConnectionHealth::Unknown;
            self.ai.cloud.usage_paused = false;
        }
        cx.notify();
    }

    pub(super) fn sign_in_chatgpt(
        &mut self,
        account: Option<String>,
        reconsent: bool,
        cx: &mut Context<Self>,
    ) {
        let (Some(accounts), Some(session), Some(setup)) = (
            self.ai.cloud.accounts.clone(),
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
        setup.check = Check::Running("Waiting for ChatGPT sign-in in your browser…");
        let prepare_accounts = accounts.clone();
        let prepare = session::spawn(&runtime, async move {
            prepare_accounts
                .prepare(account.as_deref(), reconsent)
                .await
        });
        cx.spawn(async move |this, cx| {
            let prepared = prepare.await.unwrap_or(Err(chatgpt::Error::Network));
            let (attempt, url) = match prepared {
                Ok(tuple) => tuple,
                Err(error) => {
                    let _ = this.update(cx, |this, cx| {
                        this.cloud_job_failed(revision, error.to_string(), cx)
                    });
                    return;
                }
            };
            let Ok(true) = this.update(cx, |this, cx| {
                if !this
                    .ai
                    .settings
                    .as_ref()
                    .is_some_and(|s| s.revision == revision && s.abort.is_some())
                {
                    return false;
                }
                if !crate::platform::win::open_authorization(&url) {
                    this.cloud_job_failed(revision, chatgpt::Error::Callback.to_string(), cx);
                    return false;
                }
                true
            }) else {
                return;
            };
            let completed = session::spawn(
                &runtime,
                Abortable::new(async move { accounts.complete(attempt).await }, cancel),
            )
            .await;
            let _ = this.update(cx, |this, cx| match completed {
                Ok(Ok(Ok(account))) => {
                    let current = this
                        .ai
                        .settings
                        .as_ref()
                        .is_some_and(|s| s.revision == revision);
                    this.cloud_account_changed(cx);
                    this.cloud_job_finished(revision, Vec::new(), cx);
                    if current && account.plan_allowed {
                        this.list_chatgpt_models(cx);
                    }
                }
                Ok(Ok(Err(error))) => this.cloud_job_failed(revision, error.to_string(), cx),
                Ok(Err(_)) => {}
                Err(_) => this.cloud_job_failed(revision, chatgpt::Error::Network.to_string(), cx),
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn choose_chatgpt_account(&mut self, account: AccountView, cx: &mut Context<Self>) {
        if !account.signed_in {
            self.sign_in_chatgpt(Some(account.id), false, cx);
            return;
        }
        let Some(accounts) = self.ai.cloud.accounts.as_ref() else {
            return;
        };
        if let Err(error) = accounts.choose(&account.id) {
            self.ai.cloud.error = Some(error.to_string());
        } else {
            self.cloud_account_changed(cx);
            if account.plan_allowed {
                self.list_chatgpt_models(cx);
            }
        }
        cx.notify();
    }

    pub(super) fn list_chatgpt_models(&mut self, cx: &mut Context<Self>) {
        let (Some(accounts), Some(session), Some(setup), Some(account)) = (
            self.ai.cloud.accounts.clone(),
            self.store.session(),
            self.ai.settings.as_mut(),
            self.ai.cloud.active(),
        ) else {
            return;
        };
        if !setup.draft.cloud || setup.abort.is_some() || !account.plan_allowed {
            return;
        }
        setup.invalidate();
        let revision = setup.revision;
        let (abort, cancel) = AbortHandle::new_pair();
        setup.abort = Some(abort);
        setup.check = Check::Running("Loading this account’s models…");
        let work = session::spawn(
            &session.assistant_runtime(),
            Abortable::new(async move { accounts.models(&account.id).await }, cancel),
        );
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(Ok(Ok(models))) => {
                    if this.ai.profile.cloud {
                        this.ai.connection = ConnectionHealth::Connected;
                    }
                    this.cloud_job_finished(revision, models, cx);
                }
                Ok(Ok(Err(error))) => this.cloud_job_failed(revision, cloud_problem(error), cx),
                Ok(Err(_)) => {}
                Err(_) => this.cloud_job_failed(revision, chatgpt::Error::Network.to_string(), cx),
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
            cx.notify();
        }
    }

    pub(super) fn pick_chatgpt_model(&mut self, model: String, cx: &mut Context<Self>) {
        let (Some(accounts), Some(account)) =
            (self.ai.cloud.accounts.as_ref(), self.ai.cloud.active())
        else {
            return;
        };
        if let Err(error) = accounts.model(&account.id, model) {
            self.ai.cloud.error = Some(error.to_string());
        } else {
            self.cloud_account_changed(cx);
        }
        if let Some(setup) = self.ai.settings.as_mut() {
            setup.pick_model = false;
        }
        cx.notify();
    }

    pub(super) fn welcome_chatgpt(&mut self, cx: &mut Context<Self>) {
        let (Some(accounts), Some(account)) =
            (self.ai.cloud.accounts.as_ref(), self.ai.cloud.active())
        else {
            return;
        };
        if let Err(error) = accounts.welcome(&account.id) {
            self.ai.cloud.error = Some(error.to_string());
        }
        cx.notify();
    }

    pub(super) fn sign_out_chatgpt(&mut self, cx: &mut Context<Self>) {
        let (Some(accounts), Some(session), Some(account)) = (
            self.ai.cloud.accounts.clone(),
            self.store.session(),
            self.ai.cloud.active(),
        ) else {
            return;
        };
        let runtime = session.assistant_runtime();
        if self.ai.profile.cloud {
            self.ai.controller.configure(None);
            self.ai.connection = ConnectionHealth::Unknown;
            self.ai.clear_chat_state();
            self.ai.cloud.consent = None;
            self.ai.composer.update(cx, |a, cx| a.set_text("", cx));
        }
        if let Some(setup) = &mut self.ai.settings {
            setup.invalidate();
            setup.models.clear();
            setup.pick_model = false;
            setup.check = Check::Running("Signing out of ChatGPT…");
        }
        let work = session::spawn(
            &runtime,
            async move { accounts.sign_out(&account.id).await },
        );
        cx.spawn(async move |this, cx| {
            let result = work.await.unwrap_or(Err(chatgpt::Error::Network));
            let _ = this.update(cx, |this, cx| {
                this.ai.cloud.error = result.err().map(|e| e.to_string());
                if let Some(setup) = &mut this.ai.settings
                    && matches!(setup.check, Check::Running("Signing out of ChatGPT…"))
                {
                    setup.check = Check::NotTested;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn require_cloud_consent(&mut self, prompt: &str, cx: &mut Context<Self>) -> bool {
        if !self.ai.profile.cloud {
            return false;
        }
        let Some(account) = self.ai.cloud.active() else {
            return true;
        };
        if !account.plan_allowed {
            self.ai.notice = Some(chatgpt::Error::NoGrant.to_string());
            cx.notify();
            return true;
        }
        // Use the controller's snapshot, not a second replica read that could race with sync.
        let scope = self.ai.controller.list_scope();
        if account.covers(&scope.1) {
            return false;
        }
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
        if accept {
            self.refresh_ai(cx);
            if let Some(account) = self.ai.cloud.active().filter(|a| a.id == question.account)
                && let Some(accounts) = self.ai.cloud.accounts.as_ref()
            {
                let scope = self.ai.controller.list_scope();
                if !scope.1.is_subset(&question.lists) {
                    // Sync may have added a list since the owner saw the dialog. Show it first.
                    self.ai.cloud.consent = Some(Consent::new(account, question.prompt, scope));
                } else {
                    let (_, lists) = scope;
                    match accounts.consent(&question.account, lists.clone()) {
                        Ok(()) => {
                            self.ai.controller.set_billed_list_scope(lists);
                            self.send_ai(question.prompt, cx);
                        }
                        Err(error) => self.ai.notice = Some(error.to_string()),
                    }
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

pub(super) fn cloud_problem(error: ProviderError) -> String {
    match error {
        ProviderError::Unauthorized => "ChatGPT didn't authorize plan usage. Check your account or sign in again.",
        ProviderError::RateLimited { .. } => "ChatGPT usage limit reached. Review your plan or this app’s limit in ChatGPT settings.",
        ProviderError::Unavailable | ProviderError::Timeout => "ChatGPT is unavailable. Nothing will be retried automatically.",
        ProviderError::ModelMissing => "This model isn't available to your ChatGPT account. Choose another model in AI settings.",
        ProviderError::Unsupported => "ChatGPT couldn't run this request or tool.",
        _ => "ChatGPT didn't finish the reply. No changes were prepared.",
    }.into()
}
