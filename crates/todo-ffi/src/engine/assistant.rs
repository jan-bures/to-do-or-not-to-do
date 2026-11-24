//! Android's assistant host. Model requests only reach the shared provider and controller;
//! only the separate review gesture has access to the engine's approval outbox.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

use serde::{Deserialize, Serialize};
use todo_client::{
    TrackedUndo,
    assistant::{
        AccessRules, Approval, ApprovalOutcome, AssistantError, Composer, Controller, Edit, Effect,
        ListAccess, ProposalKind, ProposalStatus, Selection, SendIntent, StepKind, StepStatus,
        ToolKind,
        local::{
            Destination, LocalProvider, Support, TailnetInterface, endpoint_info, is_tailnet_ip,
            valid_model,
        },
        provider::{DataClass, Provider, ProviderError},
    },
    date_format::DateLabels,
    native::{OsEntropy, TokioClock},
    notify::Notify,
};
use todo_core::{State, command::Placement, model::WorkflowStatus, time::Date};
use tokio::task::JoinHandle;
use uuid::Uuid;
use zeroize::Zeroizing;

use super::{ChangeListener, TodoEngine, collection_id, invalid, list_id, locked};
use crate::{
    FfiError,
    labels::{LocalNow, Now, one_line},
    runtime::runtime,
    view::priority_name,
};

type Chat = Controller<LocalProvider, TokioClock, OsEntropy>;

#[cfg(test)]
mod tests;

macro_rules! opaque_debug {
    ($($ty:ident),+ $(,)?) => {$(
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty)).finish_non_exhaustive()
            }
        }
    )+};
}

opaque_debug!(
    AndroidAssistant,
    AssistantAccessRow,
    AssistantServer,
    AssistantField,
    AssistantEffect,
    AssistantItem,
    AssistantProposal,
    AssistantLink,
    AssistantTurn,
    AssistantPanelState
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AssistantAccess {
    Hidden,
    PrivateOnly,
    AllModels,
}

impl From<ListAccess> for AssistantAccess {
    fn from(access: ListAccess) -> Self {
        match access {
            ListAccess::Hidden => Self::Hidden,
            ListAccess::PrivateOnly => Self::PrivateOnly,
            ListAccess::AllModels => Self::AllModels,
        }
    }
}
impl From<AssistantAccess> for ListAccess {
    fn from(access: AssistantAccess) -> Self {
        match access {
            AssistantAccess::Hidden => Self::Hidden,
            AssistantAccess::PrivateOnly => Self::PrivateOnly,
            AssistantAccess::AllModels => Self::AllModels,
        }
    }
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantAccessRow {
    pub id: String,
    pub name: String,
    pub collection: bool,
    pub nested: bool,
    pub inherited: bool,
    /// None means the collection's current lists are mixed.
    pub access: Option<AssistantAccess>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AssistantServer {
    pub address: String,
    pub model: String,
    /// Explicit owner confirmation for this exact HTTPS URL, not a DNS heuristic.
    pub private_server: bool,
}

/// Android supplies the Keystore and the live address of its active VPN. No task data crosses this boundary.
#[uniffi::export(with_foreign)]
pub trait AssistantPlatform: Send + Sync {
    fn seal_key(&self, plain: Vec<u8>) -> Option<Vec<u8>>;
    fn open_key(&self, sealed: Vec<u8>) -> Option<Vec<u8>>;
    /// Retire the alias before sealing a replacement or discarding the credential.
    fn destroy_key(&self) -> bool;
    fn vpn_addresses(&self) -> Vec<String>;
}

struct AndroidVpn(Arc<dyn AssistantPlatform>);

impl TailnetInterface for AndroidVpn {
    fn connected(&self) -> bool {
        self.0
            .vpn_addresses()
            .into_iter()
            .any(|address| address.parse().is_ok_and(is_tailnet_ip))
    }
}

#[derive(Clone, Debug, uniffi::Record)]
pub struct AssistantEndpoint {
    pub label: String,
    pub needs_attestation: bool,
    pub tailnet_http: bool,
}

#[uniffi::export]
pub fn assistant_endpoint(address: String, attested: bool) -> Result<AssistantEndpoint, FfiError> {
    endpoint(&address, attested)
}

fn endpoint(address: &str, attested: bool) -> Result<AssistantEndpoint, FfiError> {
    let info = endpoint_info(address, attested).map_err(|e| invalid(e.to_string()))?;
    Ok(AssistantEndpoint {
        label: info.destination.label().into(),
        needs_attestation: matches!(
            info.destination,
            Destination::NotPrivate | Destination::YourServer
        ),
        tailnet_http: info.tailnet_http,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, uniffi::Enum)]
pub enum AssistantConnection {
    #[default]
    Untested,
    Testing,
    Connected,
    Unreachable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AssistantProposalStatus {
    Pending,
    Stale,
    Rejected,
    Superseded,
    Saving,
    Queued,
    Applied,
    Failed,
    UndoRequested,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum AssistantEffectKind {
    Create,
    Edit,
    Complete,
    Delete,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantField {
    pub label: String,
    pub before: Option<String>,
    pub after: String,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantEffect {
    pub kind: AssistantEffectKind,
    pub title: String,
    pub list: String,
    pub list_hue: String,
    /// Existing task, never the provisional ID of a creation.
    pub target: Option<String>,
    pub fields: Vec<AssistantField>,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantItem {
    pub index: u32,
    pub selected: bool,
    pub effects: Vec<AssistantEffect>,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantProposal {
    pub id: String,
    /// This digest and ID belong to the displayed selection, not the latest unseen one.
    pub digest: Vec<u8>,
    pub status: AssistantProposalStatus,
    pub problem: Option<String>,
    pub items: Vec<AssistantItem>,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantLink {
    pub id: String,
    pub title: String,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantTurn {
    pub id: String,
    pub prompt: String,
    pub prose: String,
    pub steps: Vec<String>,
    pub step_summary: Option<String>,
    pub links: Vec<AssistantLink>,
    pub proposal: Option<AssistantProposal>,
    pub problem: Option<String>,
    pub stale_snapshot: bool,
}

#[derive(Clone, uniffi::Record)]
pub struct AssistantPanelState {
    pub generation: u64,
    pub server: Option<AssistantServer>,
    pub has_api_key: bool,
    pub connection: AssistantConnection,
    pub destination: Option<String>,
    pub support: Option<String>,
    pub access: Vec<AssistantAccessRow>,
    pub hidden: u32,
    pub streaming: bool,
    pub turns: Vec<AssistantTurn>,
    pub problem: Option<String>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    server: Option<AssistantServer>,
    // Keystore-encrypted envelope bound to the exact saved address. No plaintext on disk.
    sealed_key: Option<Vec<u8>>,
    support: Option<Support>,
    access: AccessRules,
}

struct Receipt {
    approval: Approval,
    undo_requested: bool,
}

#[derive(Default)]
struct Session {
    settings: Settings,
    provider: Option<LocalProvider>,
    connection: AssistantConnection,
    problem: Option<String>,
    check: Option<(Uuid, JoinHandle<()>)>,
    snapshot: Option<(Uuid, u64, bool)>,
    last_send: Option<u64>,
    receipts: HashMap<Uuid, Receipt>,
    foreground: bool,
    closed: bool,
}

#[derive(uniffi::Object)]
pub struct AndroidAssistant {
    engine: Arc<TodoEngine>,
    path: PathBuf,
    platform: Arc<dyn AssistantPlatform>,
    chat: Chat,
    session: Mutex<Session>,
    watch: Mutex<Option<JoinHandle<()>>>,
    changes: Notify,
}

#[uniffi::export]
impl AndroidAssistant {
    /// The app supplies a file inside this pairing's app-private, no-backup directory.
    #[uniffi::constructor]
    pub fn open(
        engine: Arc<TodoEngine>,
        settings_path: String,
        platform: Arc<dyn AssistantPlatform>,
    ) -> Arc<Self> {
        let path = PathBuf::from(settings_path);
        let mut session = Session::default();
        match read_settings(&path) {
            Ok(settings) => {
                if let Some(server) = &settings.server {
                    match open_key(&*platform, &server.address, settings.sealed_key.as_deref())
                        .and_then(|key| build(server, key.as_str(), settings.support, platform.clone())) {
                        Ok(provider) => session.provider = Some(provider),
                        Err(_) => session.problem = Some("Check the saved server and replace or remove its API key in AI settings.".into()),
                    }
                }
                session.settings = settings;
            }
            Err(_) => {
                session.problem =
                    Some("Couldn't read the saved AI settings. List access is off.".into())
            }
        }
        let this = Arc::new(Self {
            engine,
            path,
            platform,
            chat: Chat::new(TokioClock, OsEntropy),
            session: Mutex::new(session),
            watch: Mutex::default(),
            changes: Notify::default(),
        });
        this.install(&locked(&this.session));
        this
    }

    /// Observe both streaming changes and the confirmed replica. Never invoke Kotlin under a lock.
    pub fn listen(self: Arc<Self>, listener: Arc<dyn ChangeListener>) -> Result<(), FfiError> {
        let weak = Arc::downgrade(&self);
        let client = self.engine.client.clone();
        let mut changes = client.changes();
        let mut seen = self.chat.view_version();
        let mut version = self.changes.generation();
        let task = runtime()?.spawn(async move {
            while let Some(assistant) = weak.upgrade() {
                let chat_changed = assistant.chat.changed(seen);
                let settings_changed = assistant.changes.changed(version);
                drop(assistant);
                tokio::select! {
                    () = changes.next() => {
                        let Some(assistant) = weak.upgrade() else { break };
                        assistant.refresh_locked(&mut locked(&assistant.session));
                    }
                    new = chat_changed => seen = new,
                    new = settings_changed => version = new,
                }
                listener.on_change();
            }
        });
        if let Some(previous) = locked(&self.watch).replace(task) {
            previous.abort();
        }
        Ok(())
    }

    pub fn resume(&self) {
        let mut session = locked(&self.session);
        self.refresh_locked(&mut session);
        session.foreground = !session.closed;
    }

    pub fn pause(&self) {
        let mut session = locked(&self.session);
        session.foreground = false;
        stop_check(&mut session);
        self.chat.cancel();
    }

    pub fn shutdown(&self) {
        self.close_locked(&mut locked(&self.session));
        if let Some(watch) = locked(&self.watch).take() {
            watch.abort();
        }
    }

    pub fn panel(&self, now: LocalNow) -> AssistantPanelState {
        let mut session = locked(&self.session);
        self.refresh_locked(&mut session);
        let dates = Now::new(now).dates();
        let view = self.chat.view();
        let state = self.chat.list_scope().0;
        session.receipts.retain(|id, _| {
            view.turns
                .iter()
                .any(|turn| turn.proposal.as_ref().is_some_and(|p| p.preview.id == *id))
        });
        let (access, hidden) = if session.closed {
            (vec![], 0)
        } else {
            self.engine
                .client
                .with_confirmed(|state| access_rows(state, &session.settings.access))
        };
        let streaming = matches!(view.composer, Composer::Streaming { .. });
        AssistantPanelState {
            generation: view.generation,
            server: session.settings.server.clone(),
            has_api_key: session.settings.sealed_key.is_some(),
            destination: session
                .provider
                .as_ref()
                .and(session.settings.server.as_ref())
                .and_then(|server| endpoint(&server.address, server.private_server).ok())
                .map(|info| info.label),
            connection: session.connection,
            support: session.settings.support.map(|s| s.label().into()),
            access,
            hidden,
            streaming,
            turns: view
                .turns
                .into_iter()
                .map(|turn| {
                    let active =
                        matches!(view.composer, Composer::Streaming { turn: id } if id == turn.id);
                    let proposal = turn.proposal.map(|card| {
                        let receipt = session.receipts.get(&card.preview.id);
                        let (status, problem) = self.receipt(card.status, receipt);
                        AssistantProposal {
                            id: card.preview.id.to_string(),
                            digest: card.preview.digest.to_vec(),
                            status,
                            problem,
                            items: card
                                .available
                                .iter()
                                .map(|original| {
                                    let selected = card
                                        .preview
                                        .items
                                        .iter()
                                        .find(|item| item.index == original.index);
                                    AssistantItem {
                                        index: u32::try_from(original.index).unwrap_or_default(),
                                        selected: selected.is_some(),
                                        effects: selected
                                            .unwrap_or(original)
                                            .effects
                                            .iter()
                                            .map(|e| effect(e, dates, state.as_deref()))
                                            .collect(),
                                    }
                                })
                                .collect(),
                        }
                    });
                    let step_summary = (!active
                        && turn
                            .steps
                            .iter()
                            .any(|s| matches!(s.kind, StepKind::Tool(_))))
                    .then(|| turn.summary.label());
                    AssistantTurn {
                        id: turn.id.to_string(),
                        prompt: turn.prompt,
                        prose: turn.prose,
                        steps: turn
                            .steps
                            .into_iter()
                            .map(|step| step_label(step.kind, step.status))
                            .collect(),
                        step_summary,
                        links: turn
                            .task_links
                            .into_iter()
                            .map(|t| AssistantLink {
                                id: t.id.to_string(),
                                title: t.title,
                            })
                            .collect(),
                        proposal,
                        problem: turn.error.map(|e| match e {
                            AssistantError::Provider(error) => provider_problem(error),
                            e => e.to_string(),
                        }),
                        stale_snapshot: turn.stale_snapshot,
                    }
                })
                .collect(),
            problem: session.problem.clone(),
        }
    }

    /// A missing replacement keeps the key only for the same address. An empty replacement removes it.
    pub fn save_server(
        &self,
        server: Option<AssistantServer>,
        replacement_key: Option<String>,
    ) -> Result<(), FfiError> {
        let replacement = replacement_key.map(Zeroizing::new);
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let server = server.map(|s| AssistantServer {
            address: s.address.trim().into(),
            model: s.model.trim().into(),
            private_server: s.private_server,
        });
        if server == session.settings.server && replacement.is_none() {
            return Ok(());
        }
        let same_address = session
            .settings
            .server
            .as_ref()
            .zip(server.as_ref())
            .is_some_and(|(old, new)| old.address == new.address);
        let kept_key = session
            .settings
            .sealed_key
            .clone()
            .filter(|_| same_address && replacement.is_none());
        let retire =
            replacement.is_some() || (!same_address && session.settings.sealed_key.is_some());
        let saved_key = if let Some(server) = &server {
            open_key(&*self.platform, &server.address, kept_key.as_deref())?
        } else {
            Zeroizing::default()
        };
        let key = replacement
            .as_ref()
            .map_or(saved_key.as_str(), |key| key.trim());
        // Validate before retiring the old seal. After retirement a write error must fail closed,
        // never keep sending with the previously decrypted key.
        let provider = server
            .as_ref()
            .map(|s| build(s, key, None, self.platform.clone()))
            .transpose()?;
        self.chat.cancel();
        stop_check(&mut session);
        let commit = || -> Result<Settings, FfiError> {
            if retire && !self.platform.destroy_key() {
                return Err(invalid("Couldn't retire the saved API key."));
            }
            let sealed_key = match (&server, &replacement) {
                (Some(server), Some(_)) => seal_key(&*self.platform, &server.address, key)?,
                _ => kept_key,
            };
            let settings = Settings {
                server,
                sealed_key,
                support: None,
                access: if session.settings.server.is_some() && !same_address {
                    AccessRules::default()
                } else {
                    session.settings.access.clone()
                },
            };
            save_settings(&self.path, &settings)?;
            Ok(settings)
        };
        let settings = match commit() {
            Ok(settings) => settings,
            Err(error) => {
                if retire {
                    // Also discard any new seal from an interrupted write.
                    self.platform.destroy_key();
                    session.provider = None;
                    session.connection = AssistantConnection::Untested;
                    session.problem =
                        Some("Check the saved server and reenter or remove its API key.".into());
                    self.install(&session);
                }
                return Err(error);
            }
        };
        session.settings = settings;
        session.provider = provider;
        session.connection = AssistantConnection::Untested;
        session.problem = None;
        self.install(&session);
        Ok(())
    }

    pub fn set_access(
        &self,
        id: String,
        collection: bool,
        access: AssistantAccess,
    ) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let mut settings = session.settings.clone();
        self.engine.client.with_confirmed(|state| {
            if collection {
                let id = collection_id(&id)?;
                if !state.is_live(id.into()) {
                    return Err(invalid("That collection is no longer available."));
                }
                settings.access.set_collection(state, id, access.into());
            } else {
                let id = list_id(&id)?;
                if !state.is_live(id.into()) {
                    return Err(invalid("That list is no longer available."));
                }
                settings.access.lists.insert(id, access.into());
            }
            Ok(())
        })?;
        if settings.access != session.settings.access {
            // Cancel before disk I/O; a permission change must not race an old request's next round.
            self.chat.set_scope(settings.access.clone());
            if let Err(error) = save_settings(&self.path, &settings) {
                self.chat.set_scope(session.settings.access.clone());
                return Err(error);
            }
            session.settings = settings;
        }
        Ok(())
    }

    pub fn test_server(self: Arc<Self>) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        if session.check.is_some() {
            return Ok(());
        }
        if matches!(self.chat.view().composer, Composer::Streaming { .. }) {
            return Err(invalid("Stop the reply before testing the connection."));
        }
        let (provider, server) = session
            .provider
            .clone()
            .zip(session.settings.server.clone())
            .ok_or_else(|| invalid("Save an AI server and model first."))?;
        let weak = Arc::downgrade(&self);
        session.connection = AssistantConnection::Testing;
        session.problem = None;
        let id = Uuid::new_v4();
        session.check = Some((
            id,
            runtime()?.spawn(async move {
                let result = provider.probe(&server.model).await;
                Self::finish_test(weak, id, server, result);
            }),
        ));
        Ok(())
    }

    pub fn send(
        self: Arc<Self>,
        generation: u64,
        sequence: u64,
        prompt: String,
        now: LocalNow,
    ) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        if session.check.is_some() {
            return Err(invalid("Wait for the connection test to finish."));
        }
        let rt = runtime()?;
        let reply = self
            .chat
            .start(SendIntent {
                generation,
                sequence,
                prompt,
                context: Now::new(now).eval(),
            })
            .map_err(|e| invalid(e.to_string()))?;
        session.last_send = Some(sequence);
        let weak = Arc::downgrade(&self);
        rt.spawn(async move {
            let connection = match reply.await {
                Ok(()) => AssistantConnection::Connected,
                Err(AssistantError::Provider(
                    ProviderError::Unavailable
                    | ProviderError::Timeout
                    | ProviderError::Tls
                    | ProviderError::TailnetDisconnected,
                )) => AssistantConnection::Unreachable,
                Err(_) => return,
            };
            if let Some(this) = weak.upgrade() {
                let mut session = locked(&this.session);
                if !session.closed
                    && session.check.is_none()
                    && session.foreground
                    && session.last_send == Some(sequence)
                    && this.chat.view().generation == generation
                {
                    session.connection = connection;
                    this.changes.notify();
                }
            }
        });
        Ok(())
    }

    pub fn stop(&self) {
        self.chat.cancel();
    }

    pub fn new_chat(&self) {
        let mut session = locked(&self.session);
        self.chat.new_chat();
        session.receipts.clear();
    }

    pub fn select_items(
        &self,
        id: String,
        digest: Vec<u8>,
        indices: Vec<u32>,
    ) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let (id, digest) = review_key(&id, digest)?;
        self.chat
            .select_items(
                id,
                digest,
                &indices.into_iter().map(|i| i as usize).collect(),
            )
            .map_err(|e| invalid(e.to_string()))
    }

    pub fn reject(&self, id: String, digest: Vec<u8>) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let (id, digest) = review_key(&id, digest)?;
        self.chat
            .discard(id, digest)
            .map_err(|e| invalid(e.to_string()))
    }

    pub fn approve(&self, id: String, digest: Vec<u8>) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let (id, digest) = review_key(&id, digest)?;
        let approval = self
            .chat
            .approve(&self.engine.client, id, digest)
            .map_err(|e| invalid(e.to_string()))?;
        session.receipts.insert(
            id,
            Receipt {
                approval,
                undo_requested: false,
            },
        );
        Ok(())
    }

    pub fn undo(&self, id: String) -> Result<(), FfiError> {
        let mut session = locked(&self.session);
        self.require_active(&mut session)?;
        let id = id.parse().map_err(|_| invalid("No such approval."))?;
        let receipt = session
            .receipts
            .get_mut(&id)
            .ok_or_else(|| invalid("No such approval."))?;
        if receipt.undo_requested
            || !matches!(
                self.engine.client.approval_outcome(&receipt.approval),
                ApprovalOutcome::Confirmed { .. }
            )
        {
            return Err(invalid("That change isn't available to undo."));
        }
        match self
            .engine
            .client
            .compensate_tracked(receipt.approval.undo_target())?
        {
            TrackedUndo::Queued(_) | TrackedUndo::Cancelled => {
                receipt.undo_requested = true;
                Ok(())
            }
            TrackedUndo::Unavailable => Err(invalid("That change isn't available to undo.")),
        }
    }
}

impl AndroidAssistant {
    fn close_locked(&self, session: &mut Session) {
        if session.closed {
            return;
        }
        stop_check(session);
        *session = Session {
            closed: true,
            ..Session::default()
        };
        self.chat.sign_out();
    }

    fn refresh_locked(&self, session: &mut Session) {
        if session.closed {
            return;
        }
        if self.engine.client.revocation().is_revoked() {
            self.close_locked(session);
            return;
        }
        let next = self.engine.client.assistant_snapshot();
        // update_snapshot publishes even when nothing changed. This method is also called by panel().
        // The watcher marks its seen version after processing a sync change, not after a view read.
        let key = next
            .as_ref()
            .map(|s| (s.revision.epoch, s.revision.cursor, s.stale));
        if session.snapshot != key {
            self.chat.update_snapshot(next);
            session.snapshot = key;
        }
    }

    fn require_active(&self, session: &mut Session) -> Result<(), FfiError> {
        self.refresh_locked(session);
        if session.closed || !session.foreground {
            return Err(invalid("The assistant is not active."));
        }
        Ok(())
    }

    fn install(&self, session: &Session) {
        self.chat.configure(
            session
                .provider
                .as_ref()
                .zip(session.settings.server.as_ref())
                .map(|(provider, server)| {
                    selection(Arc::new(provider.clone()), server, &session.settings.access)
                }),
        );
    }

    fn finish_test(
        weak: Weak<Self>,
        id: Uuid,
        server: AssistantServer,
        result: Result<Support, ProviderError>,
    ) {
        let Some(this) = weak.upgrade() else { return };
        let mut session = locked(&this.session);
        if this.require_active(&mut session).is_err()
            || !session
                .check
                .as_ref()
                .is_some_and(|(current, _)| *current == id)
        {
            return;
        }
        session.check.take();
        match result {
            Ok(support) => {
                let previous = session.settings.support.unwrap_or(Support::StreamingTools);
                session.settings.support = Some(support);
                if save_settings(&this.path, &session.settings).is_err() {
                    session.problem = Some("Couldn't save the connection test result.".into());
                }
                session.connection = AssistantConnection::Connected;
                if support != previous {
                    session.provider = session
                        .provider
                        .take()
                        .map(|p| p.with_support(server.model, support));
                    this.install(&session);
                }
            }
            Err(error) => {
                session.connection = AssistantConnection::Unreachable;
                session.problem = Some(provider_problem(error));
            }
        }
        this.changes.notify();
    }

    fn receipt(
        &self,
        status: ProposalStatus,
        receipt: Option<&Receipt>,
    ) -> (AssistantProposalStatus, Option<String>) {
        use AssistantProposalStatus as Status;
        let mut problem = None;
        let label = match status {
            ProposalStatus::Pending => Status::Pending,
            ProposalStatus::Stale => Status::Stale,
            ProposalStatus::Declined => Status::Rejected,
            ProposalStatus::Superseded => Status::Superseded,
            ProposalStatus::Submitted => match receipt {
                Some(r) if r.undo_requested => Status::UndoRequested,
                Some(r) => match self.engine.client.approval_outcome(&r.approval) {
                    ApprovalOutcome::Saving => Status::Saving,
                    ApprovalOutcome::Pending => Status::Queued,
                    ApprovalOutcome::Confirmed { .. } => Status::Applied,
                    ApprovalOutcome::Rejected(rejection) => {
                        problem =
                            Some(todo_client::failure::approval_message(&rejection.code).into());
                        Status::Failed
                    }
                    ApprovalOutcome::Unavailable => Status::Failed,
                },
                None => Status::Failed,
            },
        };
        (label, problem)
    }
}

impl Drop for AndroidAssistant {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn stop_check(session: &mut Session) {
    if let Some((_, check)) = session.check.take() {
        check.abort();
        session.connection = AssistantConnection::Untested;
    }
}

// Data classification is taken from the provider, never from a Kotlin enum or a per-list override.
fn selection<P: Provider>(
    provider: Arc<P>,
    server: &AssistantServer,
    access: &AccessRules,
) -> Selection<P> {
    Selection {
        provider,
        model: server.model.clone(),
        access: access.clone(),
        allow_billed: false,
        billed_list_scope: None,
    }
}

fn provider_problem(error: ProviderError) -> String {
    match error {
        ProviderError::TailnetDisconnected => {
            "Connect to Tailscale on this phone and try again.".into()
        }
        ProviderError::Unauthorized => {
            "The server rejected the API key. Check it in AI settings.".into()
        }
        error => error.to_string(),
    }
}

fn build(
    server: &AssistantServer,
    key: &str,
    support: Option<Support>,
    platform: Arc<dyn AssistantPlatform>,
) -> Result<LocalProvider, FfiError> {
    if !valid_model(&server.model) {
        return Err(invalid("Enter a valid model name."));
    }
    let provider = LocalProvider::configured(&server.address, key, server.private_server)
        .map_err(|e| invalid(e.to_string()))?;
    if provider.capabilities(&server.model).data_class != DataClass::Local {
        return Err(invalid(
            "Confirm that this HTTPS address is your own private server.",
        ));
    }
    Ok(provider
        .with_tailnet_interface(AndroidVpn(platform))
        .with_support(
            server.model.clone(),
            support.unwrap_or(Support::StreamingTools),
        ))
}

fn seal_key(
    platform: &dyn AssistantPlatform,
    address: &str,
    key: &str,
) -> Result<Option<Vec<u8>>, FfiError> {
    if key.is_empty() {
        return Ok(None);
    }
    let mut plain = Zeroizing::new(Vec::with_capacity(address.len() + 1 + key.len()));
    plain.extend_from_slice(address.as_bytes());
    plain.push(0);
    plain.extend_from_slice(key.as_bytes());
    platform
        .seal_key(std::mem::take(&mut *plain))
        .map(Some)
        .ok_or_else(|| invalid("Couldn't protect the API key on this phone."))
}

fn open_key(
    platform: &dyn AssistantPlatform,
    address: &str,
    sealed: Option<&[u8]>,
) -> Result<Zeroizing<String>, FfiError> {
    let Some(sealed) = sealed else {
        return Ok(Zeroizing::default());
    };
    let error = || invalid("Couldn't open the saved API key. Replace or remove it in AI settings.");
    let plain = Zeroizing::new(platform.open_key(sealed.to_vec()).ok_or_else(error)?);
    let split = plain.iter().position(|byte| *byte == 0).ok_or_else(error)?;
    let (saved, key) = (&plain[..split], &plain[split + 1..]);
    if saved != address.as_bytes() || key.is_empty() {
        return Err(error());
    }
    Ok(Zeroizing::new(
        std::str::from_utf8(key).map_err(|_| error())?.into(),
    ))
}

const SETTINGS_LIMIT: u64 = 1024 * 1024;

fn read_settings(path: &Path) -> io::Result<Settings> {
    match todo_fs::read_private_file(path, SETTINGS_LIMIT) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(error) => Err(error),
    }
}

fn save_settings(path: &Path, settings: &Settings) -> Result<(), FfiError> {
    let temp = path.with_extension("json.new");
    let save = || -> io::Result<()> {
        match fs::remove_file(&temp) {
            Ok(()) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
        let bytes = serde_json::to_vec(settings)?;
        if bytes.len() as u64 > SETTINGS_LIMIT {
            return Err(io::Error::other("settings too large"));
        }
        let mut file = todo_fs::create_private_file(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        todo_fs::replace_durably(&temp, path)
    };
    save().map_err(|_| invalid("Couldn't save the AI settings on this phone."))
}

fn review_key(id: &str, digest: Vec<u8>) -> Result<(Uuid, [u8; 32]), FfiError> {
    let error = || invalid("This proposal is no longer pending.");
    Ok((
        id.parse().map_err(|_| error())?,
        digest.try_into().map_err(|_| error())?,
    ))
}

fn access_rows(state: &State, rules: &AccessRules) -> (Vec<AssistantAccessRow>, u32) {
    let mut collections: Vec<_> = state
        .collections
        .values()
        .filter(|c| state.is_live(c.id.into()))
        .collect();
    collections.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
    let mut lists: Vec<_> = state
        .lists
        .values()
        .filter(|l| state.is_live(l.id.into()))
        .collect();
    lists.sort_by(|a, b| a.position.cmp(&b.position).then(a.id.cmp(&b.id)));
    let mut rows = Vec::new();
    let mut hidden = 0;
    let mut append = |list: &todo_core::model::List, nested: bool, rows: &mut Vec<_>| {
        let access = rules.list(state, list.id);
        if !access.allows(DataClass::Local) {
            hidden += 1;
        }
        rows.push(AssistantAccessRow {
            id: list.id.to_string(),
            name: one_line(&list.name),
            collection: false,
            nested,
            inherited: nested && !rules.lists.contains_key(&list.id),
            access: Some(access.into()),
        });
    };
    let collection_ids: BTreeSet<_> = collections.iter().map(|c| c.id).collect();
    for collection in collections {
        rows.push(AssistantAccessRow {
            id: collection.id.to_string(),
            name: one_line(&collection.name),
            collection: true,
            nested: false,
            inherited: false,
            access: rules.collection(state, collection.id).map(Into::into),
        });
        for list in lists.iter().filter(|l| l.collection == Some(collection.id)) {
            append(list, true, &mut rows);
        }
    }
    for list in lists
        .into_iter()
        .filter(|l| l.collection.is_none_or(|c| !collection_ids.contains(&c)))
    {
        append(list, false, &mut rows);
    }
    (rows, hidden)
}

fn effect(effect: &Effect, dates: DateLabels, state: Option<&State>) -> AssistantEffect {
    // Single cards and expanded batch rows get the same absolute civil date labels as desktop.
    let date = |d: Option<Date>| d.map_or_else(|| "No date".into(), |d| dates.weekday(d));
    let field = |label: &str, before, after| AssistantField {
        label: label.into(),
        before,
        after,
    };
    let done = |completed| {
        if completed {
            "Completed".into()
        } else {
            "Open".into()
        }
    };
    let (task, fields) = match effect {
        Effect::Created {
            task,
            priority,
            status,
            placement,
        } => (
            task,
            vec![
                field("Due", None, date(task.due)),
                field("Priority", None, priority_name(*priority).into()),
                field(
                    "Status",
                    None,
                    match status {
                        WorkflowStatus::Backlog => "Backlog",
                        WorkflowStatus::Todo => "To do",
                        WorkflowStatus::InProgress => "In progress",
                    }
                    .into(),
                ),
                field(
                    "Position",
                    None,
                    match placement {
                        Placement::First => "Top of the list",
                        Placement::Last => "End of the list",
                        _ => "Relative to another task",
                    }
                    .into(),
                ),
            ],
        ),
        Effect::Edited { task, changes } => (
            task,
            changes
                .iter()
                .map(|edit| match edit {
                    Edit::Title(c) => field("Title", Some(c.old.clone()), c.new.clone()),
                    Edit::Due(c) => field("Due", Some(date(c.old)), date(c.new)),
                    Edit::StartDate(c) => field("Starts", Some(date(c.old)), date(c.new)),
                    Edit::Priority(c) => field(
                        "Priority",
                        Some(priority_name(c.old).into()),
                        priority_name(c.new).into(),
                    ),
                    Edit::Completed(c) => field("Status", Some(done(c.old)), done(c.new)),
                })
                .collect(),
        ),
        Effect::Deleted { task } => (task, vec![]),
    };
    AssistantEffect {
        kind: match effect.kind() {
            ProposalKind::Create => AssistantEffectKind::Create,
            ProposalKind::Edit => AssistantEffectKind::Edit,
            ProposalKind::Complete => AssistantEffectKind::Complete,
            ProposalKind::Destructive => AssistantEffectKind::Delete,
        },
        title: task.title.clone(),
        list: task.list.name.clone(),
        list_hue: crate::lists::swatch(
            state
                .and_then(|s| s.lists.get(&task.list.id))
                .and_then(|l| l.color.as_deref()),
            task.list.id.as_uuid().as_bytes(),
        )
        .into(),
        target: (!matches!(effect, Effect::Created { .. })).then(|| task.id.to_string()),
        fields,
    }
}

fn step_label(kind: StepKind, status: StepStatus) -> String {
    let label = match kind {
        StepKind::Generate => "Waiting for the model",
        StepKind::Tool(ToolKind::Lists) => "Checked permitted lists",
        StepKind::Tool(ToolKind::Search) => "Searched visible tasks",
        StepKind::Tool(ToolKind::Task) => "Read task details",
        StepKind::Tool(ToolKind::Create) => "Prepared a new task",
        StepKind::Tool(ToolKind::Update) => "Prepared task edits",
        StepKind::Tool(ToolKind::Complete) => "Prepared a completion change",
    };
    match status {
        StepStatus::Running => format!("{label}…"),
        StepStatus::Done { elapsed, .. } => format!("{label} · {} ms", elapsed.as_millis()),
        StepStatus::Failed => label.into(),
    }
}
