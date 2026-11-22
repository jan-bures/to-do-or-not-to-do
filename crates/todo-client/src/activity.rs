//! The activity feed's wire types and presentation shared by clients: the paged read, the pages a
//! client has read so far ([`Feed`]), the undos it asked for ([`Undos`]) and what each row offers
//! ([`UndoAction`]), and the words for what happened, who did it and when.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::outbox::CommandId;
use crate::platform::{Request, Response};
use crate::protocol::{self, EventRef, LogEvent};
use serde::Deserialize;
use todo_core::Actor;
use todo_core::actor::AgentId;
use todo_core::ids::EventId;
use todo_core::state::Journal;
use todo_core::time::{Date, Timestamp};

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

/// A day group's heading: “Today”, “Yesterday”, else the shared short date (`Thu 19 Nov`).
#[must_use]
pub fn day_label(day: Date, today: Date) -> String {
    if day == today {
        "Today".to_owned()
    } else if today.yesterday().is_ok_and(|y| y == day) {
        "Yesterday".to_owned()
    } else {
        crate::intent::quick_add::short_date(day, today)
    }
}

/// The feed's empty line: nothing at all, or nothing from the agent the feed is filtered to.
#[must_use]
pub fn empty_line(filtered: bool) -> &'static str {
    if filtered {
        "No activity from this agent."
    } else {
        "No activity yet."
    }
}

/// What a client shows while a page is being read.
pub const LOADING: &str = "Loading activity…";
/// The feed's order, above it.
pub const NEWEST_FIRST: &str = "Newest first";
/// Reads the newest changes again.
pub const REFRESH: &str = "Refresh activity";
/// Takes an agent filter off.
pub const CLEAR_FILTER: &str = "Clear filter";
/// Reads the next older page.
pub const LOAD_OLDER: &str = "Load older activity";

/// The button beside a failed read: try the same page again, or skip the one change too large to
/// show.
#[must_use]
pub fn retry_label(error: ActivityError) -> &'static str {
    if matches!(error, ActivityError::TooLarge { .. }) {
        "Skip this change"
    } else {
        "Try again"
    }
}
/// What a client says once an undo it asked for has gone through.
pub const UNDONE: &str = "Change undone.";
/// What a client says while an undo it asked for waits to be sent.
pub const UNDOING: &str = "Undoing…";

/// One feed row, as a client keeps it between pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The event, exactly: undo is asked for by sequence and ID together.
    pub target: EventRef,
    /// The original command's batch: one undo covers every event of it.
    pub group: EventId,
    pub actor: Actor,
    pub at: Timestamp,
    /// What happened, in plain words ([`ActivityItem::what_happened`]).
    pub what: String,
    /// The event is itself an undo (or a redo).
    pub compensating: bool,
    /// As the server read it: the event's effect is undone.
    pub undone: bool,
    /// As the server read it: the event can be undone.
    pub can_undo: bool,
}

impl Entry {
    /// The row for an item; `None` for an event without a sequence number (never in a served page).
    #[must_use]
    pub fn from_item(item: ActivityItem) -> Option<Self> {
        let target = item.event_ref()?;
        let what = item.what_happened().to_owned();
        let event = item.event.event;
        Some(Self {
            target,
            group: event.group,
            actor: event.actor,
            at: event.at,
            what,
            compensating: event.undoes.is_some(),
            undone: item.undone,
            can_undo: item.can_undo,
        })
    }
}

/// Which page a read asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// The newest changes: a refresh, which replaces every page read so far.
    Latest,
    /// The page after the oldest one read so far.
    Older,
}

/// What [`Feed::finish`] did with a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// A reply to a request that has been replaced (a refresh, another filter): nothing changed.
    Ignore,
    /// The page, or its error, is in the feed.
    Applied,
    /// A refresh asked for while this page was read: read the newest page now.
    Refresh,
    /// The server wants a sign-in. The error is in the feed too.
    SignIn,
}

/// The pages of activity a client has read so far, newest first, and the one read in flight.
///
/// A client asks [`Feed::begin`] before it reads a page and hands the reply to [`Feed::finish`]; the
/// feed keeps the cursor moving (even past a page with nothing the caller may see), drops replies to
/// requests it has since replaced, and queues a refresh asked for while an older page is read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Feed {
    entries: BTreeMap<Reverse<u64>, Entry>,
    next: Option<u64>,
    pending: Option<Load>,
    refresh_queued: bool,
    error: Option<(Option<u64>, ActivityError)>,
    loaded: bool,
    request: u64,
}

impl Feed {
    /// Forgets every page (a replaced log, another filter). A reply still in flight is ignored.
    pub fn reset(&mut self) {
        *self = Self {
            request: self.request.wrapping_add(1),
            ..Self::default()
        };
    }

    /// Starts reading the page before `before` (the newest page for `None`): the request's number,
    /// for [`Feed::finish`]. `None` while another read is in flight: nothing is read then, but a
    /// refresh asked for during an older page's read follows it.
    pub fn begin(&mut self, before: Option<u64>) -> Option<u64> {
        let kind = if before.is_none() {
            Load::Latest
        } else {
            Load::Older
        };
        if let Some(pending) = self.pending {
            if kind == Load::Latest && pending == Load::Older {
                self.refresh_queued = true;
            }
            return None;
        }
        self.pending = Some(kind);
        self.error = None;
        self.request = self.request.wrapping_add(1);
        Some(self.request)
    }

    /// Takes in the reply to request `request`, which read the page before `before`.
    pub fn finish(
        &mut self,
        request: u64,
        before: Option<u64>,
        result: Result<ActivityPage, ActivityError>,
    ) -> Loaded {
        if self.request != request || self.pending.is_none() {
            return Loaded::Ignore;
        }
        self.pending = None;
        let refresh = std::mem::take(&mut self.refresh_queued);
        match result {
            Err(ActivityError::Unauthorized) => {
                self.error = Some((before, ActivityError::Unauthorized));
                return Loaded::SignIn;
            }
            // The refresh replaces every page: don't keep this one only to drop it.
            _ if refresh => return Loaded::Refresh,
            Ok(page) => {
                if before.is_none() {
                    self.entries.clear();
                }
                self.loaded = true;
                // A cursor must move on, even past a page with nothing the caller may see.
                self.next = page
                    .next_before
                    .filter(|next| before.is_none_or(|b| *next < b));
                for entry in page.items.into_iter().filter_map(Entry::from_item) {
                    self.entries.insert(Reverse(entry.target.seq), entry);
                }
            }
            Err(error) => self.error = Some((before, error)),
        }
        Loaded::Applied
    }

    /// Every row read so far, newest first.
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }

    /// The row for the event at `seq`.
    #[must_use]
    pub fn entry(&self, seq: u64) -> Option<&Entry> {
        self.entries.get(&Reverse(seq))
    }

    /// How many rows have been read.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Nothing has been read (yet).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The cursor for the next older page, when there is one and the last read didn't fail.
    #[must_use]
    pub fn older(&self) -> Option<u64> {
        self.next.filter(|_| self.error.is_none())
    }

    /// The read in flight.
    #[must_use]
    pub fn pending(&self) -> Option<Load> {
        self.pending
    }

    /// A refresh is being read, or waits for an older page's read.
    #[must_use]
    pub fn refreshing(&self) -> bool {
        self.pending == Some(Load::Latest) || self.refresh_queued
    }

    /// Why the last read failed.
    #[must_use]
    pub fn error(&self) -> Option<ActivityError> {
        self.error.map(|(_, error)| error)
    }

    /// The cursor to read again after a failure: the same page, or past one change too large to show.
    #[must_use]
    pub fn retry(&self) -> Option<Option<u64>> {
        self.error.map(|(before, error)| match error {
            ActivityError::TooLarge { next_before } => Some(next_before),
            _ => before,
        })
    }

    /// At least one page has been read.
    #[must_use]
    pub fn loaded(&self) -> bool {
        self.loaded
    }

    /// The feed has been read and there is nothing in it, nor anything older to read.
    #[must_use]
    pub fn is_empty_feed(&self) -> bool {
        self.loaded && self.entries.is_empty() && self.next.is_none() && self.error.is_none()
    }

    /// The rows grouped by the local day `day_of` puts them on, newest first: the sequence numbers of
    /// each day's rows, newest first. Days group across pages.
    #[must_use]
    pub fn days(&self, day_of: impl Fn(Timestamp) -> Date) -> Vec<(Date, Vec<u64>)> {
        let mut days: BTreeMap<Reverse<Date>, Vec<u64>> = BTreeMap::new();
        for entry in self.entries.values() {
            days.entry(Reverse(day_of(entry.at)))
                .or_default()
                .push(entry.target.seq);
        }
        days.into_iter()
            .map(|(Reverse(day), seqs)| (day, seqs))
            .collect()
    }
}

/// What became of an undo a client asked for from the feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoOutcome {
    /// Queued in the outbox as this command.
    Pending(CommandId),
    /// Done at once (a client with no server behind it, such as a fixture).
    Done,
    /// Refused, in plain words.
    Refused(&'static str),
}

/// The undos a client asked for from the feed, by the event they undo, with the event's batch: a
/// second undo of the same batch is never asked for while one is pending.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Undos {
    outcomes: BTreeMap<EventId, (EventId, UndoOutcome)>,
}

impl Undos {
    /// What became of the undo of `event`.
    #[must_use]
    pub fn outcome(&self, event: EventId) -> Option<UndoOutcome> {
        self.outcomes.get(&event).map(|&(_, outcome)| outcome)
    }

    /// Why the undo of `event` was refused, in plain words.
    #[must_use]
    pub fn refusal(&self, event: EventId) -> Option<&'static str> {
        match self.outcome(event) {
            Some(UndoOutcome::Refused(why)) => Some(why),
            _ => None,
        }
    }

    /// An undo of batch `group` that is pending or done (a refused one doesn't count).
    #[must_use]
    pub fn group_outcome(&self, group: EventId) -> Option<UndoOutcome> {
        self.outcomes.values().find_map(|&(g, outcome)| {
            (g == group && !matches!(outcome, UndoOutcome::Refused(_))).then_some(outcome)
        })
    }

    /// Records what asking to undo `target` (of batch `group`) came to. Returns what to tell the owner.
    pub fn record(
        &mut self,
        target: EventId,
        group: EventId,
        outcome: UndoOutcome,
    ) -> &'static str {
        self.outcomes.insert(target, (group, outcome));
        match outcome {
            UndoOutcome::Pending(_) => UNDOING,
            UndoOutcome::Done => UNDONE,
            UndoOutcome::Refused(why) => why,
        }
    }

    /// Takes in what became of the pending undos: `result` is `None` while one is still queued (see
    /// [`crate::SyncClient::undo_result`]). A refusal stays beside its row; one that went through is
    /// forgotten, since the confirmed journal now knows it (and a later redo may follow). Returns what
    /// to tell the owner about the last one that settled.
    pub fn settle(
        &mut self,
        result: impl Fn(CommandId) -> Option<Result<(), &'static str>>,
    ) -> Option<&'static str> {
        let settled: Vec<_> = self
            .outcomes
            .iter()
            .filter_map(|(&id, &(_, outcome))| match outcome {
                UndoOutcome::Pending(command) => result(command).map(|r| (id, r)),
                _ => None,
            })
            .collect();
        let mut said = None;
        for (id, result) in settled {
            match result {
                Ok(()) => {
                    self.outcomes.remove(&id);
                    said = Some(UNDONE);
                }
                Err(why) => {
                    if let Some(entry) = self.outcomes.get_mut(&id) {
                        entry.1 = UndoOutcome::Refused(why);
                    }
                    said = Some(why);
                }
            }
        }
        said
    }

    /// Any undo still pending.
    #[must_use]
    pub fn any_pending(&self) -> bool {
        self.outcomes
            .values()
            .any(|(_, o)| matches!(o, UndoOutcome::Pending(_)))
    }
}

/// What a row's Undo offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UndoAction {
    Undo,
    /// Queued, waiting to be sent or confirmed.
    Undoing,
    Undone,
    /// The event isn't the head of its undo chain (or the server won't undo it).
    Unavailable,
}

impl UndoAction {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Undo => "Undo",
            Self::Undoing => "Undoing",
            Self::Undone => "Undone",
            Self::Unavailable => "Undo unavailable",
        }
    }

    /// It is a button (Undo, or Undoing while it waits); the others are quiet words.
    #[must_use]
    pub fn is_button(self) -> bool {
        matches!(self, Self::Undo | Self::Undoing)
    }
}

/// What the row for `entry` offers. `confirmed` is the replica's journal's view of the event
/// ([`confirmed`]), `pending` whether the outbox holds an undo of it, and `group` what became of an
/// undo of its batch asked for here ([`Undos::group_outcome`]). The feed can be ahead of the replica,
/// so the page's flags count until the journal has seen the event; once either knows it is no longer
/// the head of its chain, it can't be undone again (a redo makes a new link).
#[must_use]
pub fn undo_action(
    entry: &Entry,
    confirmed: Option<(bool, bool)>,
    pending: bool,
    group: Option<UndoOutcome>,
) -> UndoAction {
    let (undone, can_undo) = match confirmed {
        Some((undone, false)) => (undone, false),
        _ => (entry.undone, entry.can_undo),
    };
    if group == Some(UndoOutcome::Done) || undone {
        UndoAction::Undone
    } else if !can_undo {
        UndoAction::Unavailable
    } else if pending || matches!(group, Some(UndoOutcome::Pending(_))) {
        UndoAction::Undoing
    } else {
        UndoAction::Undo
    }
}

/// The confirmed journal's view of event `id`: `(undone, head)`, whether its effect is undone and
/// whether it is the newest link of its undo chain. `None` until the replica has applied it. For a
/// compensating event, “undone” means a later link has redone what it undid.
#[must_use]
pub fn confirmed(journal: &Journal, id: EventId, compensating: bool) -> Option<(bool, bool)> {
    if !journal.has_seen(id) {
        return None;
    }
    let root = journal.root_of(id);
    let head = journal.head_of(root);
    let undone = if compensating {
        head != id
    } else {
        journal.is_undone(root)
    };
    Some((undone, head == id))
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

    fn item(seq: u64, at: &str, undone: bool) -> ActivityItem {
        let id = format!("00000000-0000-0000-0000-{seq:012}");
        serde_json::from_value(json!({
            "event": {"id": id, "seq": seq, "group": id, "command_id": id, "actor": {"type": "agent", "agent_id": "codex"},
                      "at": at, "forced": false,
                      "payload": {"op": "updated", "target": {"kind": "task", "id": id}, "changes": []}},
            "summary": format!("Change {seq}"), "undone": undone, "can_undo": !undone
        }))
        .unwrap()
    }

    fn page(items: Vec<ActivityItem>, next_before: Option<u64>) -> ActivityPage {
        ActivityPage { items, next_before }
    }

    fn seqs(feed: &Feed) -> Vec<u64> {
        feed.entries().map(|e| e.target.seq).collect()
    }

    #[test]
    fn the_feed_pages_newest_first_and_its_cursor_always_moves_on() {
        let mut feed = Feed::default();
        assert!(!feed.loaded() && !feed.is_empty_feed());
        let r = feed.begin(None).unwrap();
        assert_eq!(feed.pending(), Some(Load::Latest));
        assert!(feed.begin(None).is_none(), "one read at a time");
        let first = page(
            vec![
                item(9, "2026-11-21T10:00:00Z", false),
                item(8, "2026-11-20T10:00:00Z", false),
            ],
            Some(8),
        );
        assert_eq!(feed.finish(r, None, Ok(first)), Loaded::Applied);
        assert_eq!(seqs(&feed), [9, 8]);
        assert_eq!(feed.older(), Some(8));
        // An older page whose cursor doesn't move on ends the feed instead of looping.
        let r = feed.begin(Some(8)).unwrap();
        let stuck = page(vec![item(7, "2026-11-19T10:00:00Z", true)], Some(8));
        assert_eq!(feed.finish(r, Some(8), Ok(stuck)), Loaded::Applied);
        assert_eq!(seqs(&feed), [9, 8, 7]);
        assert_eq!(feed.older(), None);
        assert!(feed.entry(7).unwrap().undone);
        // A stale reply changes nothing.
        assert_eq!(
            feed.finish(r, Some(8), Ok(page(Vec::new(), None))),
            Loaded::Ignore
        );
        // A refresh replaces every page.
        let r = feed.begin(None).unwrap();
        let fresh = page(vec![item(10, "2026-11-21T11:00:00Z", false)], None);
        assert_eq!(feed.finish(r, None, Ok(fresh)), Loaded::Applied);
        assert_eq!(seqs(&feed), [10]);
        assert!(!feed.is_empty_feed());
        feed.reset();
        assert!(feed.is_empty() && !feed.loaded());
        let r = feed.begin(None).unwrap();
        feed.finish(r, None, Ok(page(Vec::new(), None)));
        assert!(feed.is_empty_feed());
    }

    #[test]
    fn a_refresh_asked_for_during_an_older_read_follows_it_and_errors_can_be_retried() {
        let mut feed = Feed::default();
        let r = feed.begin(None).unwrap();
        feed.finish(
            r,
            None,
            Ok(page(vec![item(5, "2026-11-21T10:00:00Z", false)], Some(5))),
        );
        let older = feed.begin(Some(5)).unwrap();
        assert!(feed.begin(None).is_none());
        assert!(feed.refreshing());
        let reply = page(vec![item(4, "2026-11-21T09:00:00Z", false)], None);
        assert_eq!(feed.finish(older, Some(5), Ok(reply)), Loaded::Refresh);
        assert_eq!(seqs(&feed), [5], "the stale older page isn't kept");
        assert!(!feed.refreshing());
        // A failure keeps the rows, stops paging and says where to try again.
        let r = feed.begin(Some(5)).unwrap();
        feed.finish(r, Some(5), Err(ActivityError::Unavailable));
        assert_eq!(feed.error(), Some(ActivityError::Unavailable));
        assert_eq!(feed.older(), None);
        assert_eq!(feed.retry(), Some(Some(5)));
        let r = feed.begin(Some(5)).unwrap();
        assert_eq!(feed.error(), None);
        feed.finish(r, Some(5), Err(ActivityError::TooLarge { next_before: 3 }));
        assert_eq!(
            feed.retry(),
            Some(Some(3)),
            "skip the change too large to show"
        );
        assert_eq!(retry_label(feed.error().unwrap()), "Skip this change");
        assert_eq!(retry_label(ActivityError::Unavailable), "Try again");
        let r = feed.begin(None).unwrap();
        assert_eq!(
            feed.finish(r, None, Err(ActivityError::Unauthorized)),
            Loaded::SignIn
        );
        assert_eq!(feed.error(), Some(ActivityError::Unauthorized));
        assert_eq!(seqs(&feed), [5]);
    }

    #[test]
    fn days_group_across_pages_newest_first_with_shared_labels() {
        let mut feed = Feed::default();
        let r = feed.begin(None).unwrap();
        feed.finish(
            r,
            None,
            Ok(page(
                vec![
                    item(3, "2026-11-21T10:00:00Z", false),
                    item(2, "2026-11-20T23:30:00Z", false),
                ],
                Some(2),
            )),
        );
        let r = feed.begin(Some(2)).unwrap();
        let older = page(vec![item(1, "2026-11-20T08:00:00Z", false)], None);
        feed.finish(r, Some(2), Ok(older));
        let utc = |at: Timestamp| todo_core::time::utc_date(at);
        let days = feed.days(utc);
        let today: Date = "2026-11-21".parse().unwrap();
        assert_eq!(
            days,
            [
                (today, vec![3]),
                ("2026-11-20".parse().unwrap(), vec![2, 1])
            ]
        );
        assert_eq!(day_label(today, today), "Today");
        assert_eq!(day_label(days[1].0, today), "Yesterday");
        assert_eq!(
            day_label("2026-11-19".parse().unwrap(), today),
            "Thu 19 Nov"
        );
        assert_eq!(empty_line(true), "No activity from this agent.");
        assert_eq!(empty_line(false), "No activity yet.");
    }

    #[test]
    fn undos_track_each_batch_once_and_keep_refusals_in_plain_words() {
        let mut undos = Undos::default();
        let id = |n| EventId::from_uuid(uuid::Uuid::from_u128(n));
        let (a, b, group) = (id(1), id(2), id(3));
        let command = uuid::Uuid::from_u128(7);
        assert_eq!(
            undos.record(a, group, UndoOutcome::Pending(command)),
            UNDOING
        );
        assert!(undos.any_pending());
        // Another event of the same batch is covered by that undo.
        assert_eq!(
            undos.group_outcome(group),
            Some(UndoOutcome::Pending(command))
        );
        assert_eq!(undos.settle(|_| None), None, "still queued");
        let why = crate::failure::undo_message("undo_conflict");
        assert_eq!(undos.settle(|_| Some(Err(why))), Some(why));
        assert_eq!(undos.refusal(a), Some(why));
        assert_eq!(undos.group_outcome(group), None, "a refusal can be retried");
        assert!(!undos.any_pending());
        undos.record(b, group, UndoOutcome::Pending(command));
        assert_eq!(undos.settle(|_| Some(Ok(()))), Some(UNDONE));
        assert_eq!(undos.outcome(b), None, "the journal knows it now");
        assert_eq!(undos.record(b, group, UndoOutcome::Done), UNDONE);
        assert_eq!(undos.record(a, group, UndoOutcome::Refused(why)), why);
    }

    #[test]
    fn a_rows_action_prefers_the_journal_once_it_has_seen_the_event() {
        let entry = Entry::from_item(item(4, "2026-11-21T10:00:00Z", false)).unwrap();
        assert_eq!(entry.what, "Change 4");
        assert!(!entry.compensating);
        let command = uuid::Uuid::from_u128(1);
        assert_eq!(undo_action(&entry, None, false, None), UndoAction::Undo);
        assert_eq!(undo_action(&entry, None, true, None), UndoAction::Undoing);
        assert_eq!(
            undo_action(&entry, None, false, Some(UndoOutcome::Pending(command))),
            UndoAction::Undoing
        );
        assert_eq!(
            undo_action(&entry, None, false, Some(UndoOutcome::Done)),
            UndoAction::Undone
        );
        assert_eq!(
            undo_action(&entry, Some((true, false)), false, None),
            UndoAction::Undone
        );
        assert_eq!(
            undo_action(&entry, Some((false, false)), true, None),
            UndoAction::Unavailable
        );
        // The page is newer than the replica's flags while the event is still the head.
        assert_eq!(
            undo_action(&entry, Some((false, true)), false, None),
            UndoAction::Undo
        );
        let undone = Entry::from_item(item(3, "2026-11-21T10:00:00Z", true)).unwrap();
        assert_eq!(undo_action(&undone, None, false, None), UndoAction::Undone);
        assert_eq!(UndoAction::Undoing.label(), "Undoing");
        assert!(UndoAction::Undoing.is_button() && !UndoAction::Undone.is_button());
        assert_eq!(UndoAction::Unavailable.label(), "Undo unavailable");
    }

    #[test]
    fn the_journal_says_whether_an_event_is_undone_and_still_the_head() {
        use todo_core::{State, UndoOptions, apply, decide, ids::ListId, undo::undo_batch};
        let owner = Actor::owner("web").unwrap();
        let at = Timestamp::from_second(1_800_000_000).unwrap();
        let mut state = State::new();
        let list = ListId::from_uuid(uuid::Uuid::from_u128(9));
        let create = crate::intent::create_list(list, None, "Errands");
        let events = decide(&state, &owner, &create, at).unwrap();
        for e in &events {
            apply(&mut state, e).unwrap();
        }
        let original = events[0].id;
        let unseen = EventId::from_uuid(uuid::Uuid::from_u128(1));
        assert_eq!(confirmed(&state.journal, unseen, false), None);
        assert_eq!(
            confirmed(&state.journal, original, false),
            Some((false, true))
        );
        let undo = undo_batch(&state, &events, &owner, at, UndoOptions::default()).unwrap();
        for e in &undo {
            apply(&mut state, e).unwrap();
        }
        let compensation = undo[0].id;
        assert_eq!(
            confirmed(&state.journal, original, false),
            Some((true, false))
        );
        assert_eq!(
            confirmed(&state.journal, compensation, true),
            Some((false, true))
        );
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
