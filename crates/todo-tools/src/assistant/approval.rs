//! Deterministic review and approval, shared between client and writer. Neither owns a transport.

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use todo_core::actor::ASSISTANT_AGENT_ID;
use todo_core::command::{Command, NewTask, Placement, TaskPatch};
use todo_core::decide::{MAX_TITLE, decide_seeded};
use todo_core::event::{Change, EventPayload, FieldChange};
use todo_core::ids::{EntityId, ListId, TaskId};
use todo_core::model::{Entity, Priority, Task, WorkflowStatus};
use todo_core::time::{Date, Timestamp};
use todo_core::{AccessLevel, Actor, DeviceId, Event, Grants, State, apply};
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::{UnicodeNormalization, is_nfc};
use uuid::Uuid;

pub const PROPOSAL_SCHEMA: u16 = 1;
pub const MAX_COMMANDS: usize = 12;
pub const MAX_ALLOWED_LISTS: usize = 64;
const MAX_EVENTS: usize = 32;
const SEED_SCHEME: &str = "assistant-batch/v1";

macro_rules! opaque_debug {
    ($($ty:ident),+ $(,)?) => {$(
        impl std::fmt::Debug for $ty {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.debug_struct(stringify!($ty)).finish_non_exhaustive()
            }
        }
    )+};
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Revision {
    pub epoch: Uuid,
    pub cursor: u64,
}

/// Server-recorded provenance, not part of the request or the effect digest. Owner-only metadata.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ApprovalOrigin {
    Device { device: DeviceId },
    Session { session_hash: String },
}

/// Frozen wire body. The command ID doubles as the proposal ID and retry key.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRequest {
    pub command_id: Uuid,
    pub batch: ApprovedBatch,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedBatch {
    pub schema: u16,
    pub revision: Revision,
    pub scope_revision: u64,
    pub allowed_lists: BTreeSet<ListId>,
    pub at: Timestamp,
    pub commands: Vec<ApprovalCommand>,
    pub digest: [u8; 32],
}

// A strict subset rather than arbitrary core Commands with ignored fields. Unsupported fields,
// actor and access metadata are not expressible; even null fields outside the subset are refused.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApprovalCommand {
    CreateTask {
        id: TaskId,
        list: ListId,
        title: String,
        #[serde(default)]
        due: Option<Date>,
        #[serde(default)]
        priority: Priority,
    },
    UpdateTask {
        id: TaskId,
        patch: ApprovalPatch,
    },
    CompleteTask {
        id: TaskId,
    },
    UncompleteTask {
        id: TaskId,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(
        default,
        deserialize_with = "present_date",
        skip_serializing_if = "Option::is_none"
    )]
    pub due: Option<Option<Date>>,
    #[serde(default)]
    pub priority: Option<Priority>,
}

fn present_date<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<Date>>, D::Error> {
    Option::deserialize(d).map(Some)
}

impl From<&ApprovalCommand> for Command {
    fn from(command: &ApprovalCommand) -> Self {
        match command {
            ApprovalCommand::CreateTask {
                id,
                list,
                title,
                due,
                priority,
            } => {
                let mut task = NewTask::new(*id, *list, title.clone());
                task.due = *due;
                task.priority = *priority;
                Self::CreateTask(Box::new(task))
            }
            ApprovalCommand::UpdateTask { id, patch } => Self::UpdateTask {
                id: *id,
                patch: TaskPatch {
                    title: patch.title.clone(),
                    due: patch.due,
                    priority: patch.priority,
                    ..TaskPatch::default()
                },
            },
            ApprovalCommand::CompleteTask { id } => Self::CompleteTask { id: *id },
            ApprovalCommand::UncompleteTask { id } => Self::UncompleteTask { id: *id },
        }
    }
}

impl TryFrom<&Command> for ApprovalCommand {
    type Error = BatchError;

    fn try_from(command: &Command) -> Result<Self, BatchError> {
        Ok(match command {
            Command::CreateTask(task)
                if task.notes.is_empty()
                    && task.start_date.is_none()
                    && task.status == WorkflowStatus::Todo
                    && task.my_day.is_none()
                    && task.recurrence.is_none()
                    && task.tags.is_empty()
                    && task.placement == Placement::Last =>
            {
                Self::CreateTask {
                    id: task.id,
                    list: task.list,
                    title: task.title.clone(),
                    due: task.due,
                    priority: task.priority,
                }
            }
            Command::UpdateTask { id, patch }
                if patch.notes.is_none()
                    && patch.start_date.is_none()
                    && patch.status.is_none()
                    && patch.recurrence.is_none() =>
            {
                Self::UpdateTask {
                    id: *id,
                    patch: ApprovalPatch {
                        title: patch.title.clone(),
                        due: patch.due,
                        priority: patch.priority,
                    },
                }
            }
            Command::CompleteTask { id } => Self::CompleteTask { id: *id },
            Command::UncompleteTask { id } => Self::UncompleteTask { id: *id },
            _ => return Err(BatchError::UnsupportedTool),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BatchError {
    #[error("approval exceeds its size limit")]
    Limit,
    #[error("approval uses an unsupported command or field")]
    UnsupportedTool,
    #[error("the requested task or list is not available")]
    NotFound,
    #[error("the proposed change could not be prepared")]
    InvalidDraft,
    #[error("the approved effects do not match")]
    DigestMismatch,
}

opaque_debug!(
    ApprovalOrigin,
    ApprovalRequest,
    ApprovedBatch,
    ApprovalCommand,
    ApprovalPatch,
    ListRef,
    TaskRef,
    Edit,
    Effect,
    ProposalItem,
    ProposalPreview,
    PendingProposal,
    Batch
);

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ListRef {
    pub id: ListId,
    pub name: String,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct TaskRef {
    pub id: TaskId,
    pub title: String,
    pub list: ListRef,
    pub due: Option<Date>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProposalKind {
    Create,
    Edit,
    Complete,
    Destructive,
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum Edit {
    Title(Change<String>),
    Due(Change<Option<Date>>),
    StartDate(Change<Option<Date>>),
    Priority(Change<Priority>),
    Completed(Change<bool>),
}

#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Effect {
    Created {
        task: TaskRef,
        priority: Priority,
        status: WorkflowStatus,
        /// Semantic insertion; no rank bytes are needed for review or for the digest.
        placement: Placement,
    },
    Edited {
        task: TaskRef,
        changes: Vec<Edit>,
    },
    /// Presentation for a tombstone, not an advertised tool or a supported command form in v1.
    Deleted {
        task: TaskRef,
    },
}

impl Effect {
    #[must_use]
    pub fn kind(&self) -> ProposalKind {
        match self {
            Self::Created { .. } => ProposalKind::Create,
            Self::Edited { changes, .. } if matches!(changes.as_slice(), [Edit::Completed(_)]) => {
                ProposalKind::Complete
            }
            Self::Edited { .. } => ProposalKind::Edit,
            Self::Deleted { .. } => ProposalKind::Destructive,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct ProposalItem {
    /// Position in the original draft, retained across subset reviews.
    pub index: usize,
    /// Ordered effects, not a net diff: an edit followed by its reversal is still reviewable.
    pub effects: Vec<Effect>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ProposalPreview {
    pub id: Uuid,
    pub digest: [u8; 32],
    pub items: Vec<ProposalItem>,
}

impl ProposalPreview {
    #[must_use]
    pub fn kind(&self) -> Option<ProposalKind> {
        self.items
            .iter()
            .flat_map(|item| &item.effects)
            .map(Effect::kind)
            .max()
    }
}

/// Frozen in-memory review. Only the approved wire batch is serializable for the outbox.
#[derive(Clone)]
pub struct PendingProposal {
    preview: ProposalPreview,
    revision: Revision,
    scope_revision: u64,
    allowed: BTreeSet<ListId>,
    at: Timestamp,
    commands: Vec<Command>,
}

impl PendingProposal {
    #[must_use]
    pub const fn schema(&self) -> u16 {
        PROPOSAL_SCHEMA
    }
    #[must_use]
    pub fn preview(&self) -> &ProposalPreview {
        &self.preview
    }
    #[must_use]
    pub fn revision(&self) -> Revision {
        self.revision
    }
    #[must_use]
    pub fn scope_revision(&self) -> u64 {
        self.scope_revision
    }
    #[must_use]
    pub fn allowed_lists(&self) -> &BTreeSet<ListId> {
        &self.allowed
    }
    #[must_use]
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }
    #[must_use]
    pub fn at(&self) -> Timestamp {
        self.at
    }

    pub fn approval(&self) -> Result<ApprovedBatch, BatchError> {
        Ok(ApprovedBatch {
            schema: PROPOSAL_SCHEMA,
            revision: self.revision,
            scope_revision: self.scope_revision,
            allowed_lists: self.allowed.clone(),
            at: self.at,
            commands: self
                .commands
                .iter()
                .map(ApprovalCommand::try_from)
                .collect::<Result<_, _>>()?,
            digest: self.preview.digest,
        })
    }
}

pub struct Batch {
    id: Uuid,
    revision: Revision,
    scope_revision: u64,
    allowed: BTreeSet<ListId>,
    actor: Actor,
    at: Timestamp,
    scratch: State,
    commands: Vec<Command>,
    items: Vec<ProposalItem>,
    events: Vec<Event>,
}

impl Batch {
    pub fn new(
        id: Uuid,
        revision: Revision,
        scope_revision: u64,
        state: &State,
        allowed: BTreeSet<ListId>,
        at: Timestamp,
    ) -> Result<Self, BatchError> {
        if allowed.len() > MAX_ALLOWED_LISTS {
            return Err(BatchError::Limit);
        }
        if allowed.iter().any(|id| !state.is_live((*id).into())) {
            return Err(BatchError::NotFound);
        }
        let actor = Actor::agent(ASSISTANT_AGENT_ID).map_err(|_| BatchError::InvalidDraft)?;
        let mut scratch = state.clone();
        let agent = actor.agent_id().ok_or(BatchError::InvalidDraft)?;
        scratch.grants.insert(
            agent.clone(),
            Grants {
                lists: allowed
                    .iter()
                    .copied()
                    .map(|id| (id, AccessLevel::Plan))
                    .collect(),
                ..Grants::default()
            },
        );
        Ok(Self {
            id,
            revision,
            scope_revision,
            allowed,
            actor,
            at,
            scratch,
            commands: Vec::new(),
            items: Vec::new(),
            events: Vec::new(),
        })
    }

    fn seed(&self) -> String {
        // Only retained commands get an ordinal. UI indices are not part of the wire contract.
        format!("{SEED_SCHEME}:{}:{}", self.id, self.commands.len())
    }

    /// Callers must discard the entire simulation if this returns an error.
    pub fn push(&mut self, command: Command, index: usize) -> Result<bool, BatchError> {
        let mut approved = ApprovalCommand::try_from(&command)?;
        if let Some(title) = approved.title_mut() {
            // Validate raw input before normalization, including characters at the edges.
            title_characters(title)?;
            if !is_nfc(title) {
                *title = title.nfc().collect();
            }
        }
        approved.check_shape(&self.allowed)?;
        self.push_checked(&approved, index)
    }

    fn push_checked(
        &mut self,
        approved: &ApprovalCommand,
        index: usize,
    ) -> Result<bool, BatchError> {
        if self.commands.len() >= MAX_COMMANDS {
            return Err(BatchError::Limit);
        }
        match approved {
            ApprovalCommand::CreateTask { .. } => {}
            ApprovalCommand::UpdateTask { id, .. } => {
                self.task(*id)?;
            }
            ApprovalCommand::CompleteTask { id } | ApprovalCommand::UncompleteTask { id } => {
                if self.task(*id)?.recurrence.is_some() {
                    return Err(BatchError::UnsupportedTool);
                }
            }
        }
        let command = Command::from(approved);
        let events = decide_seeded(&self.scratch, &self.actor, &command, self.at, &self.seed())
            .map_err(|_| BatchError::InvalidDraft)?;
        if events.is_empty() {
            return Ok(false);
        }
        if self.events.len() + events.len() > MAX_EVENTS {
            return Err(BatchError::Limit);
        }
        let mut effects = Vec::new();
        for event in &events {
            if let Some(effect) = self.effect(&event.payload)? {
                if effect.kind() == ProposalKind::Destructive {
                    return Err(BatchError::InvalidDraft);
                }
                effects.push(effect);
            }
            apply(&mut self.scratch, event).map_err(|_| BatchError::InvalidDraft)?;
        }
        if effects.is_empty() {
            return Err(BatchError::InvalidDraft);
        }
        self.events.extend(events);
        self.commands.push(command);
        self.items.push(ProposalItem { index, effects });
        Ok(true)
    }

    fn list_ref(&self, id: ListId) -> Result<ListRef, BatchError> {
        self.scratch
            .lists
            .get(&id)
            .filter(|_| self.allowed.contains(&id) && self.scratch.is_live(id.into()))
            .map(|list| ListRef {
                id,
                name: list.name.clone(),
            })
            .ok_or(BatchError::NotFound)
    }

    pub fn task(&self, id: TaskId) -> Result<&Task, BatchError> {
        self.scratch
            .tasks
            .get(&id)
            .filter(|task| self.allowed.contains(&task.list) && self.scratch.is_live(id.into()))
            .ok_or(BatchError::NotFound)
    }

    fn task_ref(&self, task: &Task) -> Result<TaskRef, BatchError> {
        Ok(TaskRef {
            id: task.id,
            title: task.title.clone(),
            list: self.list_ref(task.list)?,
            due: task.due,
        })
    }

    fn effect(&self, payload: &EventPayload) -> Result<Option<Effect>, BatchError> {
        Ok(Some(match payload {
            EventPayload::Created {
                entity: Entity::Task(task),
            } if task.notes.is_empty()
                && task.start_date.is_none()
                && task.my_day.is_none()
                && task.completed_at.is_none()
                && task.recurrence.is_none()
                && task.tags.is_empty()
                && task.deleted.is_none()
                && task.merged_into.is_none() =>
            {
                Effect::Created {
                    task: self.task_ref(task)?,
                    priority: task.priority,
                    status: task.status,
                    placement: Placement::Last,
                }
            }
            EventPayload::Updated {
                target: EntityId::Task(id),
                changes,
            } => {
                let task = self.task_ref(self.task(*id)?)?;
                let mut edits = Vec::new();
                for change in changes {
                    edits.push(match change {
                        FieldChange::Title(c) => Edit::Title(c.clone()),
                        FieldChange::Due(c) => Edit::Due(c.clone()),
                        FieldChange::StartDate(c) => Edit::StartDate(c.clone()),
                        FieldChange::Priority(c) => Edit::Priority(c.clone()),
                        FieldChange::CompletedAt(c) => {
                            Edit::Completed(Change::new(c.old.is_some(), c.new.is_some()))
                        }
                        FieldChange::Touch(_) => continue,
                        _ => return Err(BatchError::InvalidDraft), // No omitted semantic changes.
                    });
                }
                if edits.is_empty() {
                    return Ok(None);
                }
                Effect::Edited {
                    task,
                    changes: edits,
                }
            }
            EventPayload::Deleted {
                target: EntityId::Task(id),
                ..
            } => Effect::Deleted {
                task: self.task_ref(self.task(*id)?)?,
            },
            _ => return Err(BatchError::InvalidDraft),
        }))
    }

    fn digest(&self) -> Result<[u8; 32], BatchError> {
        let effects: Vec<_> = self.items.iter().map(|item| &item.effects).collect();
        let bytes = serde_json::to_vec(&(
            PROPOSAL_SCHEMA,
            SEED_SCHEME,
            self.id,
            self.revision,
            self.scope_revision,
            &self.allowed,
            self.at,
            &self.commands,
            effects,
        ))
        .map_err(|_| BatchError::InvalidDraft)?;
        Ok(Sha256::digest(bytes).into())
    }

    pub fn freeze(self) -> Result<Option<PendingProposal>, BatchError> {
        if self.items.is_empty() {
            return Ok(None);
        }
        let digest = self.digest()?;
        Ok(Some(PendingProposal {
            preview: ProposalPreview {
                id: self.id,
                digest,
                items: self.items,
            },
            revision: self.revision,
            scope_revision: self.scope_revision,
            allowed: self.allowed,
            at: self.at,
            commands: self.commands,
        }))
    }
}

fn title_characters(text: &str) -> Result<(), BatchError> {
    // Reject invisible controls/formats (including ZWJ in emoji sequences), private-use and
    // unassigned code points. Scope these rules to assistant-authored titles, not old task data.
    if text.len() > 8 * 1024 {
        return Err(BatchError::Limit);
    }
    if text.chars().any(|c| {
        matches!(
            get_general_category(c),
            GeneralCategory::Control
                | GeneralCategory::Format
                | GeneralCategory::PrivateUse
                | GeneralCategory::Unassigned
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
    }) {
        return Err(BatchError::InvalidDraft);
    }
    Ok(())
}

impl ApprovalCommand {
    fn title_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::CreateTask { title, .. } => Some(title),
            Self::UpdateTask { patch, .. } => patch.title.as_mut(),
            _ => None,
        }
    }

    fn check_shape(&self, allowed: &BTreeSet<ListId>) -> Result<(), BatchError> {
        let title = match self {
            Self::CreateTask { list, title, .. } => {
                if !allowed.contains(list) {
                    return Err(BatchError::NotFound);
                }
                Some(title)
            }
            Self::UpdateTask { patch, .. } => patch.title.as_ref(),
            _ => None,
        };
        if let Some(title) = title {
            title_characters(title)?;
            let trimmed = title.trim();
            if trimmed.is_empty() || trimmed.chars().count() > MAX_TITLE || !is_nfc(title) {
                return Err(BatchError::InvalidDraft);
            }
        }
        Ok(())
    }
}

/// An approval that passed all checks independent of the current event head.
#[derive(Debug)]
pub struct CheckedApproval<'a>(&'a ApprovalRequest);

impl ApprovalRequest {
    /// Bound and validate the request before contending for the writer. Live lists and targets
    /// are checked later, against the exact head. The wire accepts only already normalized titles.
    pub fn preflight(&self) -> Result<CheckedApproval<'_>, BatchError> {
        let payload = &self.batch;
        if payload.schema != PROPOSAL_SCHEMA
            || payload.commands.is_empty()
            || payload.allowed_lists.is_empty()
        {
            return Err(BatchError::InvalidDraft);
        }
        if payload.commands.len() > MAX_COMMANDS || payload.allowed_lists.len() > MAX_ALLOWED_LISTS
        {
            return Err(BatchError::Limit);
        }
        for command in &payload.commands {
            command.check_shape(&payload.allowed_lists)?;
        }
        Ok(CheckedApproval(self))
    }
}

impl CheckedApproval<'_> {
    /// Re-decide against the exact head checked by the writer and compare against the review.
    /// The caller commits events onto the original state, never to the scratch grants.
    pub fn redecide(self, base: &State) -> Result<Vec<Event>, BatchError> {
        let payload = &self.0.batch;
        let mut batch = Batch::new(
            self.0.command_id,
            payload.revision,
            payload.scope_revision,
            base,
            payload.allowed_lists.clone(),
            payload.at,
        )?;
        for (index, command) in payload.commands.iter().enumerate() {
            if !batch.push_checked(command, index)? {
                return Err(BatchError::InvalidDraft);
            }
        }
        if batch.digest()? != payload.digest {
            return Err(BatchError::DigestMismatch);
        }
        let group = batch.events.first().ok_or(BatchError::InvalidDraft)?.id;
        for event in &mut batch.events {
            event.group = group;
        }
        Ok(batch.events)
    }
}
