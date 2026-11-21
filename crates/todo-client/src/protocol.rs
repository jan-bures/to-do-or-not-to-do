//! The server's HTTP API as the sync engine uses it: paths, bodies, and how responses are classified.

use crate::outbox::{CommandId, Operation, Rejection};
use crate::platform::{Request, Response};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use todo_core::command::Command;
use todo_core::event::Event;
use todo_core::ids::EventId;

/// `POST` path for commands.
pub const COMMANDS_PATH: &str = "/api/v1/commands";

/// The SSE event name of an event frame.
pub const EVENT_FRAME: &str = "event";
/// The SSE event name of the "too far behind" notice.
pub const CATCH_UP_FRAME: &str = "catch_up";

#[derive(Serialize)]
struct CommandRequest<'a> {
    command_id: CommandId,
    command: &'a Command,
}

#[derive(Serialize)]
struct UndoRequest {
    force: bool,
    command_id: CommandId,
}

/// Reference to a stored event.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRef {
    /// Position in the log.
    pub seq: u64,
    /// Event ID.
    pub id: EventId,
}

/// What a command or undo produced (the 200 body of both endpoints).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandResult {
    /// The command ID.
    pub command_id: CommandId,
    /// The stored events, in log order. Empty if the command changed nothing.
    pub events: Vec<EventRef>,
    /// One-line description.
    #[serde(default)]
    pub summary: String,
}

/// A page of `GET /api/v1/events`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct EventPage {
    /// Events after `since`, oldest first.
    pub events: Vec<Event>,
    /// Pass as `since` for the next page.
    pub next_since: u64,
    /// The newest sequence number when the page was read.
    pub head_seq: u64,
    /// True if more events follow.
    pub has_more: bool,
}

/// Data of the `catch_up` SSE frame.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CatchUpNotice {
    /// The last sequence number the stream delivered.
    pub since: u64,
    /// Page up to here.
    pub head_seq: u64,
}

/// The server's error envelope.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ErrorBody {
    /// The error.
    pub error: ErrorDetail,
}

/// One error.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ErrorDetail {
    /// Machine-readable code.
    pub code: String,
    /// Description.
    #[serde(default)]
    pub message: String,
    /// Structured details.
    #[serde(default)]
    pub details: Option<Value>,
}

/// Parses an error envelope, if the body is one.
#[must_use]
pub fn error_detail(res: &Response) -> Option<ErrorDetail> {
    serde_json::from_slice::<ErrorBody>(&res.body)
        .ok()
        .map(|b| b.error)
}

/// The request that sends an outbox entry.
pub fn entry_request(command_id: CommandId, op: &Operation) -> Result<Request, serde_json::Error> {
    Ok(match op {
        Operation::Command { command } => Request::post(
            COMMANDS_PATH,
            serde_json::to_vec(&CommandRequest {
                command_id,
                command,
            })?,
        ),
        Operation::Undo { seq, force } => Request::post(
            format!("/api/v1/events/{seq}/undo"),
            serde_json::to_vec(&UndoRequest {
                force: *force,
                command_id,
            })?,
        ),
    })
}

/// A JSON page of the event log.
#[must_use]
pub fn events_page_request(since: u64, limit: usize) -> Request {
    Request::get(format!("/api/v1/events?since={since}&limit={limit}"))
}

/// The event stream from `since`.
#[must_use]
pub fn stream_request(since: u64) -> Request {
    Request::get(format!("/api/v1/events?since={since}"))
}

/// How a response to a command or undo is handled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    /// Stored (or replayed).
    Accepted(CommandResult),
    /// Not definitive: send the same command ID again later.
    Retry {
        /// What went wrong, for the status.
        reason: String,
        /// The server's `Retry-After`.
        after: Option<Duration>,
        /// True for authentication failures (the token is wrong; retrying will not help until it is
        /// fixed, but the command itself is fine).
        unauthorized: bool,
    },
    /// The server refused the command for good.
    Rejected(Rejection),
}

/// Classifies a response to `POST /commands` or `POST /events/{seq}/undo`.
///
/// Definitive rejections are 4xx answers in the server's error format, except 401 (token problem),
/// 408 (nothing stored), 429 (rate limited) and a bare 404 without details (wrong base URL or an
/// unknown route, not a missing entity). Everything else (5xx, including 503 `outcome_unknown`,
/// unparseable answers from a proxy) is retried with the same command ID.
#[must_use]
pub fn classify_send(res: &Response) -> SendOutcome {
    let retry = |reason: String| SendOutcome::Retry {
        reason,
        after: res.retry_after,
        unauthorized: false,
    };
    match res.status {
        200..=299 => match serde_json::from_slice::<CommandResult>(&res.body) {
            Ok(r) => SendOutcome::Accepted(r),
            Err(e) => retry(format!("unreadable response: {e}")),
        },
        401 => SendOutcome::Retry {
            reason: "the server rejected the token (401)".into(),
            after: res.retry_after,
            unauthorized: true,
        },
        408 | 429 => retry(format!("server answered {}", res.status)),
        400..=499 => match error_detail(res) {
            Some(d) if !(res.status == 404 && d.details.is_none()) => {
                SendOutcome::Rejected(Rejection {
                    status: res.status,
                    code: d.code,
                    message: d.message,
                })
            }
            Some(d) => retry(format!("server answered {} {}", res.status, d.code)),
            None => retry(format!(
                "server answered {} without an error body",
                res.status
            )),
        },
        _ => {
            let code = error_detail(res).map(|d| d.code).unwrap_or_default();
            retry(format!("server answered {} {code}", res.status))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(status: u16, body: &str) -> Response {
        Response {
            status,
            retry_after: None,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn classification() {
        let ok = format!(r#"{{"command_id":"{}","events":[]}}"#, uuid::Uuid::nil());
        assert!(matches!(
            classify_send(&res(200, &ok)),
            SendOutcome::Accepted(_)
        ));
        for status in [408, 429, 500, 502, 503] {
            assert!(
                matches!(
                    classify_send(&res(
                        status,
                        r#"{"error":{"code":"outcome_unknown","message":""}}"#
                    )),
                    SendOutcome::Retry {
                        unauthorized: false,
                        ..
                    }
                ),
                "{status}"
            );
        }
        assert!(matches!(
            classify_send(&res(401, "")),
            SendOutcome::Retry {
                unauthorized: true,
                ..
            }
        ));
        let rejected = classify_send(&res(
            409,
            r#"{"error":{"code":"deleted","message":"gone","details":{"entity":1}}}"#,
        ));
        assert_eq!(
            rejected,
            SendOutcome::Rejected(Rejection {
                status: 409,
                code: "deleted".into(),
                message: "gone".into()
            })
        );
        // A proxy's HTML error page or an unknown route is not a verdict on the command.
        assert!(matches!(
            classify_send(&res(404, "<html>")),
            SendOutcome::Retry { .. }
        ));
        assert!(matches!(
            classify_send(&res(
                404,
                r#"{"error":{"code":"not_found","message":"no such endpoint"}}"#
            )),
            SendOutcome::Retry { .. }
        ));
        assert!(matches!(
            classify_send(&res(
                404,
                r#"{"error":{"code":"not_found","message":"x","details":{"entity":{}}}}"#
            )),
            SendOutcome::Rejected(_)
        ));
    }

    #[test]
    fn undo_always_carries_a_command_id() {
        let id = uuid::Uuid::from_u128(7);
        let req = entry_request(
            id,
            &Operation::Undo {
                seq: 3,
                force: true,
            },
        )
        .unwrap();
        assert_eq!(req.path, "/api/v1/events/3/undo");
        let body: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["command_id"], id.to_string());
        assert_eq!(body["force"], true);
    }
}
