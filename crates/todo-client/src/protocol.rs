//! The server's HTTP API as the sync engine uses it: paths, bodies, and how responses are classified.

use crate::outbox::{CommandId, Epoch, Operation, Rejection};
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
/// The SSE event name of the first frame of every stream.
pub const HELLO_FRAME: &str = "hello";

/// Error code: the request named another log epoch than the server's (409).
pub const EPOCH_MISMATCH: &str = "epoch_mismatch";
/// Error code: an undo's `expected_event_id` is not the event at its `seq` (409).
pub const EVENT_MISMATCH: &str = "event_mismatch";
/// The error code of a request whose session CSRF token (or `Origin`) the server refused. Says
/// nothing about the command: the web transport fetches the session's current token and retries (see
/// [`crate::session`]), and the engine retries later if it persists.
pub const CSRF: &str = "csrf";

#[derive(Serialize)]
struct CommandRequest<'a> {
    command_id: CommandId,
    command: &'a Command,
}

#[derive(Serialize)]
struct UndoRequest {
    force: bool,
    command_id: CommandId,
    expected_event_id: EventId,
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<Epoch>,
}

/// An event of the server's log with the command that produced it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEvent {
    /// The event.
    #[serde(flatten)]
    pub event: Event,
    /// The command (or undo) that produced it.
    pub command_id: CommandId,
    /// Its index among that command's events.
    #[serde(default)]
    pub command_idx: u32,
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
    /// The log epoch the events' sequence numbers belong to.
    #[serde(default)]
    pub epoch: Option<Epoch>,
}

/// A page of `GET /api/v1/events`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct EventPage {
    /// Events after `since`, oldest first.
    pub events: Vec<LogEvent>,
    /// Pass as `since` for the next page.
    pub next_since: u64,
    /// The newest sequence number when the page was read.
    pub head_seq: u64,
    /// True if more events follow.
    pub has_more: bool,
    /// The log epoch the sequence numbers belong to.
    pub epoch: Epoch,
}

/// Data of the `hello` SSE frame.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Hello {
    /// The log epoch the stream's sequence numbers belong to.
    pub epoch: Epoch,
    /// The newest sequence number when the stream opened. Servers leave it out for agent tokens.
    #[serde(default)]
    pub head_seq: Option<u64>,
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
        Operation::Undo {
            seq,
            force,
            expected_event_id,
            epoch,
        } => Request::post(
            format!("/api/v1/events/{seq}/undo"),
            serde_json::to_vec(&UndoRequest {
                force: *force,
                command_id,
                expected_event_id: *expected_event_id,
                epoch: *epoch,
            })?,
        ),
    })
}

fn epoch_param(epoch: Option<Epoch>) -> String {
    epoch.map(|e| format!("&epoch={e}")).unwrap_or_default()
}

/// A JSON page of the event log. With `epoch`, the server answers 409 `epoch_mismatch` if its log has
/// another one.
#[must_use]
pub fn events_page_request(since: u64, limit: usize, epoch: Option<Epoch>) -> Request {
    Request::get(format!(
        "/api/v1/events?since={since}&limit={limit}{}",
        epoch_param(epoch)
    ))
}

/// The event stream from `since` (see [`events_page_request`] for `epoch`).
#[must_use]
pub fn stream_request(since: u64, epoch: Option<Epoch>) -> Request {
    Request::get(format!(
        "/api/v1/events?since={since}{}",
        epoch_param(epoch)
    ))
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
/// 403 `csrf` (the browser session's token or origin, not the command), 408 (nothing stored), 429
/// (rate limited) and a bare 404 without details (wrong base URL or an unknown route, not a missing
/// entity). Everything else (5xx, including 503 `outcome_unknown`,
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
            Some(d) if res.status == 403 && d.code == CSRF => {
                retry("the server refused the session's CSRF token (403)".into())
            }
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
        // A CSRF refusal is about the browser session, not the command.
        assert!(matches!(
            classify_send(&res(
                403,
                r#"{"error":{"code":"csrf","message":"bad token"}}"#
            )),
            SendOutcome::Retry {
                unauthorized: false,
                ..
            }
        ));
        // Any other 403 in the error format is a verdict.
        assert!(matches!(
            classify_send(&res(
                403,
                r#"{"error":{"code":"forbidden","message":"no"}}"#
            )),
            SendOutcome::Rejected(_)
        ));
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
    fn undo_carries_its_command_id_expected_event_and_epoch() {
        let id = uuid::Uuid::from_u128(7);
        let event = EventId::from_uuid(uuid::Uuid::from_u128(8));
        let epoch = uuid::Uuid::from_u128(9);
        let req = entry_request(
            id,
            &Operation::Undo {
                seq: 3,
                force: true,
                expected_event_id: event,
                epoch: Some(epoch),
            },
        )
        .unwrap();
        assert_eq!(req.path, "/api/v1/events/3/undo");
        let body: Value = serde_json::from_slice(req.body.as_deref().unwrap()).unwrap();
        assert_eq!(body["command_id"], id.to_string());
        assert_eq!(body["force"], true);
        assert_eq!(
            body["expected_event_id"],
            serde_json::to_value(event).unwrap()
        );
        assert_eq!(body["epoch"], epoch.to_string());
    }

    #[test]
    fn requests_name_the_epoch_when_known() {
        let e = uuid::Uuid::from_u128(5);
        assert_eq!(stream_request(4, None).path, "/api/v1/events?since=4");
        assert_eq!(
            stream_request(4, Some(e)).path,
            format!("/api/v1/events?since=4&epoch={e}")
        );
        assert_eq!(
            events_page_request(4, 10, Some(e)).path,
            format!("/api/v1/events?since=4&limit=10&epoch={e}")
        );
    }

    #[test]
    fn a_stale_undo_is_rejected_for_good() {
        for code in [EPOCH_MISMATCH, EVENT_MISMATCH] {
            let body = format!(r#"{{"error":{{"code":"{code}","message":"x","details":{{}}}}}}"#);
            assert!(matches!(
                classify_send(&res(409, &body)),
                SendOutcome::Rejected(r) if r.code == code
            ));
        }
    }
}
