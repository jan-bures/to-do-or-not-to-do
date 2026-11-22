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
}
#[derive(Debug, uniffi::Record)]
pub struct FeedPage {
    pub rows: Vec<ActivityRow>,
    pub before: Option<u64>,
    pub problem: Option<String>,
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
                rows: page
                    .items
                    .into_iter()
                    .filter_map(|i| {
                        let event = i.event_ref()?;
                        let e = &i.event.event;
                        Some(ActivityRow {
                            id: event.id.to_string(),
                            seq: event.seq,
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
            }),
        }
    }
    pub fn undo_activity_event(&self, seq: u64, id: String) -> Result<(), FfiError> {
        let id = id.parse().map_err(|_| invalid("Couldn't undo that."))?;
        self.client
            .undo_activity(EventRef { seq, id })
            .map_err(|_| invalid("Couldn't undo that."))?;
        Ok(())
    }
    pub fn activity_revision(&self) -> u64 {
        self.client.cursor()
    }
    pub fn activity_undo_states(&self) -> std::collections::HashMap<String, String> {
        self.client
            .activity_undos()
            .into_iter()
            .map(|(id, refusal)| (id.to_string(), refusal.unwrap_or("Undoing…").to_owned()))
            .collect()
    }
}
