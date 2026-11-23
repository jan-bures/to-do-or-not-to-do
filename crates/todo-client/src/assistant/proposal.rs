//! Simulate the restricted tools against a scratch state. Nothing here has access to the outbox.

#[cfg(test)]
mod tests;

use super::{AssistantError, Revision};
use crate::CommandId;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use todo_core::command::{Command, Placement};
use todo_core::decide::decide_seeded;
use todo_core::event::{Change, EventPayload, FieldChange};
use todo_core::ids::{EntityId, ListId, TaskId};
use todo_core::model::{Entity, Priority, Task, WorkflowStatus};
use todo_core::time::{Date, Timestamp};
use todo_core::{AccessLevel, Actor, Grants, State, apply};

pub const PROPOSAL_SCHEMA: u16 = 1;
pub(super) const MAX_COMMANDS: usize = 12;
const MAX_EVENTS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ListRef {
    pub id: ListId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum Edit {
    Title(Change<String>),
    Due(Change<Option<Date>>),
    StartDate(Change<Option<Date>>),
    Priority(Change<Priority>),
    Completed(Change<bool>),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProposalItem {
    /// Position in the original draft, retained across subset reviews.
    pub index: usize,
    /// Ordered effects, not a net diff: e.g. an edit followed by its reversal is still reviewable.
    pub effects: Vec<Effect>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalPreview {
    pub id: CommandId,
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

/// The frozen, in-memory review record. Intentionally not serializable as an outbox operation.
#[derive(Clone)]
pub struct PendingProposal {
    preview: ProposalPreview,
    revision: Revision,
    scope_revision: u64,
    allowed: BTreeSet<ListId>,
    at: Timestamp,
    commands: Vec<Command>,
}

opaque_debug!(PendingProposal);

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
    pub(super) fn at(&self) -> Timestamp {
        self.at
    }
}

pub(super) struct Batch {
    id: CommandId,
    revision: Revision,
    scope_revision: u64,
    allowed: BTreeSet<ListId>,
    actor: Actor,
    at: Timestamp,
    scratch: State,
    commands: Vec<Command>,
    items: Vec<ProposalItem>,
    event_count: usize,
}

impl Batch {
    pub(super) fn new(
        id: CommandId,
        revision: Revision,
        scope_revision: u64,
        state: &State,
        allowed: BTreeSet<ListId>,
        at: Timestamp,
    ) -> Result<Self, AssistantError> {
        let actor = Actor::agent("assistant").map_err(|_| AssistantError::InvalidDraft)?;
        let mut scratch = state.clone();
        let agent = actor.agent_id().ok_or(AssistantError::InvalidDraft)?;
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
            event_count: 0,
        })
    }

    pub(super) fn push(&mut self, command: Command, index: usize) -> Result<bool, AssistantError> {
        if self.commands.len() >= MAX_COMMANDS {
            return Err(AssistantError::Limit);
        }
        self.validate(&command)?;
        // The outer index is stable for this proposal, even if some steps were omitted as no-ops.
        let seed = format!("{}:{index}", self.id);
        let events = decide_seeded(&self.scratch, &self.actor, &command, self.at, &seed)
            .map_err(|_| AssistantError::InvalidDraft)?;
        if self.event_count + events.len() > MAX_EVENTS {
            return Err(AssistantError::Limit);
        }
        let mut effects = Vec::new();
        for event in events {
            if let Some(effect) = self.effect(&event.payload)? {
                // This surface can display a tombstone, but no v1 tool may cause one.
                if effect.kind() == ProposalKind::Destructive {
                    return Err(AssistantError::InvalidDraft);
                }
                effects.push(effect);
            }
            apply(&mut self.scratch, &event).map_err(|_| AssistantError::InvalidDraft)?;
            self.event_count += 1;
        }
        if effects.is_empty() {
            return Ok(false);
        }
        self.commands.push(command);
        self.items.push(ProposalItem { index, effects });
        Ok(true)
    }

    // Independent of the model-facing decoder. Keep the command envelope smaller than the grants.
    fn validate(&self, command: &Command) -> Result<(), AssistantError> {
        match command {
            Command::CreateTask(task)
                if task.notes.is_empty()
                    && task.start_date.is_none()
                    && task.status == WorkflowStatus::Todo
                    && task.my_day.is_none()
                    && task.recurrence.is_none()
                    && task.tags.is_empty()
                    && task.placement == Placement::Last =>
            {
                self.list_ref(task.list)?;
            }
            Command::UpdateTask { id, patch }
                if patch.notes.is_none()
                    && patch.start_date.is_none()
                    && patch.status.is_none()
                    && patch.recurrence.is_none() =>
            {
                self.task(*id)?;
            }
            Command::CompleteTask { id } | Command::UncompleteTask { id } => {
                if self.task(*id)?.recurrence.is_some() {
                    return Err(AssistantError::UnsupportedTool);
                }
            }
            _ => return Err(AssistantError::UnsupportedTool),
        }
        Ok(())
    }

    fn list_ref(&self, id: ListId) -> Result<ListRef, AssistantError> {
        self.scratch
            .lists
            .get(&id)
            .filter(|_| self.allowed.contains(&id) && self.scratch.is_live(id.into()))
            .map(|list| ListRef {
                id,
                name: list.name.clone(),
            })
            .ok_or(AssistantError::NotFound)
    }

    pub(super) fn task(&self, id: TaskId) -> Result<&Task, AssistantError> {
        self.scratch
            .tasks
            .get(&id)
            .filter(|task| self.allowed.contains(&task.list) && self.scratch.is_live(id.into()))
            .ok_or(AssistantError::NotFound)
    }

    fn task_ref(&self, task: &Task) -> Result<TaskRef, AssistantError> {
        Ok(TaskRef {
            id: task.id,
            title: task.title.clone(),
            list: self.list_ref(task.list)?,
            due: task.due,
        })
    }

    fn effect(&self, payload: &EventPayload) -> Result<Option<Effect>, AssistantError> {
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
                        // Do not silently omit new semantic effects (including a move/reorder).
                        _ => return Err(AssistantError::InvalidDraft),
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
            _ => return Err(AssistantError::InvalidDraft),
        }))
    }

    pub(super) fn freeze(self) -> Result<Option<PendingProposal>, AssistantError> {
        if self.items.is_empty() {
            return Ok(None);
        }
        let digest = Sha256::digest(
            serde_json::to_vec(&(
                PROPOSAL_SCHEMA,
                self.id,
                self.revision,
                self.scope_revision,
                &self.allowed,
                &self.commands,
                &self.items,
            ))
            .map_err(|_| AssistantError::InvalidDraft)?,
        )
        .into();
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
