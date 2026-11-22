use super::*;
use crate::intent::create_list;
use crate::intent::organize::create_collection;
use proptest::prelude::*;
use todo_core::actor::Actor;
use todo_core::command::NewTask;
use todo_core::model::Priority;
use todo_core::{Command, apply, decide};
use uuid::Uuid;

/// The owner's state, built with real commands and then adjusted by hand where a test needs a time.
struct World {
    state: State,
    next: u64,
}

const CREATED: &str = "2026-11-01T08:00:00Z";

fn ts(s: &str) -> Timestamp {
    s.parse().expect("timestamp")
}

fn utc(at: &str) -> LocalTime {
    LocalTime::new(ts(at), TimeZone::UTC)
}

/// Prague: CET in winter, CEST in summer.
fn prague(at: &str) -> LocalTime {
    LocalTime::new(
        ts(at),
        TimeZone::posix("CET-1CEST,M3.5.0,M10.5.0/3").expect("posix zone"),
    )
}

impl World {
    fn new() -> Self {
        Self {
            state: State::new(),
            next: 1,
        }
    }

    fn id(&mut self) -> Uuid {
        self.next += 1;
        todo_core::ids::uuid_v7(1_790_000_000_000 + self.next, [self.next as u8; 10])
    }

    fn run(&mut self, cmd: Command) {
        let actor = Actor::owner("test").expect("actor");
        let events = decide(&self.state, &actor, &cmd, ts(CREATED)).expect("valid");
        for e in &events {
            apply(&mut self.state, e).expect("applies");
        }
    }

    fn collection(&mut self, name: &str) -> CollectionId {
        let id = CollectionId::from_uuid(self.id());
        self.run(create_collection(id, name));
        id
    }

    fn list(&mut self, name: &str, collection: Option<CollectionId>) -> ListId {
        let id = ListId::from_uuid(self.id());
        self.run(create_list(id, collection, name));
        id
    }

    fn task(&mut self, list: ListId, title: &str) -> TaskId {
        let id = TaskId::from_uuid(self.id());
        self.run(Command::CreateTask(Box::new(NewTask::new(id, list, title))));
        id
    }

    fn task_mut(&mut self, id: TaskId) -> &mut Task {
        self.state.tasks.get_mut(&id).expect("task")
    }

    fn done_at(&mut self, id: TaskId, at: &str) {
        self.task_mut(id).completed_at = Some(ts(at));
    }

    fn important(&mut self, id: TaskId) {
        self.task_mut(id).priority = Priority::High;
    }

    fn header(&self, key: RoomKey<'_>, now: &LocalTime) -> RoomHeader {
        room_header(&self.state, key, &BTreeSet::new(), now)
    }
}

fn day(s: &str) -> Date {
    s.parse().expect("date")
}

#[test]
fn a_list_measures_today_and_not_its_old_completions() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    for i in 0..3 {
        w.task(list, &format!("open {i}"));
    }
    let today = w.task(list, "done this morning");
    w.done_at(today, "2026-11-21T07:00:00Z");
    for i in 0..450 {
        let old = w.task(list, &format!("old {i}"));
        w.done_at(old, "2026-11-02T12:00:00Z");
    }
    let h = w.header(RoomKey::List(list), &utc("2026-11-21T10:35:00Z"));
    assert_eq!((h.progress_done, h.progress_total), (1, 4));
    assert_eq!(h.summary.text(), "1 of 4 done · 3 open");
    assert_eq!(h.eyebrow.text(), "List · 3 open");
    assert_eq!(
        h.status.map(|s| s.text()).as_deref(),
        Some("3 open · 1 done today")
    );
    assert_eq!(h.finished_this_month, 451);
    assert_eq!(h.empty_line(false), "Nothing here yet.");
    assert_eq!(h.empty_line(true), "Nothing matches the filter.");
    assert!(!h.finished_day());
}

/// The owner's report: Important with two open tasks and nineteen finished on earlier days.
#[test]
fn important_counts_today_and_shows_the_day() {
    let mut w = World::new();
    let list = w.list("Work", None);
    for i in 0..2 {
        let t = w.task(list, &format!("open {i}"));
        w.important(t);
    }
    for i in 0..19 {
        let t = w.task(list, &format!("done {i}"));
        w.important(t);
        w.done_at(t, "2026-11-20T16:00:00Z");
    }
    let h = w.header(
        RoomKey::Smart(SmartList::Important),
        &utc("2026-11-22T10:00:00Z"),
    );
    assert_eq!(h.eyebrow.text(), "Sunday · 22 Nov 2026 · W47");
    assert_eq!(h.summary.text(), "0 of 2 done · 2 open");
    assert_eq!(
        h.status.map(|s| s.text()).as_deref(),
        Some("2 open · 0 done today")
    );
    assert_eq!(h.empty_line(false), "Nothing pressing.");
}

#[test]
fn a_collection_counts_its_lists_and_their_work() {
    let mut w = World::new();
    let home = w.collection("Home\nand garden");
    let a = w.list("Garden", Some(home));
    let b = w.list("Kitchen", Some(home));
    let elsewhere = w.list("Work", None);
    w.task(a, "Prune");
    let t = w.task(b, "Descale");
    w.done_at(t, "2026-11-21T09:00:00Z");
    w.task(elsewhere, "Not here");
    let h = w.header(RoomKey::Collection(home), &utc("2026-11-21T10:35:00Z"));
    assert_eq!(h.eyebrow.text(), "Collection · 2 lists");
    assert_eq!(h.summary.text(), "1 of 2 done · 1 open");
    // A list in it names it, on one line.
    let h = w.header(RoomKey::List(a), &utc("2026-11-21T10:35:00Z"));
    assert_eq!(h.eyebrow.text(), "Home and garden · 1 open");

    let single = w.collection("Solo");
    w.list("Only", Some(single));
    let h = w.header(RoomKey::Collection(single), &utc("2026-11-21T10:35:00Z"));
    assert_eq!(h.eyebrow.text(), "Collection · 1 list");
    assert_eq!(
        h.summary,
        Summary::Empty {
            finished_this_month: 0
        }
    );
    assert_eq!(h.summary.text(), "nothing yet");
}

#[test]
fn an_empty_room_says_what_it_finished_this_month() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    for at in [
        "2026-11-03T10:00:00Z",
        "2026-11-12T10:00:00Z",
        "2026-10-30T10:00:00Z",
    ] {
        let t = w.task(list, "done");
        w.done_at(t, at);
    }
    let h = w.header(RoomKey::List(list), &utc("2026-11-21T10:35:00Z"));
    assert_eq!((h.progress_done, h.progress_total), (0, 0));
    assert_eq!(h.summary.text(), "2 finished this month");
    assert_eq!(
        h.status.map(|s| s.text()).as_deref(),
        Some("0 open · 0 done today")
    );
}

#[test]
fn today_starts_at_local_midnight_across_daylight_saving() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    w.task(list, "open");
    // Spring forward on 29 March: 00:30 CET (23:30 UTC the day before) is today; 23:30 CET the day before is
    // not, though at today's offset (CEST, +2) it would read as 00:30.
    let after = w.task(list, "after midnight");
    w.done_at(after, "2026-03-28T23:30:00Z");
    let before = w.task(list, "before midnight");
    w.done_at(before, "2026-03-28T22:30:00Z");
    let h = w.header(RoomKey::List(list), &prague("2026-03-29T08:00:00Z"));
    assert_eq!((h.progress_done, h.progress_total), (1, 2));
    assert_eq!(h.done_today, 1);

    // Fall back on 25 October: 00:30 CEST (22:30 UTC the day before) is today, though at today's offset
    // (CET, +1) it would read as 23:30 the day before.
    let mut w = World::new();
    let list = w.list("Errands", None);
    w.task(list, "open");
    let after = w.task(list, "after midnight");
    w.done_at(after, "2026-10-24T22:30:00Z");
    let before = w.task(list, "before midnight");
    w.done_at(before, "2026-10-24T21:30:00Z");
    let now = prague("2026-10-25T12:00:00Z");
    assert_eq!(now.today(), day("2026-10-25"));
    let h = w.header(RoomKey::List(list), &now);
    assert_eq!((h.progress_done, h.progress_total), (1, 2));
    assert_eq!(h.eyebrow.text(), "List · 1 open");
}

#[test]
fn a_held_task_counts_as_open_until_it_leaves() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    let a = w.task(list, "a");
    let b = w.task(list, "b");
    w.done_at(b, "2026-11-21T08:00:00Z");
    let now = utc("2026-11-21T10:35:00Z");
    let before = w.header(RoomKey::List(list), &now);
    assert_eq!(before.summary.text(), "1 of 2 done · 1 open");

    w.done_at(a, "2026-11-21T10:35:00Z");
    let holding = BTreeSet::from([a]);
    let held = room_header(&w.state, RoomKey::List(list), &holding, &now);
    assert_eq!(held, before);
    // It stays in the rows where it was, and out of the Completed section.
    let tasks = room_tasks(&w.state, RoomKey::List(list), &holding, &now.eval());
    assert_eq!(tasks.open.iter().map(|t| t.id).collect::<Vec<_>>(), vec![a]);
    assert_eq!(tasks.done.iter().map(|t| t.id).collect::<Vec<_>>(), vec![b]);

    let released = w.header(RoomKey::List(list), &now);
    assert_eq!(released.summary.text(), "2 of 2 done · 0 open");

    // In a smart list too: it stays among the rows that match it as if it were open.
    w.important(a);
    let tasks = room_tasks(
        &w.state,
        RoomKey::Smart(SmartList::Important),
        &holding,
        &now.eval(),
    );
    assert_eq!(tasks.open.len(), 1);
    assert!(tasks.done.is_empty());
    let h = room_header(
        &w.state,
        RoomKey::Smart(SmartList::Important),
        &holding,
        &now,
    );
    assert_eq!(h.summary.text(), "0 of 1 done · 1 open");
}

#[test]
fn my_day_counts_its_whole_plan_and_finishes_the_day() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    let today = day("2026-11-21");
    let now = utc("2026-11-21T10:35:00Z");
    let mut plan = Vec::new();
    for i in 0..3 {
        let t = w.task(list, &format!("plan {i}"));
        w.task_mut(t).my_day = Some(today);
        plan.push(t);
    }
    // Planned today but finished just before midnight: it counts in the plan, not in "done today".
    w.done_at(plan[0], "2026-11-20T23:50:00Z");
    w.task(list, "not planned");
    let h = w.header(RoomKey::Smart(SmartList::MyDay), &now);
    assert_eq!(h.summary.text(), "1 of 3 done · 2 open");
    assert_eq!(h.done_today, 0);
    assert_eq!(
        h.status.map(|s| s.text()).as_deref(),
        Some("2 open · 0 done today")
    );
    assert_eq!(h.eyebrow.text(), "Saturday · 21 Nov 2026 · W47");
    assert!(h.finishes_day && !h.finished_day());

    w.done_at(plan[1], "2026-11-21T09:00:00Z");
    w.done_at(plan[2], "2026-11-21T10:00:00Z");
    let held = room_header(
        &w.state,
        RoomKey::Smart(SmartList::MyDay),
        &BTreeSet::from([plan[2]]),
        &now,
    );
    assert_eq!(held.summary.text(), "2 of 3 done · 1 open");
    assert!(!held.finished_day());
    let h = w.header(RoomKey::Smart(SmartList::MyDay), &now);
    assert_eq!(h.summary, Summary::AllClear { done: 3, total: 3 });
    assert_eq!(h.summary.text(), "3 of 3 done · all clear");
    assert!(h.finished_day());
    assert_eq!(h.empty_line(false), "Nothing planned yet.");
}

#[test]
fn completed_counts_every_finished_task() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    let now = utc("2026-11-21T10:35:00Z");
    let h = w.header(RoomKey::Smart(SmartList::Completed), &now);
    assert_eq!(h.summary.text(), "nothing yet");
    assert_eq!(h.empty_line(false), "Nothing finished yet.");

    w.task(list, "open");
    let old = w.task(list, "old");
    w.done_at(old, "2025-01-01T10:00:00Z");
    let new = w.task(list, "new");
    w.done_at(new, "2026-11-21T10:00:00Z");
    let held = w.task(list, "held");
    w.done_at(held, "2026-11-21T10:35:00Z");
    let key = RoomKey::Smart(SmartList::Completed);
    let holding = BTreeSet::from([held]);
    let ids = |holding| {
        room_tasks(&w.state, key, holding, &now.eval())
            .open
            .iter()
            .map(|t| t.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&holding), vec![new, old]);
    assert_eq!(ids(&BTreeSet::new()), vec![held, new, old]);
    let h = room_header(&w.state, key, &holding, &now);
    assert_eq!(h.summary, Summary::Done { done: 2 });
    assert_eq!(h.summary.text(), "2 done");
    assert_eq!((h.progress_done, h.progress_total), (2, 2));
    assert_eq!(
        h.status.map(|s| s.text()).as_deref(),
        Some("0 open · 1 done today")
    );
}

#[test]
fn agent_activity_measures_what_agents_changed_this_week() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    let agent = Actor::agent("claude").expect("agent");
    let mut changed = Vec::new();
    for i in 0..3 {
        let t = w.task(list, &format!("agent {i}"));
        w.task_mut(t).updated = todo_core::model::Stamp {
            at: ts("2026-11-19T10:00:00Z"),
            by: agent.clone(),
        };
        changed.push(t);
    }
    // Finished on an earlier day: Agent activity still counts it done.
    w.done_at(changed[0], "2026-11-19T11:00:00Z");
    w.task(list, "the owner's");
    let h = w.header(
        RoomKey::Smart(SmartList::AgentActivity),
        &utc("2026-11-21T10:35:00Z"),
    );
    assert_eq!(h.summary.text(), "1 of 3 done · 2 open");
    assert_eq!(h.done_today, 0);
    assert_eq!(h.empty_line(false), "No agent changes this week.");
}

#[test]
fn activity_keeps_no_score() {
    let mut w = World::new();
    let list = w.list("Errands", None);
    w.task(list, "open");
    let now = utc("2026-11-21T10:35:00Z");
    let h = w.header(RoomKey::Activity(None), &now);
    assert_eq!(h.summary, Summary::None);
    assert_eq!(h.summary.text(), "");
    assert_eq!(h.status, None);
    assert_eq!((h.progress_done, h.progress_total), (0, 0));
    assert_eq!(h.eyebrow.text(), "History · All changes");
    assert_eq!(h.empty_line(false), "No activity yet.");
    let h = w.header(RoomKey::Activity(Some("claude")), &now);
    assert_eq!(h.eyebrow.text(), "History · Agent: Claude");
    assert_eq!(h.empty_line(false), "No activity from this agent.");
    assert_eq!(StatusCounts::activity(12, 3).text(), "12 shown · 3 today");
}

#[test]
fn smart_lists_round_trip_through_their_ids() {
    for s in SmartList::ALL {
        assert_eq!(SmartList::from_id(s.id()), Some(s));
        assert_eq!(s.builtin().id, s.id());
    }
    assert_eq!(SmartList::from_id("nope"), None);
}

/// One task of a generated state.
#[derive(Clone, Debug)]
struct Gen {
    list: usize,
    my_day: bool,
    important: bool,
    /// Days since a change by an agent (`None`: owner).
    changed_days_ago: Option<i64>,
    due_in: Option<i64>,
    position: bool,
    keep: bool,
    /// Days before today it was completed (`None`: open).
    done_days_ago: Option<i64>,
    held: bool,
    deleted: bool,
}

fn gen_task() -> impl Strategy<Value = Gen> {
    (
        0..3usize,
        any::<bool>(),
        any::<bool>(),
        prop::option::of(0..10i64),
        prop::option::of(0..4i64),
        any::<bool>(),
        any::<bool>(),
        prop::option::of(0..40i64),
        any::<bool>(),
        prop::bool::weighted(0.1),
    )
        .prop_map(
            |(
                list,
                my_day,
                important,
                changed_days_ago,
                due_in,
                position,
                keep,
                done_days_ago,
                held,
                deleted,
            )| Gen {
                list,
                my_day,
                important,
                changed_days_ago,
                due_in,
                position,
                keep,
                done_days_ago,
                held,
                deleted,
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn room_sections_match_the_plan_in_order(tasks in prop::collection::vec(gen_task(), 1..24)) {
        let mut w = World::new();
        let col = w.collection("Home");
        let lists = [w.list("A", Some(col)), w.list("B", Some(col)), w.list("C", None)];
        let now = prague("2026-11-21T10:35:00Z");
        let today = now.today();
        // Restoring a merge source clears its tombstone, but not the merge link. It must stay
        // out of both the primary rows and the drawer even though State::is_live sees it.
        let target = w.task(lists[0], "merge target");
        for completed in [false, true] {
            let source = w.task(lists[0], "keep merge source");
            w.task_mut(source).my_day = Some(today);
            if completed {
                w.done_at(source, "2026-11-21T09:00:00Z");
            }
            w.run(Command::MergeTasks { source, target });
            w.run(Command::Restore { id: source.into() });
            prop_assert!(w.state.is_live(source.into()));
            prop_assert_eq!(w.task_mut(source).merged_into, Some(target));
        }
        w.run(Command::Delete { id: target.into() });
        let mut holding = BTreeSet::new();
        let mut generated = Vec::new();
        for g in &tasks {
            let id = w.task(lists[g.list], if g.keep { "keep" } else { "omit" });
            generated.push((id, g));
            let t = w.task_mut(id);
            t.my_day = g.my_day.then_some(today);
            t.due = g.due_in.and_then(|d| todo_core::time::add_days(today, d));
            // Two positions, so task-ID tie-breaks are common.
            t.position = todo_core::ordering::PosKey::new(if g.position { "b" } else { "a" }).unwrap();
            if g.important {
                t.priority = Priority::High;
            }
            if let Some(days) = g.changed_days_ago {
                t.updated = todo_core::model::Stamp {
                    at: now.at - jiff::SignedDuration::from_hours(days * 24),
                    by: Actor::agent("claude").expect("agent"),
                };
            }
            if let Some(days) = g.done_days_ago {
                t.completed_at = Some(now.at - jiff::SignedDuration::from_hours(days * 24 + 1));
            }
            if g.deleted {
                t.deleted = Some(todo_core::model::Tombstone {
                    at: now.at,
                    by: Actor::owner("test").expect("actor"),
                });
            }
            if g.held && g.done_days_ago.is_some() {
                holding.insert(id);
            }
        }
        // Tied list positions must not interleave the collection's lists.
        let pos = w.state.lists[&lists[0]].position.clone();
        w.state.lists.get_mut(&lists[1]).unwrap().position = pos;
        let mut keys: Vec<RoomKey<'_>> = SmartList::ALL.into_iter().map(RoomKey::Smart).collect();
        keys.extend(lists.iter().map(|l| RoomKey::List(*l)));
        keys.push(RoomKey::Collection(col));
        keys.push(RoomKey::Activity(None));
        let eval = now.eval();
        let query = Query::parse("keep").unwrap();
        let filter = query.matcher(&w.state, &eval);
        for released in [false, true] {
            let no_holds = BTreeSet::new();
            let holds = if released { &no_holds } else { &holding };
            for &key in &keys {
                use SmartList::*;
                let mut rows = Vec::new();
                let mut drawer = Vec::new();
                let (mut open, mut done, mut done_today) = (0, 0, 0);
                for &(id, g) in &generated {
                    let done_days = g.done_days_ago.filter(|_| released || !g.held);
                    let matches = !g.deleted && match key {
                        RoomKey::Smart(MyDay) => g.my_day,
                        RoomKey::Smart(Important) => g.important,
                        RoomKey::Smart(Planned) => g.due_in.is_some(),
                        RoomKey::Smart(All) => true,
                        RoomKey::Smart(Completed) => done_days.is_some(),
                        RoomKey::Smart(AgentActivity) => g.changed_days_ago.is_some_and(|d| d <= 7),
                        RoomKey::Collection(_) => g.list < 2,
                        RoomKey::List(l) => lists[g.list] == l,
                        RoomKey::Activity(_) => false,
                    };
                    if !matches { continue; }
                    done += usize::from(done_days.is_some());
                    done_today += usize::from(done_days == Some(0));
                    open += usize::from(done_days.is_none());
                    if done_days.is_some() && !matches!(key, RoomKey::Smart(Completed | AgentActivity)) {
                        drawer.push((id, g));
                    } else {
                        rows.push((id, g));
                    }
                }
                // Uncollected lists precede collections in smart lists. List ID breaks the tied
                // list positions before the row's position; task ID follows the row's position.
                let manual = |g: &Gen| (if g.list == 2 { 0 } else { g.list + 1 }, g.position);
                rows.sort_by_key(|&(id, g)| match key {
                    RoomKey::Smart(Important | Planned) => (g.due_in.unwrap_or(i64::MAX), manual(g), id),
                    RoomKey::Smart(Completed) => (g.done_days_ago.unwrap(), manual(g), id),
                    RoomKey::Smart(AgentActivity) => (g.changed_days_ago.unwrap(), manual(g), id),
                    _ => (0, manual(g), id),
                });
                drawer.sort_by_key(|&(id, g)| (g.done_days_ago.unwrap(), matches!(key, RoomKey::Smart(_)).then(|| manual(g)), id));
                let actual = room_tasks(&w.state, key, holds, &eval);
                prop_assert_eq!(actual.open.iter().map(|t| t.id).collect::<Vec<_>>(), rows.iter().map(|&(id, _)| id).collect::<Vec<_>>(), "{:?}", key);
                prop_assert_eq!(actual.done.iter().map(|t| t.id).collect::<Vec<_>>(), drawer.iter().map(|&(id, _)| id).collect::<Vec<_>>(), "{:?}", key);
                prop_assert_eq!(actual.done.iter().filter(|t| filter.matches(t)).map(|t| t.id).collect::<Vec<_>>(), drawer.iter().filter(|(_, g)| g.keep).map(|&(id, _)| id).collect::<Vec<_>>());
                let h = header_of(&w.state, key, &actual, holds, &now);
                let expected_done = match key {
                    RoomKey::Smart(MyDay | AgentActivity | Completed) => done,
                    _ => done_today,
                };
                prop_assert_eq!((h.progress_done, h.progress_total), (expected_done, open + expected_done));
                prop_assert_eq!(h.status, (!key.is_activity()).then(|| StatusCounts::tasks(open, done_today)));
                if released && key == RoomKey::Smart(MyDay) {
                    prop_assert_eq!(my_day_progress(&w.state, today), (done, open + done));
                }
            }
        }
    }
}

#[test]
fn the_day_eyebrow_reads_naturally() {
    assert_eq!(
        day_eyebrow(day("2026-11-21")).text(),
        "Saturday · 21 Nov 2026 · W47"
    );
    assert_eq!(iso_week(day("2021-01-01")), "W53");
    let e = day_eyebrow(day("2027-01-04"));
    assert_eq!(e.lead(), "Monday");
    assert_eq!(e.details(), ["4 Jan 2027", "W1"]);
}
