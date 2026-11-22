//! Paged reads use the same wire types, cursor and event identity as web. Network I/O is off the UI thread.
use super::*;
use todo_client::{
    activity::{self, ActivityError, ActivityQuery},
    protocol::EventRef,
};
use todo_core::actor::AgentId;

#[derive(Debug, Clone, uniffi::Record)]
pub struct ActivityRow {
    pub id: String,
    pub seq: u64,
    pub summary: String,
    pub actor: String,
    pub at_ms: i64,
    pub relative: String,
    pub undone: bool,
    pub can_undo: bool,
    /// The whole batch is compensated by an undo of any one member.
    pub group: String,
}
#[derive(Debug, uniffi::Record)]
pub struct FeedPage {
    pub rows: Vec<ActivityRow>,
    pub before: Option<u64>,
    pub problem: Option<String>,
    pub skip: bool,
}
#[derive(Debug, uniffi::Record)]
pub struct FeedRevision {
    pub epoch: Option<String>,
    pub cursor: u64,
}

#[uniffi::export]
pub fn activity_agent_name(id: String) -> String {
    activity::agent_name(&id)
}
#[uniffi::export]
impl TodoEngine {
    pub fn activity_page(
        &self,
        agent: Option<String>,
        before: Option<u64>,
        now: LocalNow,
    ) -> Result<FeedPage, FfiError> {
        let agent = agent
            .map(AgentId::new)
            .transpose()
            .map_err(|_| invalid("No such agent."))?;
        let query = ActivityQuery {
            agent,
            before,
            limit: None,
        };
        let now = Now::new(now);
        match runtime()?.block_on(self.client.activity(&query)) {
            Ok(page) => Ok(FeedPage {
                before: page.next_before,
                problem: None,
                skip: false,
                rows: page
                    .items
                    .into_iter()
                    .filter_map(|i| {
                        let event = i.event_ref()?;
                        let e = &i.event.event;
                        if self.client.pending_undo(event.id).is_some() {
                            locked(&self.activity_groups).insert(e.group, event.id);
                        }
                        Some(ActivityRow {
                            id: event.id.to_string(),
                            seq: event.seq,
                            group: e.group.to_string(),
                            summary: i.what_happened().into(),
                            actor: activity::actor_name(&e.actor),
                            at_ms: e.at.as_millisecond(),
                            relative: activity::relative_time(e.at, now.at),
                            undone: i.undone,
                            can_undo: i.can_undo,
                        })
                    })
                    .collect(),
            }),
            Err(e) => Ok(FeedPage {
                rows: vec![],
                before: match e {
                    ActivityError::TooLarge { next_before } => Some(next_before),
                    _ => before,
                },
                problem: Some(e.to_string()),
                skip: matches!(e, ActivityError::TooLarge { .. }),
            }),
        }
    }
    pub fn pending_activity_groups(&self) -> Vec<String> {
        let mut groups = locked(&self.activity_groups);
        groups.retain(|_, id| self.client.pending_undo(*id).is_some());
        groups.keys().map(ToString::to_string).collect()
    }
    pub fn undo_activity_event(&self, seq: u64, id: String, group: String) -> Result<(), FfiError> {
        let id = id.parse().map_err(|_| invalid("Couldn't undo that."))?;
        let group = group.parse().map_err(|_| invalid("Couldn't undo that."))?;
        let mut groups = locked(&self.activity_groups);
        if groups
            .get(&group)
            .is_some_and(|id| self.client.pending_undo(*id).is_some())
            || self
                .client
                .with_confirmed(|s| s.journal.has_seen(id) && !s.journal.is_head(id))
        {
            return Ok(());
        }
        self.client
            .undo_activity(EventRef { seq, id })
            .map_err(|_| invalid("Couldn't undo that."))?;
        groups.insert(group, id);
        Ok(())
    }
    pub fn activity_revision(&self) -> FeedRevision {
        let (epoch, cursor) = self.client.activity_revision();
        FeedRevision {
            epoch: epoch.map(|e| e.to_string()),
            cursor,
        }
    }
    /// Confirmed rows that have left the head of their undo chain, even when the loaded page is older.
    pub fn activity_retired(&self, ids: Vec<String>) -> HashMap<String, String> {
        self.client.with_confirmed(|s| {
            ids.into_iter()
                .filter_map(|id| {
                    let event = id.parse().ok()?;
                    let j = &s.journal;
                    if !j.has_seen(event) || j.is_head(event) {
                        return None;
                    }
                    let root = j.root_of(event);
                    Some((
                        id,
                        if root != event || j.is_undone(root) {
                            "Undone"
                        } else {
                            "Undo unavailable"
                        }
                        .to_owned(),
                    ))
                })
                .collect()
        })
    }
    pub fn activity_undo_states(&self) -> std::collections::HashMap<String, String> {
        self.client
            .activity_undos()
            .into_iter()
            .map(|(id, refusal)| (id.to_string(), refusal.unwrap_or("Undoing…").to_owned()))
            .collect()
    }
}
