//! The activity feed's wire types and presentation shared by clients.

use crate::platform::{Request, Response};
use crate::protocol::{self, EventRef, LogEvent};
use serde::Deserialize;
use todo_core::Actor;
use todo_core::actor::AgentId;
use todo_core::time::Timestamp;

/// Paging and filtering for [`crate::SyncClient::activity`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActivityQuery {
    /// Exclusive sequence cursor from the previous page. Omit to start with the newest event.
    pub before: Option<u64>,
    /// Page size (server default: 50; server clamps to 1–200).
    pub limit: Option<usize>,
    /// Only events performed by this agent. This never expands the caller's visibility.
    pub agent: Option<AgentId>,
}

impl ActivityQuery {
    pub(crate) fn request(&self) -> Request {
        let mut params = Vec::new();
        if let Some(n) = self.before {
            params.push(format!("before={n}"));
        }
        if let Some(n) = self.limit {
            params.push(format!("limit={n}"));
        }
        if let Some(agent) = &self.agent {
            // AgentId validates its URL-safe alphabet.
            params.push(format!("agent={agent}"));
        }
        if params.is_empty() {
            Request::get("/api/v1/activity")
        } else {
            Request::get(format!("/api/v1/activity?{}", params.join("&")))
        }
    }
}

/// One page, newest first in log order.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ActivityPage {
    /// Events visible to the caller.
    pub items: Vec<ActivityItem>,
    /// Continue with this `before`, even if a page had no visible items.
    pub next_before: Option<u64>,
}

/// A confirmed event and its undo state when the page was read.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct ActivityItem {
    /// The event and its command metadata.
    pub event: LogEvent,
    /// Plain description prepared by the server, which has the historical payload and the entity's
    /// current name. Render as text, not markup.
    pub summary: String,
    /// This event's effect is currently undone.
    pub undone: bool,
    /// The server can attempt to undo this event; it may still refuse a conflicting change.
    pub can_undo: bool,
}

impl ActivityItem {
    /// What happened, in plain words. No client needs to reconstruct a task name from its replica.
    #[must_use]
    pub fn what_happened(&self) -> &str {
        if self.summary.trim().is_empty() {
            "Made a change"
        } else {
            &self.summary
        }
    }

    /// Exact event identity for undo; the sequence number alone is not safe across log restores.
    #[must_use]
    pub fn event_ref(&self) -> Option<EventRef> {
        self.event.event.seq.map(|seq| EventRef {
            seq,
            id: self.event.event.id,
        })
    }
}

/// An activity read failure. No variant carries raw server error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ActivityError {
    /// No current session or accepted device token.
    #[error("Sign in to see activity.")]
    Unauthorized,
    /// No usable response from the server.
    #[error("Couldn’t load activity. Try again.")]
    Unavailable,
    /// One entry exceeded the server's page budget; the cursor can be used to skip it.
    #[error("One change was too large to show.")]
    TooLarge {
        /// The exclusive cursor for the next page.
        next_before: u64,
    },
}

pub(crate) fn read_response(response: &Response) -> Result<ActivityPage, ActivityError> {
    match response.status {
        200 => serde_json::from_slice(&response.body).map_err(|_| ActivityError::Unavailable),
        401 => Err(ActivityError::Unauthorized),
        _ => {
            if let Some(error) = protocol::error_detail(response)
                && error.code == "item_too_large"
                && let Some(next_before) = error.details.and_then(|d| d["next_before"].as_u64())
            {
                return Err(ActivityError::TooLarge { next_before });
            }
            Err(ActivityError::Unavailable)
        }
    }
}

/// How to name the actor to the owner. Owner device identifiers aren't display names.
#[must_use]
pub fn actor_name(actor: &Actor) -> String {
    match actor {
        Actor::Owner { .. } => "You".to_owned(),
        Actor::Agent { agent_id } => agent_name(agent_id.as_str()),
        Actor::EmailIngest => "Email ingest".to_owned(),
        Actor::Importer => "Import".to_owned(),
        Actor::System => "System".to_owned(),
    }
}

/// Display name from an agent ID (the event log doesn't carry a separate display name).
#[must_use]
pub fn agent_name(id: &str) -> String {
    let mut chars = id.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Relative time, using the caller's clock. Future timestamps (clock skew) read as “Just now”.
#[must_use]
pub fn relative_time(at: Timestamp, now: Timestamp) -> String {
    let seconds = (now - at).as_secs();
    let (n, unit) = match seconds {
        ..=59 => return "Just now".to_owned(),
        60..=3599 => (seconds / 60, "min"),
        3600..=86_399 => (seconds / 3600, "hr"),
        _ => (seconds / 86_400, "day"),
    };
    let plural = if unit == "day" && n != 1 { "s" } else { "" };
    format!("{n} {unit}{plural} ago")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Method;
    use serde_json::json;

    fn response(status: u16, body: serde_json::Value) -> Response {
        Response {
            status,
            body: serde_json::to_vec(&body).unwrap(),
            retry_after: None,
        }
    }

    #[test]
    fn query_sends_only_supplied_fields() {
        assert_eq!(ActivityQuery::default().request().path, "/api/v1/activity");
        let query = ActivityQuery {
            before: Some(50),
            limit: Some(20),
            agent: Some(AgentId::new("sample_agent-1").unwrap()),
        };
        assert_eq!(query.request().method, Method::Get);
        assert_eq!(
            query.request().path,
            "/api/v1/activity?before=50&limit=20&agent=sample_agent-1"
        );
    }

    #[test]
    fn errors_are_typed_never_prose_from_the_response() {
        let error = |status, code, details| {
            response(
                status,
                json!({"error": {"code": code, "message": "raw error text", "details": details}}),
            )
        };
        assert_eq!(
            read_response(&error(401, "unauthorized", json!(null))),
            Err(ActivityError::Unauthorized)
        );
        assert_eq!(
            read_response(&error(500, "item_too_large", json!({"next_before": 5}))),
            Err(ActivityError::TooLarge { next_before: 5 })
        );
        assert_eq!(
            read_response(&error(500, "oops", json!(null))),
            Err(ActivityError::Unavailable)
        );
        assert_eq!(
            read_response(&response(200, json!("not a page"))),
            Err(ActivityError::Unavailable)
        );
    }

    #[test]
    fn descriptions_and_identities_are_read_from_the_event() {
        let id = "00000000-0000-0000-0000-000000000001";
        let target = "00000000-0000-0000-0000-000000000002";
        let page = read_response(&response(200, json!({"items": [{
            "event": {"id": id, "seq": 42, "group": id, "command_id": id, "actor": {"type": "email_ingest"},
                      "at": "2026-11-21T10:00:00Z", "forced": false,
                      "payload": {"op": "updated", "target": {"kind": "task", "id": target}, "changes": []}},
            "summary": "Updated a task", "undone": false, "can_undo": true
        }], "next_before": 42}))).unwrap();
        assert_eq!(page.next_before, Some(42));
        let item = &page.items[0];
        assert_eq!(
            item.event_ref(),
            Some(EventRef {
                seq: 42,
                id: id.parse().unwrap()
            })
        );
        assert_eq!(item.what_happened(), "Updated a task");
        assert_eq!(actor_name(&item.event.event.actor), "Email ingest");
        let mut item = item.clone();
        item.summary = " ".into();
        assert_eq!(item.what_happened(), "Made a change");
    }

    #[test]
    fn attribution_and_time_do_not_need_platform_services() {
        assert_eq!(actor_name(&Actor::owner("web").unwrap()), "You");
        assert_eq!(actor_name(&Actor::agent("codex").unwrap()), "Codex");
        assert_eq!(actor_name(&Actor::Importer), "Import");
        assert_eq!(actor_name(&Actor::System), "System");
        let now = Timestamp::from_second(200_000).unwrap();
        for (ago, label) in [
            (-5, "Just now"),
            (59, "Just now"),
            (60, "1 min ago"),
            (3599, "59 min ago"),
            (3600, "1 hr ago"),
            (86_400, "1 day ago"),
            (172_800, "2 days ago"),
        ] {
            assert_eq!(
                relative_time(Timestamp::from_second(200_000 - ago).unwrap(), now),
                label
            );
        }
    }
}
