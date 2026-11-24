//! Android's paired assistant operations. UniFFI only exposes account labels and opaque handles.
use super::*;
use todo_client::assistant::{
    browser_sign_in::Attempt, default_chatgpt_account, native_relay::VERIFY_URL,
};
use tokio::task::{AbortHandle, JoinHandle};

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AssistantHostedAccount {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AssistantHosted {
    pub chatgpt: bool,
    pub device_code: bool,
    pub private_models: bool,
    pub accounts: Vec<AssistantHostedAccount>,
    pub selected_account: Option<String>,
    pub private_selected: bool,
    pub model: String,
    pub disabled_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct AssistantHostedModel {
    pub id: String,
    pub label: String,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantDeviceCode {
    pub attempt: String,
    pub user_code: String,
    pub verification_url: String,
}

#[derive(uniffi::Object)]
pub struct AssistantBrowserSignIn {
    authorize_url: Zeroizing<String>,
    task: Mutex<
        Option<JoinHandle<Result<relay::Account, todo_client::assistant::browser_sign_in::Error>>>,
    >,
    abort: AbortHandle,
}

impl Drop for AssistantBrowserSignIn {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[uniffi::export]
impl AssistantBrowserSignIn {
    pub fn authorization_url(&self) -> String {
        self.authorize_url.to_string()
    }

    pub async fn finish(&self) -> Result<AssistantHostedAccount, FfiError> {
        let task = locked(&self.task)
            .take()
            .ok_or_else(|| invalid("Start a new sign-in."))?;
        let account = task
            .await
            .map_err(|_| invalid("ChatGPT sign-in didn’t finish. Start again."))?
            .map_err(|error| invalid(error.to_string()))?;
        Ok(account_view(account))
    }
}

fn account_view(account: relay::Account) -> AssistantHostedAccount {
    AssistantHostedAccount {
        id: account.id.to_string(),
        label: one_line(&account.label),
    }
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantScopeReview {
    pub account: String,
    pub version: String,
    pub generation: u64,
    pub lists: Vec<String>,
    pub names: Vec<String>,
}

opaque_debug!(
    AssistantBrowserSignIn,
    AssistantDeviceCode,
    AssistantScopeReview
);

fn request<T>(work: impl Future<Output = Result<T, ProviderError>>) -> Result<T, FfiError> {
    runtime()?.block_on(work).map_err(|err| {
        invalid(match err {
            ProviderError::NotPrivate => {
                "List access changed on another device. Check the current rules and try again."
                    .into()
            }
            _ => err.to_string(),
        })
    })
}

fn id(value: String) -> Result<Uuid, FfiError> {
    value
        .parse()
        .map_err(|_| invalid("That assistant selection is no longer available."))
}

fn route(account: Option<String>) -> Result<Hosted, FfiError> {
    account
        .map(id)
        .transpose()
        .map(|id| id.map_or(Hosted::Private, Hosted::ChatGpt))
}

fn available(status: &relay::Status, route: Hosted) -> bool {
    match route {
        Hosted::Private => status.private_models,
        Hosted::ChatGpt(id) => status.accounts.iter().any(|a| a.id == id),
    }
}

impl AndroidAssistant {
    fn paired(&self) -> Result<&Relay, FfiError> {
        self.require_active(&mut locked(&self.session))?;
        self.relay
            .as_ref()
            .ok_or_else(|| invalid("Pair this phone to use the server assistant."))
    }

    fn apply_hosted_status(
        &self,
        status: relay::Status,
        signed_in: Option<Uuid>,
    ) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let route = session.settings.hosted;
        let account = match route {
            Some(Hosted::ChatGpt(id)) => {
                default_chatgpt_account(&status.accounts, Some(id), signed_in)
            }
            None if signed_in.is_some() => {
                default_chatgpt_account(&status.accounts, None, signed_in)
            }
            _ => None,
        };
        if let Some(account) = account
            .map(Hosted::ChatGpt)
            .filter(|choice| Some(*choice) != route)
        {
            self.save_relay_choice(&mut session, account, None)?;
            session.relay_status = Some(status);
            self.install(&session);
            self.changes.notify();
            return Ok(());
        }
        let (ready, approval_changed) = match route {
            Some(Hosted::ChatGpt(id)) => {
                let old = session
                    .relay_status
                    .as_ref()
                    .and_then(|s| s.accounts.iter().find(|a| a.id == id));
                let new = status.accounts.iter().find(|a| a.id == id);
                (
                    old.is_some() != new.is_some(),
                    old.zip(new).is_some_and(|(old, new)| {
                        old.approved != new.approved || old.approved_lists != new.approved_lists
                    }),
                )
            }
            Some(Hosted::Private) => (
                session
                    .relay_status
                    .as_ref()
                    .is_none_or(|s| s.private_models != status.private_models),
                false,
            ),
            None => (false, false),
        };
        let scope_changed = session
            .relay_status
            .as_ref()
            .is_none_or(|s| s.access != status.access);
        session.relay_status = Some(status);
        if route.is_some() && (ready || scope_changed || approval_changed) {
            self.install(&session);
        }
        self.changes.notify();
        Ok(())
    }

    pub(super) fn hosted_view(&self, session: &Session) -> Option<AssistantHosted> {
        let status = session.relay_status.as_ref()?;
        Some(AssistantHosted {
            chatgpt: status.chatgpt,
            device_code: status.chatgpt_device_code,
            private_models: status.private_models,
            accounts: status
                .accounts
                .iter()
                .map(|account| AssistantHostedAccount {
                    id: account.id.to_string(),
                    label: one_line(&account.label),
                })
                .collect(),
            selected_account: match session.settings.hosted {
                Some(Hosted::ChatGpt(id)) => Some(id.to_string()),
                _ => None,
            },
            private_selected: session.settings.hosted == Some(Hosted::Private),
            model: session.settings.hosted_model.clone(),
            disabled_reason: status.disabled_reason.clone(),
        })
    }

    pub(super) fn hosted_selection(
        &self,
        session: &Session,
        route: Hosted,
    ) -> Option<Selection<NativeProvider>> {
        let status = session.relay_status.as_ref()?;
        if !available(status, route) || !valid_model(&session.settings.hosted_model) {
            return None;
        }
        let account = match route {
            Hosted::ChatGpt(id) => status.accounts.iter().find(|a| a.id == id),
            _ => None,
        };
        Some(Selection {
            provider: Arc::new(self.relay.as_ref()?.provider(route)),
            model: session.settings.hosted_model.clone(),
            access: status.access.clone(),
            allow_billed: account.is_some_and(|a| a.approved),
            billed_list_scope: account.map(|a| a.approved_lists.clone()),
        })
    }

    pub(super) fn set_hosted_access(
        &self,
        id: String,
        collection: bool,
        access: AssistantAccess,
    ) -> Result<(), FfiError> {
        let relay = self.paired()?;
        let status = locked(&self.session)
            .relay_status
            .clone()
            .ok_or_else(|| invalid("Reload AI settings to get list access from your server."))?;
        let mut rules = status.access;
        self.engine.client.with_confirmed(|state| {
            if collection {
                let id = collection_id(&id)?;
                if !state.is_live(id.into()) {
                    return Err(invalid("That collection is no longer available."));
                }
                rules.set_collection(state, id, access.into());
            } else {
                let id = list_id(&id)?;
                if !state.is_live(id.into()) {
                    return Err(invalid("That list is no longer available."));
                }
                rules.lists.insert(id, access.into());
            }
            Ok(())
        })?;
        self.chat.cancel();
        let result = request(relay.access(relay::AccessUpdate {
            access: rules,
            access_version: status.access_version,
        }));
        match result {
            Ok(status) => self.apply_hosted_status(status, None),
            Err(error) => {
                let _ = self.refresh_relay();
                Err(error)
            }
        }
    }
}

impl AndroidAssistant {
    fn set_relay_choice(&self, route: Hosted, model: Option<String>) -> Result<(), FfiError> {
        self.refresh_relay()?;
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        if !session
            .relay_status
            .as_ref()
            .is_some_and(|status| available(status, route))
        {
            return Err(invalid("Choose an available account and model."));
        }
        if session.settings.hosted == Some(route) && model.is_none() {
            return Ok(());
        }
        self.save_relay_choice(&mut session, route, model)?;
        self.install(&session);
        self.changes.notify();
        Ok(())
    }

    fn save_relay_choice(
        &self,
        session: &mut Session,
        route: Hosted,
        model: Option<String>,
    ) -> Result<(), FfiError> {
        let mut next = session.settings.clone();
        next.hosted = Some(route);
        next.hosted_model = model.unwrap_or_default();
        save_settings(&self.path, &next)?;
        stop_check(session);
        session.settings = next;
        session.problem = None;
        session.connection = AssistantConnection::Untested;
        session.receipts.clear();
        Ok(())
    }

    fn apply_chatgpt_sign_in(&self, account: Uuid) -> Result<(), FfiError> {
        self.apply_hosted_status(request(self.paired()?.status())?, Some(account))
    }
}

#[uniffi::export]
impl AndroidAssistant {
    /// Refresh the paired server's assistant status on the IO dispatcher.
    pub fn refresh_relay(&self) -> Result<(), FfiError> {
        if self.relay.is_none() {
            return Ok(());
        }
        let relay = self.paired()?;
        self.apply_hosted_status(request(relay.status())?, None)
    }

    pub fn relay_models(
        &self,
        account: Option<String>,
    ) -> Result<Vec<AssistantHostedModel>, FfiError> {
        let relay = self.paired()?;
        Ok(request(relay.models(route(account)?))?
            .into_iter()
            .map(|m| AssistantHostedModel {
                id: m.id,
                label: m.label,
            })
            .collect())
    }

    pub fn select_relay(&self, account: Option<String>, model: String) -> Result<(), FfiError> {
        let model = model.trim();
        if !valid_model(model) {
            return Err(invalid("Choose an available account and model."));
        }
        self.set_relay_choice(route(account)?, Some(model.into()))
    }

    pub fn select_chatgpt_account(&self, account: String) -> Result<(), FfiError> {
        self.set_relay_choice(Hosted::ChatGpt(id(account)?), None)
    }

    /// The browser returns to a checked loopback listener; only the paired server exchanges credentials.
    pub fn start_chatgpt_browser(&self) -> Result<Arc<AssistantBrowserSignIn>, FfiError> {
        let relay = self.paired()?.clone();
        let runtime = runtime()?;
        let (attempt, authorize_url) = runtime
            .block_on(Attempt::start(&relay))
            .map_err(|error| invalid(error.to_string()))?;
        let task = runtime.spawn(async move { attempt.finish(&relay).await });
        Ok(Arc::new(AssistantBrowserSignIn {
            authorize_url,
            abort: task.abort_handle(),
            task: Mutex::new(Some(task)),
        }))
    }

    pub fn start_chatgpt(&self) -> Result<AssistantDeviceCode, FfiError> {
        let result = request(self.paired()?.start(relay::SignIn::Device))?;
        match result {
            relay::SignInStarted::Device {
                attempt,
                user_code,
                verification_url,
            } if verification_url == VERIFY_URL => Ok(AssistantDeviceCode {
                attempt: attempt.to_string(),
                user_code,
                verification_url,
            }),
            _ => Err(invalid("Couldn’t start ChatGPT sign-in.")),
        }
    }

    /// Apply a completed browser sign-in once the app is back in the foreground.
    pub fn finish_chatgpt_browser(&self, account: String) -> Result<(), FfiError> {
        self.apply_chatgpt_sign_in(id(account)?)
    }

    /// One bounded poll; None means the owner has not approved the code yet.
    pub fn poll_chatgpt(
        &self,
        attempt: String,
    ) -> Result<Option<AssistantHostedAccount>, FfiError> {
        match request(self.paired()?.poll(id(attempt)?))? {
            relay::Poll::Pending => Ok(None),
            relay::Poll::SignedIn { account } => {
                self.apply_chatgpt_sign_in(account.id)?;
                Ok(Some(account_view(account)))
            }
            relay::Poll::Expired => Err(invalid("The ChatGPT sign-in code expired. Start again.")),
            relay::Poll::Failed => Err(invalid("ChatGPT sign-in didn’t finish. Start again.")),
        }
    }

    pub fn sign_out_chatgpt(&self, account: String) -> Result<bool, FfiError> {
        self.chat.cancel();
        let result = request(self.paired()?.sign_out(id(account)?))?;
        self.refresh_relay()?;
        Ok(result.revoked)
    }

    /// Preview the current cloud scope, only when an explicit approval is needed.
    pub fn relay_scope_review(&self) -> Result<Option<AssistantScopeReview>, FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let Some(Hosted::ChatGpt(id)) = session.settings.hosted else {
            return Ok(None);
        };
        let status = session
            .relay_status
            .as_ref()
            .ok_or_else(|| invalid("Reload AI settings to reconnect to ChatGPT."))?;
        let account = status
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| invalid("This ChatGPT account is no longer connected."))?;
        let (state, scope) = self.chat.list_scope();
        if account.approved && scope.is_subset(&account.approved_lists) {
            return Ok(None);
        }
        let mut names = state.map_or_else(Vec::new, |state| {
            scope
                .iter()
                .filter_map(|id| state.lists.get(id).map(|l| one_line(&l.name)))
                .collect()
        });
        names.sort();
        Ok(Some(AssistantScopeReview {
            account: id.to_string(),
            version: status.access_version.to_string(),
            generation: self.chat.view().generation,
            lists: scope.iter().map(ToString::to_string).collect(),
            names,
        }))
    }

    pub fn approve_relay_scope(&self, shown: AssistantScopeReview) -> Result<(), FfiError> {
        let relay = self.paired()?;
        let (account, version) = (id(shown.account)?, id(shown.version)?);
        let lists = shown
            .lists
            .iter()
            .map(|id| list_id(id))
            .collect::<Result<BTreeSet<_>, _>>()?;
        {
            let session = locked(&self.session);
            if session.settings.hosted != Some(Hosted::ChatGpt(account))
                || self.chat.view().generation != shown.generation
                || self.chat.list_scope().1 != lists
            {
                return Err(invalid(
                    "List access changed. Review the new list scope before sending.",
                ));
            }
        }
        let approved = request(relay.consent(
            account,
            relay::Consent {
                lists,
                access_version: version,
            },
        ))?;
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        if session.settings.hosted != Some(Hosted::ChatGpt(account))
            || self.chat.view().generation != shown.generation
        {
            return Err(invalid(
                "The selected account changed. Review it before sending.",
            ));
        }
        if let Some(status) = &mut session.relay_status
            && let Some(previous) = status.accounts.iter_mut().find(|a| a.id == account)
        {
            *previous = approved;
            self.chat
                .set_billed_list_scope(previous.approved_lists.clone());
        }
        Ok(())
    }
}
