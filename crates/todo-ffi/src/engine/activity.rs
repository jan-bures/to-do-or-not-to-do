//! A room owns its pages; the paired engine owns its undo tracker, across rooms and filters.
use super::*;
use todo_client::Epoch;
use todo_client::activity::{
    self, ActivityError, ActivityPage, ActivityQuery, Entry, Feed, Loaded, UndoAction, UndoOutcome,
    Undos,
};
use todo_core::actor::AgentId;

#[derive(Debug, uniffi::Record)]
pub struct ActivityRow {
    pub id: String,
    pub seq: u64,
    pub summary: String,
    pub actor: String,
    pub relative: String,
    pub note: Option<String>,
    pub undo_label: Option<String>,
}
#[derive(Debug, uniffi::Record)]
pub struct ActivityDay {
    pub heading: String,
    pub rows: Vec<ActivityRow>,
    /// Whether this group is on the owner's current local date.
    pub is_today: bool,
}
#[derive(Debug, uniffi::Record)]
pub struct ActivityView {
    pub agent: Option<String>,
    pub days: Vec<ActivityDay>,
    /// Changes only when the pager discards its pages (not when it first learns its epoch).
    pub reset_count: u64,
    pub refresh_label: String,
    pub refreshing: bool,
    pub footer: ActivityFooter,
}

#[derive(Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ActivityFooter {
    None,
    Loading { label: String },
    Older { cursor: u64 },
    Error { message: String, action: String },
    Empty { label: String },
}

impl ActivityFooter {
    fn for_feed(feed: &Feed, filtered: bool) -> Self {
        if feed.pending().is_some() {
            Self::Loading {
                label: activity::LOADING.into(),
            }
        } else if let Some(error) = feed.error() {
            Self::Error {
                message: error.to_string(),
                action: activity::retry_label(error).into(),
            }
        } else if let Some(cursor) = feed.older() {
            Self::Older { cursor }
        } else if feed.is_empty_feed() {
            Self::Empty {
                label: activity::empty_line(filtered).into(),
            }
        } else {
            Self::None
        }
    }
}

#[derive(Debug, Clone, Copy, uniffi::Enum)]
pub enum ActivityLoad {
    Initial,
    Refresh,
    Older,
    Retry,
}

#[derive(Debug, uniffi::Record)]
pub struct ActivityRequest {
    pub number: u64,
    pub before: Option<u64>,
    /// Confirmed cursor at the start of the read, not when its reply arrived.
    pub cursor: u64,
}

#[derive(Debug, Default)]
struct Pages {
    feed: Feed,
    epoch: Option<Epoch>,
    reset_count: u64,
    read_at: Option<u64>,
}
impl Pages {
    fn observe(&mut self, epoch: Option<Epoch>) {
        if self.epoch.is_some() && self.epoch != epoch {
            self.feed.reset();
            self.reset_count += 1;
            self.read_at = None;
        }
        self.epoch = epoch;
    }
    fn begin(&mut self, load: ActivityLoad, cursor: u64) -> Option<ActivityRequest> {
        let before = match load {
            ActivityLoad::Initial
                if self.feed.loaded()
                    || self.feed.error().is_some()
                    || self.feed.pending().is_some() =>
            {
                return None;
            }
            ActivityLoad::Initial | ActivityLoad::Refresh => None,
            ActivityLoad::Older => Some(self.feed.older()?),
            ActivityLoad::Retry => self.feed.retry()?,
        };
        Some(ActivityRequest {
            number: self.feed.begin(before)?,
            before,
            cursor,
        })
    }
    fn finish(
        &mut self,
        request: ActivityRequest,
        result: Result<ActivityPage, ActivityError>,
    ) -> Loaded {
        // A skip may make an older page the first successful read.
        let baseline = result.is_ok() && (request.before.is_none() || self.read_at.is_none());
        let loaded = self.feed.finish(request.number, request.before, result);
        if baseline && loaded == Loaded::Applied {
            self.read_at = Some(request.cursor);
        }
        loaded
    }
}

/// A replacement room has its own pager; a read does not hold its lock during network I/O.
#[derive(Debug, uniffi::Object)]
pub struct ActivityPager {
    agent: Option<String>,
    pages: Mutex<Pages>,
}
#[uniffi::export]
impl ActivityPager {
    #[uniffi::constructor]
    pub fn new(agent: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            agent,
            pages: Mutex::default(),
        })
    }
}

impl TodoEngine {
    fn activity_pages<'a>(
        &self,
        pager: &'a ActivityPager,
    ) -> (std::sync::MutexGuard<'a, Pages>, u64) {
        let mut pages = locked(&pager.pages);
        let (epoch, cursor) = self.client.activity_revision();
        pages.observe(epoch);
        (pages, cursor)
    }

    fn row_action(&self, e: &Entry, undos: &Undos) -> UndoAction {
        activity::undo_action(
            e,
            self.client.activity_confirmed(e.target.id, e.compensating),
            self.client.pending_undo(e.target.id).is_some(),
            undos.group_outcome(e.group),
        )
    }

    /// After a restart the outbox knows the target but not its batch. A sibling alone cannot
    /// recover batch protection: the feed must have seen the target row in this session.
    fn observe_undos(&self, feed: &Feed, undos: &mut Undos) {
        undos.settle(|id| self.client.undo_result(id));
        // A failed outbox entry can outlive a successful retry. Only settle the command tracked
        // for this attempt; never import old refusals from the durable failure list.
        for e in feed.entries() {
            if let Some(command) = self.client.pending_undo(e.target.id) {
                undos.record(e.target.id, e.group, UndoOutcome::Pending(command));
            }
        }
    }
}

#[uniffi::export]
impl TodoEngine {
    pub fn begin_activity(
        &self,
        pager: &ActivityPager,
        load: ActivityLoad,
    ) -> Option<ActivityRequest> {
        let (mut pages, cursor) = self.activity_pages(pager);
        pages.begin(load, cursor)
    }

    /// Completes one read, returning the refresh (if any) queued behind it.
    pub fn read_activity(
        &self,
        pager: &ActivityPager,
        request: ActivityRequest,
    ) -> Option<ActivityRequest> {
        let result = (|| {
            let agent = pager
                .agent
                .clone()
                .map(AgentId::new)
                .transpose()
                .map_err(|_| ActivityError::Unavailable)?;
            runtime()
                .map_err(|_| ActivityError::Unavailable)?
                .block_on(self.client.activity(&ActivityQuery {
                    agent,
                    before: request.before,
                    limit: None,
                }))
        })();
        let (mut pages, cursor) = self.activity_pages(pager);
        if pages.finish(request, result) == Loaded::Refresh {
            pages.begin(ActivityLoad::Refresh, cursor)
        } else {
            None
        }
    }

    pub fn activity_view(&self, pager: &ActivityPager, now: LocalNow) -> ActivityView {
        let now = Now::new(now);
        let (pages, cursor) = self.activity_pages(pager);
        let feed = &pages.feed;
        let mut undos = locked(&self.activity_undos);
        self.observe_undos(feed, &mut undos);
        let days = feed
            .days(|at| now.date_of(at))
            .into_iter()
            .map(|(day, seqs)| ActivityDay {
                heading: activity::day_label(day, now.today()),
                is_today: day == now.today(),
                rows: seqs
                    .into_iter()
                    .filter_map(|seq| feed.entry(seq))
                    .map(|e| {
                        let action = self.row_action(e, &undos);
                        let note = match action {
                            UndoAction::Undo => undos.refusal(e.target.id),
                            UndoAction::Undoing => Some(activity::UNDOING),
                            _ => Some(action.label()),
                        };
                        ActivityRow {
                            id: e.target.id.to_string(),
                            seq: e.target.seq,
                            summary: e.what.clone(),
                            actor: e.actor_name(),
                            relative: activity::relative_time(e.at, now.at),
                            note: note.map(str::to_owned),
                            undo_label: (action == UndoAction::Undo).then(|| action.label().into()),
                        }
                    })
                    .collect(),
            })
            .collect();
        ActivityView {
            agent: pager.agent.as_deref().map(activity::agent_name),
            days,
            reset_count: pages.reset_count,
            refresh_label: if pages.read_at.is_some_and(|at| at != cursor) {
                format!("{} •", activity::REFRESH)
            } else {
                activity::REFRESH.into()
            },
            refreshing: feed.refreshing(),
            footer: ActivityFooter::for_feed(feed, pager.agent.is_some()),
        }
    }

    pub fn undo_activity_event(
        &self,
        pager: &ActivityPager,
        seq: u64,
        id: String,
    ) -> Result<(), FfiError> {
        let (pages, _) = self.activity_pages(pager);
        let Some(e) = pages
            .feed
            .entry(seq)
            .filter(|e| e.target.id.to_string() == id)
        else {
            return Ok(());
        };
        let mut undos = locked(&self.activity_undos);
        self.observe_undos(&pages.feed, &mut undos);
        if self.row_action(e, &undos) == UndoAction::Undo {
            let outcome = self.client.undo_activity(e.target).map_or_else(
                |_| UndoOutcome::Refused(todo_client::failure::undo_message("")),
                UndoOutcome::Pending,
            );
            let message = undos.record(e.target.id, e.group, outcome);
            if matches!(outcome, UndoOutcome::Refused(_)) {
                return Err(invalid(message));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(seq: u64) -> ActivityPage {
        let id = uuid::Uuid::from_u128(u128::from(seq));
        serde_json::from_value(serde_json::json!({"items": [{
            "event": {"id": id, "seq": seq, "group": uuid::Uuid::nil(), "command_id": uuid::Uuid::nil(),
                "actor": {"type": "agent", "agent_id": "gardener"}, "at": "2025-11-20T23:30:00Z", "forced": false,
                "payload": {"op": "updated", "target": {"kind": "task", "id": id}, "changes": []}},
            "summary": "Edited a task", "undone": false, "can_undo": true
        }], "next_before": seq})).expect("activity page")
    }

    #[test]
    fn replacing_a_filter_isolates_a_late_reply_and_keeps_the_batch_blocked() {
        let now = LocalNow {
            epoch_ms: "2025-11-22T01:00:00Z"
                .parse::<Timestamp>()
                .unwrap()
                .as_millisecond(),
            utc_offset_secs: 3600,
            zone_id: "Europe/Prague".into(),
            local_day: None,
        };
        let e = TodoEngine::fixture("blank".into(), now.clone()).unwrap();
        let all = ActivityPager::new(None);
        let first = e.begin_activity(&all, ActivityLoad::Initial).unwrap();
        locked(&all.pages).finish(first, Ok(page(7)));
        let view = e.activity_view(&all, now.clone());
        assert_eq!(
            view.days[0].heading, "Yesterday",
            "date at the phone's offset"
        );
        let target = &view.days[0].rows[0];
        e.undo_activity_event(&all, target.seq, target.id.clone())
            .unwrap();
        let late = e.begin_activity(&all, ActivityLoad::Older).unwrap();

        let filtered = ActivityPager::new(Some("gardener".into()));
        let current = e.begin_activity(&filtered, ActivityLoad::Initial).unwrap();
        // The loop keeps its own pager. Its late reply can't finish the replacement's request.
        assert_ne!(current.number, late.number);
        assert_eq!(
            locked(&all.pages).finish(late, Ok(page(1))),
            Loaded::Applied
        );
        assert!(matches!(
            e.activity_view(&filtered, now.clone()).footer,
            ActivityFooter::Loading { .. }
        ));
        // Only the sibling is on the replacement page. Looking up its own pending undo would miss.
        assert_eq!(
            locked(&filtered.pages).finish(current, Ok(page(6))),
            Loaded::Applied
        );
        let view = e.activity_view(&filtered, now);
        assert_eq!(view.days[0].rows.len(), 1);
        let sibling = &view.days[0].rows[0];
        assert_eq!(sibling.seq, 6);
        assert_eq!(sibling.note.as_deref(), Some(activity::UNDOING));
        assert!(sibling.undo_label.is_none());
        e.undo_activity_event(&filtered, sibling.seq, sibling.id.clone())
            .unwrap();
        assert_eq!(e.status().pending, 1);
    }

    #[test]
    fn a_recorded_refusal_never_hides_a_later_row_state() {
        let now = LocalNow {
            epoch_ms: 0,
            utc_offset_secs: 0,
            zone_id: "UTC".into(),
            local_day: None,
        };
        let engine = TodoEngine::fixture("blank".into(), now.clone()).unwrap();
        let pager = ActivityPager::new(None);
        for (undone, expected) in [(true, UndoAction::Undone), (false, UndoAction::Unavailable)] {
            let mut page = page(7);
            let item = &mut page.items[0];
            item.undone = undone;
            item.can_undo = false;
            let event = &item.event.event;
            locked(&engine.activity_undos).record(
                event.id,
                event.group,
                UndoOutcome::Refused("Earlier refusal"),
            );
            let request = engine
                .begin_activity(&pager, ActivityLoad::Refresh)
                .unwrap();
            locked(&pager.pages).finish(request, Ok(page));
            let view = engine.activity_view(&pager, now.clone());
            assert_eq!(view.days[0].rows[0].note.as_deref(), Some(expected.label()));
        }
    }

    #[test]
    fn epoch_change_drops_pages_and_invalidates_an_in_flight_read() {
        let mut pages = Pages::default();
        let first = pages.begin(ActivityLoad::Initial, 7).unwrap();
        pages.observe(Some(uuid::Uuid::nil()));
        assert_eq!(
            pages.reset_count, 0,
            "learning the epoch preserves the read and scroll"
        );
        assert_eq!(pages.finish(first, Ok(page(7))), Loaded::Applied);
        let older = pages.begin(ActivityLoad::Older, 8).unwrap();
        pages.observe(Some(uuid::Uuid::from_u128(42)));
        assert_eq!(pages.finish(older, Ok(page(1))), Loaded::Ignore);
        assert_eq!(pages.reset_count, 1);
        assert!(!pages.feed.loaded());
        assert!(pages.read_at.is_none());
    }
}
