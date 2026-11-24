//! Android's paired assistant operations. UniFFI only exposes account labels and opaque handles.
use super::*;
use todo_client::assistant::native_relay::VERIFY_URL;

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

#[derive(Clone, uniffi::Record)]
pub struct AssistantScopeReview {
    pub account: String,
    pub version: String,
    pub generation: u64,
    pub lists: Vec<String>,
    pub names: Vec<String>,
}

opaque_debug!(AssistantDeviceCode, AssistantScopeReview);

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

    fn apply_hosted_status(&self, status: relay::Status) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let route = session.settings.hosted;
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
            Ok(status) => self.apply_hosted_status(status),
            Err(error) => {
                let _ = self.refresh_relay();
                Err(error)
            }
        }
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
        self.apply_hosted_status(request(relay.status())?)
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
        self.refresh_relay()?;
        let route = route(account)?;
        let model = model.trim().to_owned();
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        if !valid_model(&model)
            || !session
                .relay_status
                .as_ref()
                .is_some_and(|status| available(status, route))
        {
            return Err(invalid("Choose an available account and model."));
        }
        let mut next = session.settings.clone();
        next.hosted = Some(route);
        next.hosted_model = model;
        save_settings(&self.path, &next)?;
        stop_check(&mut session);
        session.settings = next;
        session.problem = None;
        session.connection = AssistantConnection::Untested;
        session.receipts.clear();
        self.install(&session);
        Ok(())
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

    /// One bounded poll; None means the owner has not approved the code yet.
    pub fn poll_chatgpt(&self, attempt: String) -> Result<Option<String>, FfiError> {
        match request(self.paired()?.poll(id(attempt)?))? {
            relay::Poll::Pending => Ok(None),
            relay::Poll::SignedIn { account } => {
                self.refresh_relay()?;
                Ok(Some(account.id.to_string()))
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
