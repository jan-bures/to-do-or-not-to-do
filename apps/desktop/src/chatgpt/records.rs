use super::{Attempt, Error, Secret, Service, Tokens, now};
use crate::{config::DataDir, platform::dpapi};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{ErrorKind, Write},
    os::windows::fs::OpenOptionsExt,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
};
use todo_client::assistant::provider::Model;
use uuid::Uuid;
use zeroize::Zeroizing;

const FILE: &str = "assistant-chatgpt.bin";
const LOCK: &str = "assistant-chatgpt.lock";
const PURPOSE: &[u8] = b"to-do-or-not-to-do/assistant-chatgpt/v1";
const MAX_SIZE: u64 = 256 * 1024;
const MAX_ACCOUNTS: usize = 8;

#[derive(Clone, Deserialize, Serialize)]
struct Account {
    subject: String,
    email: Option<String>,
    name: Option<String>,
    tokens: Option<Tokens>,
    model: Option<String>,
    consent: bool,
    welcomed: bool,
}

#[derive(Clone, Deserialize, Serialize)]
struct Record {
    host: Uuid,
    active: Option<String>,
    accounts: BTreeMap<String, Account>,
    /// An issued registration with no verified account yet. A new sign-in resumes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_client: Option<String>,
}

#[derive(Clone)]
pub struct AccountView {
    pub id: String,
    pub label: String,
    pub model: Option<String>,
    pub signed_in: bool,
    pub plan_allowed: bool,
    pub consent: bool,
    pub welcomed: bool,
}

#[derive(Clone)]
pub struct Accounts(Arc<Inner>);

struct Inner {
    data: DataDir,
    service: Service,
    /// Keeps another process using the same pairing from racing a rotating token. Opened with
    /// delete sharing so a pairing can still be removed during sign-out or revocation.
    _lease: File,
    operation: tokio::sync::Mutex<()>,
    closed: AtomicBool,
    record: Mutex<Record>,
}

impl Accounts {
    pub fn load(data: DataDir) -> Result<Self, Error> {
        data.check().map_err(|_| Error::Storage)?;
        let path = data.path().join(LOCK);
        match todo_fs::create_private_file(&path) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(_) => return Err(Error::Storage),
        }
        todo_fs::verify_private(&path, todo_fs::Private::File).map_err(|_| Error::Storage)?;
        const SHARE_ALL: u32 = 7;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(SHARE_ALL)
            .open(&path)
            .map_err(|_| Error::Storage)?;
        lease.try_lock().map_err(|_| Error::Storage)?;
        let record = match todo_fs::read_private_file(&data.path().join(FILE), MAX_SIZE) {
            Ok(sealed) => {
                let plain =
                    Zeroizing::new(dpapi::open(&sealed, PURPOSE).map_err(|_| Error::Storage)?);
                serde_json::from_slice(&plain).map_err(|_| Error::Storage)?
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Record {
                host: Uuid::new_v4(),
                active: None,
                accounts: BTreeMap::new(),
                pending_client: None,
            },
            Err(_) => return Err(Error::Storage),
        };
        Ok(Self(Arc::new(Inner {
            data,
            service: Service::new()?,
            _lease: lease,
            operation: tokio::sync::Mutex::new(()),
            closed: AtomicBool::new(false),
            record: Mutex::new(record),
        })))
    }

    fn record(&self) -> MutexGuard<'_, Record> {
        self.0.record.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn store(&self, record: &Record) -> Result<(), Error> {
        if self.0.closed.load(Ordering::Acquire) {
            return Err(Error::Storage);
        }
        let data = &self.0.data;
        data.check().map_err(|_| Error::Storage)?;
        let plain = Zeroizing::new(serde_json::to_vec(record).map_err(|_| Error::Storage)?);
        let sealed = dpapi::seal(&plain, PURPOSE).map_err(|_| Error::Storage)?;
        if sealed.len() as u64 > MAX_SIZE {
            return Err(Error::Storage);
        }
        let temporary = data
            .path()
            .join(format!("assistant-chatgpt-{}.tmp", Uuid::new_v4()));
        let mut file = todo_fs::create_private_file(&temporary).map_err(|_| Error::Storage)?;
        let result = file.write_all(&sealed).and_then(|()| file.sync_all());
        drop(file);
        let result =
            result.and_then(|()| todo_fs::replace_durably(&temporary, &data.path().join(FILE)));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result.map_err(|_| Error::Storage)
    }

    pub fn active(&self) -> Option<AccountView> {
        let record = self.record();
        let id = record.active.as_ref()?;
        record.accounts.get(id).map(|a| view(id, a))
    }

    pub fn saved(&self) -> Vec<AccountView> {
        self.record()
            .accounts
            .iter()
            .map(|(id, a)| view(id, a))
            .collect()
    }

    pub async fn prepare(
        &self,
        id: Option<&str>,
        reconsent: bool,
    ) -> Result<(Attempt, Zeroizing<String>), Error> {
        let _operation = self.0.operation.lock().await;
        let record = self.record().clone();
        let account = id.and_then(|id| record.accounts.get(id));
        if id.is_some() && account.is_none() {
            return Err(Error::Protocol);
        }
        if id.is_none() && record.accounts.len() >= MAX_ACCOUNTS {
            return Err(Error::Storage);
        }
        // Persist the opaque host ID before starting the browser flow, even if it is canceled.
        self.store(&record)?;
        let email = account.and_then(|a| a.email.as_deref());
        let hint = account
            .and_then(|a| a.tokens.as_ref())
            .map(|t| t.id.0.as_str());
        Attempt::prepare(
            record.host,
            id.or(record.pending_client.as_deref()),
            email,
            hint,
            reconsent,
        )
        .await
    }

    pub async fn complete(&self, attempt: Attempt) -> Result<AccountView, Error> {
        let grant = attempt.receive().await?;
        let _operation = self.0.operation.lock().await;
        self.complete_grant(grant, |grant| self.0.service.exchange(grant))
            .await
    }

    async fn complete_grant<F, Fut>(
        &self,
        grant: super::Grant,
        exchange: F,
    ) -> Result<AccountView, Error>
    where
        F: FnOnce(super::Grant) -> Fut,
        Fut: Future<Output = Result<(super::authorization::Identity, Tokens), Error>>,
    {
        let client = grant.client.clone();
        {
            let mut current = self.record();
            if !current.accounts.contains_key(&client)
                && current.pending_client.as_ref() != Some(&client)
            {
                // The server has registered this client already. Save its ID even if the code
                // expires, the exchange fails, or sign-in is canceled before identity is verified.
                let mut next = current.clone();
                next.pending_client = Some(client.clone());
                self.store(&next)?;
                *current = next;
            }
        }
        let (identity, tokens) = exchange(grant).await?;
        let mut current = self.record();
        let mut next = current.clone();
        let old = next.accounts.get(&client);
        if old.is_some_and(|a| a.subject != identity.sub) {
            return Err(Error::Identity);
        }
        let account = Account {
            subject: identity.sub,
            email: identity.email,
            name: identity.name,
            tokens: Some(tokens),
            model: old.and_then(|a| a.model.clone()),
            consent: old.is_some_and(|a| a.consent),
            welcomed: old.is_some_and(|a| a.welcomed),
        };
        let display = view(&client, &account);
        next.accounts.insert(client.clone(), account);
        if next.pending_client.as_ref() == Some(&client) {
            next.pending_client = None;
        }
        next.active = Some(client);
        self.store(&next)?;
        *current = next;
        Ok(display)
    }

    pub fn choose(&self, id: &str) -> Result<(), Error> {
        let mut current = self.record();
        if !current.accounts.get(id).is_some_and(|a| a.tokens.is_some()) {
            return Err(Error::Expired);
        }
        let mut next = current.clone();
        next.active = Some(id.to_owned());
        self.store(&next)?;
        *current = next;
        Ok(())
    }

    fn edit(&self, id: &str, configure: impl FnOnce(&mut Account)) -> Result<(), Error> {
        let mut current = self.record();
        let mut next = current.clone();
        configure(next.accounts.get_mut(id).ok_or(Error::Expired)?);
        self.store(&next)?;
        *current = next;
        Ok(())
    }

    pub fn consent(&self, id: &str) -> Result<(), Error> {
        self.edit(id, |a| a.consent = true)
    }
    pub fn welcome(&self, id: &str) -> Result<(), Error> {
        self.edit(id, |a| a.welcomed = true)
    }
    pub fn model(&self, id: &str, model: String) -> Result<(), Error> {
        self.edit(id, |a| a.model = Some(model))
    }

    pub async fn sign_out(&self, id: &str) -> Result<(), Error> {
        self.sign_out_with(id, |refresh| async move {
            self.0.service.revoke(id, &refresh).await
        })
        .await
    }

    async fn sign_out_with<F, Fut>(&self, id: &str, revoke: F) -> Result<(), Error>
    where
        F: FnOnce(Secret) -> Fut,
        Fut: Future<Output = Result<(), Error>>,
    {
        let _operation = self.0.operation.lock().await;
        let (refresh, has_access) = {
            let record = self.record();
            let tokens = record.accounts.get(id).and_then(|a| a.tokens.as_ref());
            (
                tokens.and_then(|tokens| tokens.refresh.clone()),
                tokens.is_some_and(|tokens| !tokens.access.0.is_empty()),
            )
        };
        let revoke = match refresh {
            Some(refresh) => revoke(refresh).await,
            None if has_access => Err(Error::NotRevoked),
            None => Ok(()),
        };
        self.clear_tokens(id)?;
        revoke.map_err(|_| Error::NotRevoked)
    }

    fn clear_tokens(&self, id: &str) -> Result<(), Error> {
        let result = self.edit(id, |a| a.tokens = None);
        if result.is_err() {
            // Never re-enable egress from memory after the owner has signed out, even if the
            // protected file couldn't be replaced. The UI reports the failed disk cleanup.
            if let Some(account) = self.record().accounts.get_mut(id) {
                account.tokens = None;
            }
        }
        result
    }

    pub fn close(&self) {
        let mut record = self.record();
        self.0.closed.store(true, Ordering::Release);
        record.accounts.clear();
        record.active = None;
        record.pending_client = None;
    }

    pub(super) fn service(&self) -> &Service {
        &self.0.service
    }

    pub(super) async fn access(&self, id: &str, require_consent: bool) -> Result<Secret, Error> {
        // A completed rotation must save even if the owner cancels the inference that requested
        // it. Sign-out waits on this operation, and close prevents any post-unpair disk write.
        let this = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move { this.access_serialized(&id, require_consent).await })
            .await
            .map_err(|_| Error::Storage)?
    }

    async fn access_serialized(&self, id: &str, require_consent: bool) -> Result<Secret, Error> {
        self.access_with(id, require_consent, |old| async move {
            self.0.service.refresh(id, &old).await
        })
        .await
    }

    async fn access_with<F, Fut>(
        &self,
        id: &str,
        require_consent: bool,
        refresh: F,
    ) -> Result<Secret, Error>
    where
        F: FnOnce(Tokens) -> Fut,
        Fut: Future<Output = Result<Tokens, Error>>,
    {
        let _operation = self.0.operation.lock().await;
        let account = self
            .record()
            .accounts
            .get(id)
            .cloned()
            .ok_or(Error::Expired)?;
        let old = account.tokens.ok_or(Error::Expired)?;
        if !old.plan_allowed() || (require_consent && !account.consent) {
            return Err(Error::NoGrant);
        }
        if old.expires > now().saturating_add(90) {
            return Ok(old.access);
        }
        let next = match refresh(old).await {
            Ok(next) => next,
            Err(Error::Expired) => {
                self.clear_tokens(id)?;
                return Err(Error::Expired);
            }
            Err(e) => return Err(e),
        };
        let access = next.access.clone();
        let allowed = next.plan_allowed();
        self.edit(id, |a| a.tokens = Some(next))?;
        if allowed {
            Ok(access)
        } else {
            Err(Error::NoGrant)
        }
    }

    pub async fn models(
        &self,
        id: &str,
    ) -> Result<Vec<Model>, todo_client::assistant::provider::ProviderError> {
        super::responses::models(self, id).await
    }
}

fn view(id: &str, a: &Account) -> AccountView {
    let account = a
        .email
        .as_deref()
        .or(a.name.as_deref())
        .unwrap_or("ChatGPT account");
    let suffix = id.get(id.len().saturating_sub(6)..).unwrap_or(id);
    AccountView {
        id: id.to_owned(),
        label: format!("{account} · {suffix}"),
        model: a.model.clone(),
        signed_in: a.tokens.is_some(),
        plan_allowed: a.tokens.as_ref().is_some_and(Tokens::plan_allowed),
        consent: a.consent,
        welcomed: a.welcomed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PairingId, Paths};
    use std::sync::atomic::AtomicUsize;
    const ID: &str = "oaiapp_fixture";

    fn tokens(access: &str, refresh: &str, expires: u64) -> Tokens {
        Tokens {
            access: Secret(Zeroizing::new(access.into())),
            refresh: Some(Secret(Zeroizing::new(refresh.into()))),
            id: Secret(Zeroizing::new("sample-identity".into())),
            expires,
            scope: "openid email profile offline_access resource.invoke chatgpt.tokens.use.direct"
                .into(),
        }
    }

    fn fixture() -> (tempfile::TempDir, DataDir, Accounts) {
        let temp = tempfile::tempdir_in("target").expect("fixture directory");
        let data = Paths::in_dir(temp.path().to_owned()).data(&PairingId::random());
        std::fs::create_dir_all(data.path()).expect("fixture pairing");
        let accounts = Accounts::load(data.clone()).expect("fixture account store");
        {
            let mut record = accounts.record();
            record.active = Some(ID.into());
            record.accounts.insert(
                ID.into(),
                Account {
                    subject: "fixture".into(),
                    email: Some("you@example.com".into()),
                    name: None,
                    tokens: Some(tokens("initial-access", "initial-refresh", 0)),
                    model: Some("sample-model".into()),
                    consent: false,
                    welcomed: false,
                },
            );
            accounts.store(&record).expect("save fixture account");
        }
        (temp, data, accounts)
    }

    #[test]
    fn old_records_keep_the_same_uuid_and_do_not_require_a_pending_client() {
        let old = r#"{"host":"12345678-1234-4123-8123-123456789abc","active":null,"accounts":{}}"#;
        let record: Record = serde_json::from_str(old).expect("legacy record");
        assert_eq!(
            record.host.to_string(),
            "12345678-1234-4123-8123-123456789abc"
        );
        assert!(record.pending_client.is_none());
        assert_eq!(serde_json::to_string(&record).expect("saved record"), old);
    }

    #[tokio::test]
    async fn registration_survives_a_failed_exchange_without_changing_the_active_account() {
        use super::super::{Grant, authorization::Identity};
        const ISSUED: &str = "oaiapp_new_registration";
        let (_temp, data, accounts) = fixture();
        let host = accounts.record().host;
        let grant = || Grant {
            client: ISSUED.into(),
            code: Zeroizing::new("one-time-code".into()),
            redirect: url::Url::parse("http://127.0.0.1:1455/auth/callback").expect("loopback"),
            verifier: Zeroizing::new("verifier".into()),
            nonce: Zeroizing::new("nonce".into()),
        };
        let accounts_ref = &accounts;
        let failed = accounts.complete_grant(grant(), |grant| async move {
            assert_eq!(grant.client, ISSUED);
            assert_eq!(
                accounts_ref.record().pending_client.as_deref(),
                Some(ISSUED)
            );
            Err(Error::Expired)
        });
        assert!(matches!(failed.await, Err(Error::Expired)));
        drop(accounts);
        let loaded = Accounts::load(data).expect("reload pending registration");
        assert_eq!(loaded.active().expect("existing account").id, ID);
        assert_eq!(loaded.saved().len(), 1);
        let (_, browser) = loaded
            .prepare(None, false)
            .await
            .expect("resume registration");
        let query = url::Url::parse(&browser).expect("authorization URL");
        let params: BTreeMap<_, _> = query.query_pairs().collect();
        assert_eq!(params["client_id"], ISSUED);
        assert_eq!(params["ext_agent_host_id"], host.urn().to_string());
        assert!(!params.contains_key("agent_name_hint"));
        let active = loaded
            .complete_grant(grant(), |_| async {
                Ok((
                    Identity {
                        sub: "new-subject".into(),
                        email: None,
                        name: None,
                    },
                    tokens("new-access", "new-refresh", now() + 3600),
                ))
            })
            .await
            .expect("complete the issued registration");
        assert_eq!(active.id, ISSUED);
        assert!(loaded.record().pending_client.is_none());
        assert!(!active.consent);
    }

    #[tokio::test]
    async fn rotation_is_serialized_persisted_and_denied_before_consent() {
        let (_temp, data, accounts) = fixture();
        let refreshes = AtomicUsize::new(0);
        let refresh = |old: Tokens| async {
            assert_eq!(
                old.refresh.expect("old refresh token").0.as_str(),
                "initial-refresh"
            );
            refreshes.fetch_add(1, Ordering::Relaxed);
            tokio::task::yield_now().await;
            Ok(tokens("rotated-access", "rotated-refresh", now() + 3600))
        };
        assert!(matches!(
            accounts.access_with(ID, true, refresh).await,
            Err(Error::NoGrant)
        ));
        assert!(
            Accounts::load(data.clone()).is_err(),
            "second process lease must fail"
        );
        accounts.consent(ID).expect("persist consent");
        let (one, two) = tokio::join!(
            accounts.access_with(ID, true, refresh),
            accounts.access_with(ID, true, refresh)
        );
        assert_eq!(one.expect("first token").0.as_str(), "rotated-access");
        assert_eq!(two.expect("second token").0.as_str(), "rotated-access");
        assert_eq!(refreshes.load(Ordering::Relaxed), 1);
        drop(accounts);
        let loaded = Accounts::load(data).expect("reload protected tokens");
        let record = loaded.record();
        let account = &record.accounts[ID];
        assert!(account.consent);
        assert_eq!(
            account
                .tokens
                .as_ref()
                .expect("tokens saved")
                .refresh
                .as_ref()
                .expect("refresh saved")
                .0
                .as_str(),
            "rotated-refresh"
        );
    }

    #[tokio::test]
    async fn sign_out_revokes_then_clears_sealed_tokens_even_if_revocation_fails() {
        let (_temp, data, accounts) = fixture();
        let host = accounts.record().host;
        let result = accounts
            .sign_out_with(ID, |refresh| async move {
                assert_eq!(refresh.0.as_str(), "initial-refresh");
                Err(Error::Network)
            })
            .await;
        assert_eq!(result, Err(Error::NotRevoked));
        assert!(!accounts.active().expect("registration remains").signed_in);
        drop(accounts);
        let loaded = Accounts::load(data).expect("reload signed-out account");
        assert_eq!(loaded.record().host, host);
        assert!(!loaded.active().expect("registration remains").signed_in);
        assert!(matches!(
            loaded
                .access_with(ID, true, |_| async { panic!("must not refresh") })
                .await,
            Err(Error::Expired)
        ));
    }
}
