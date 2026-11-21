//! Shared helpers for the integration tests.
#![allow(dead_code, clippy::unwrap_used)]

use todo_core::access::{AccessLevel, Grants};
use todo_core::command::{Command, NewTask, Placement};
use todo_core::decide::decide;
use todo_core::error::{DomainError, UndoError};
use todo_core::event::Event;
use todo_core::ids::*;
use todo_core::time::Timestamp;
use todo_core::undo::{UndoOptions, undo_batch, undo_with};
use todo_core::{Actor, AgentId, State, apply};

/// Deterministic ID generator.
#[derive(Default)]
pub struct Gen(u64);

impl Gen {
    fn bytes(&mut self) -> (u64, [u8; 10]) {
        self.0 += 1;
        let mut b = [0u8; 10];
        b[..8].copy_from_slice(&self.0.to_be_bytes());
        (1_700_000_000_000 + self.0, b)
    }
    pub fn collection(&mut self) -> CollectionId {
        let (ms, b) = self.bytes();
        CollectionId::new_v7(ms, b)
    }
    pub fn list(&mut self) -> ListId {
        let (ms, b) = self.bytes();
        ListId::new_v7(ms, b)
    }
    pub fn task(&mut self) -> TaskId {
        let (ms, b) = self.bytes();
        TaskId::new_v7(ms, b)
    }
    pub fn step(&mut self) -> StepId {
        let (ms, b) = self.bytes();
        StepId::new_v7(ms, b)
    }
    pub fn tag(&mut self) -> TagId {
        let (ms, b) = self.bytes();
        TagId::new_v7(ms, b)
    }
    pub fn link(&mut self) -> LinkId {
        let (ms, b) = self.bytes();
        LinkId::new_v7(ms, b)
    }
    pub fn attachment(&mut self) -> AttachmentId {
        let (ms, b) = self.bytes();
        AttachmentId::new_v7(ms, b)
    }
    pub fn reminder(&mut self) -> ReminderId {
        let (ms, b) = self.bytes();
        ReminderId::new_v7(ms, b)
    }
}

/// A state plus its event log and a clock that advances one second per call.
pub struct World {
    pub state: State,
    pub log: Vec<Event>,
    pub ids: Gen,
    pub clock: i64,
    pub owner: Actor,
}

pub fn ts(secs: i64) -> Timestamp {
    Timestamp::from_second(1_700_000_000 + secs).unwrap()
}

impl World {
    pub fn new() -> Self {
        Self {
            state: State::new(),
            log: Vec::new(),
            ids: Gen::default(),
            clock: 0,
            owner: Actor::owner("pc").unwrap(),
        }
    }

    pub fn now(&mut self) -> Timestamp {
        self.clock += 1;
        ts(self.clock)
    }

    pub fn grant(&mut self, agent: &str, grants: Grants) {
        self.state
            .grants
            .insert(AgentId::new(agent).unwrap(), grants);
    }

    pub fn grant_default(&mut self, agent: &str, level: AccessLevel) {
        self.grant(agent, Grants::with_default(level));
    }

    /// Runs a command as `actor`: decide, apply, log. Returns the events.
    pub fn run_as(&mut self, actor: &Actor, cmd: Command) -> Result<Vec<Event>, DomainError> {
        let now = self.now();
        let events = decide(&self.state, actor, &cmd, now)?;
        for e in &events {
            apply(&mut self.state, e).unwrap();
        }
        self.log.extend(events.iter().cloned());
        Ok(events)
    }

    pub fn run(&mut self, cmd: Command) -> Vec<Event> {
        let owner = self.owner.clone();
        self.run_as(&owner, cmd).unwrap()
    }

    pub fn undo_as(
        &mut self,
        actor: &Actor,
        event: &Event,
        force: bool,
    ) -> Result<Vec<Event>, UndoError> {
        let now = self.now();
        let comp = undo_with(&self.state, event, actor, now, UndoOptions { force })?;
        for e in &comp {
            apply(&mut self.state, e).unwrap();
        }
        self.log.extend(comp.iter().cloned());
        Ok(comp)
    }

    pub fn undo(&mut self, event: &Event) -> Result<Vec<Event>, UndoError> {
        let owner = self.owner.clone();
        self.undo_as(&owner, event, false)
    }

    /// Undoes the events of one group (a command, or a compensation with its rekey) as a unit.
    pub fn undo_group_as(
        &mut self,
        actor: &Actor,
        events: &[Event],
        force: bool,
    ) -> Result<Vec<Event>, UndoError> {
        let now = self.now();
        let comp = undo_batch(&self.state, events, actor, now, UndoOptions { force })?;
        for e in &comp {
            apply(&mut self.state, e).unwrap();
        }
        self.log.extend(comp.iter().cloned());
        Ok(comp)
    }

    pub fn undo_group(&mut self, events: &[Event]) -> Result<Vec<Event>, UndoError> {
        let owner = self.owner.clone();
        self.undo_group_as(&owner, events, false)
    }

    // Convenience builders -------------------------------------------------

    pub fn collection(&mut self, name: &str) -> CollectionId {
        let id = self.ids.collection();
        self.run(Command::CreateCollection {
            id,
            name: name.into(),
            color: None,
            placement: Placement::Last,
        });
        id
    }

    pub fn list(&mut self, collection: Option<CollectionId>, name: &str) -> ListId {
        let id = self.ids.list();
        self.run(Command::CreateList {
            id,
            collection,
            name: name.into(),
            color: None,
            placement: Placement::Last,
        });
        id
    }

    pub fn task(&mut self, list: ListId, title: &str) -> TaskId {
        let id = self.ids.task();
        self.run(Command::CreateTask(Box::new(NewTask::new(id, list, title))));
        id
    }
}
