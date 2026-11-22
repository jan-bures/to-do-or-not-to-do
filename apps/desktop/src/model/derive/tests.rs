use super::*;
use crate::fixtures;
use todo_core::{Command, apply, decide};

fn run(state: &mut State, at: &str, cmd: Command) {
    let actor = Actor::owner("desktop").expect("actor");
    let events = decide(state, &actor, &cmd, at.parse().expect("ts")).expect("valid");
    for e in &events {
        apply(state, e).expect("applies");
    }
}

fn titles(rows: &[TaskRow]) -> Vec<&str> {
    rows.iter().map(|r| r.title.as_ref()).collect()
}

fn list_named(state: &State, name: &str) -> ListId {
    state
        .lists
        .values()
        .find(|l| l.name == name)
        .map(|l| l.id)
        .expect("list")
}

#[test]
fn a_list_room_shows_its_open_tasks_in_order_and_hides_chips() {
    let mut f = fixtures::my_day();
    let office = list_named(&f.state, "Office");
    f.ui = UiState {
        room: RoomKey::List(office),
        ..UiState::default()
    };
    let shell = f.shell();
    assert_eq!(
        titles(&shell.room.tasks),
        [
            "Review the vendor contract",
            "Order printer toner",
            "Archive the old design files"
        ]
    );
    assert_eq!(shell.room.chips, Chips::None);
    assert_eq!(
        shell.room.eyebrow,
        Eyebrow::Kind {
            kind: "Work".into(),
            detail: "3 open".into()
        }
    );
    // Today's work: one done this morning, three open.
    assert_eq!(shell.room.summary(), (1, 4, 3));
    // The Completed section is closed: counted, not listed.
    assert!(shell.room.completed.is_empty());
    assert!(shell.room.completed_count > 1);
    let agent = shell.room.tasks[1].agent;
    assert_eq!(agent, Some(crate::theme::Agent::Pi));
}

#[test]
fn the_completed_section_opens_newest_first() {
    let mut f = fixtures::my_day();
    f.ui.show_completed = true;
    let shell = f.shell();
    assert_eq!(shell.room.completed.len(), 3);
    // Completed at 09:50, 09:05 and 08:15.
    assert_eq!(
        titles(&shell.room.completed),
        [
            "Reply to the landlord",
            "Write the stand-up notes",
            "Water the plants"
        ]
    );
    assert!(shell.room.completed.iter().all(|r| r.done));
    // Rows are open tasks, then the completed ones.
    assert_eq!(shell.row_count(), 12);
}

#[test]
fn a_task_being_completed_keeps_its_place_until_released() {
    let mut f = fixtures::my_day();
    let before = f.shell();
    let vat = before.room.tasks[0].id;
    run(
        &mut f.state,
        fixtures_now(),
        Command::CompleteTask { id: vat },
    );
    // Without holding, it leaves the open rows at once.
    let gone = f.shell();
    assert!(!gone.room.tasks.iter().any(|r| r.id == vat));
    assert_eq!(gone.room.summary(), (4, 12, 8));
    // Held, it stays where it was, shown done, and still counts as open: the counts (and the sun) change
    // when it has left.
    f.ui.holding.insert(vat);
    let held = f.shell();
    assert_eq!(held.room.tasks[0].id, vat);
    assert!(held.room.tasks[0].done);
    assert_eq!(held.room.summary(), before.room.summary());
    assert_eq!(held.room.done_today, before.room.done_today);
    assert_eq!(held.room.completed_count, 3);
    // So does the sidebar's (the next occurrence of this recurring task is already there, though).
    assert_eq!(held.sidebar.smart[0].count, before.sidebar.smart[0].count);
}

#[test]
fn space_can_reach_the_open_detail_after_its_row_leaves() {
    let mut f = fixtures::my_day();
    let task = f.shell().room.tasks[0].id;
    f.ui.task = Some(task);
    run(
        &mut f.state,
        fixtures_now(),
        Command::CompleteTask { id: task },
    );
    let completed = f.shell();
    assert!(!f.ui.show_completed);
    assert_eq!(completed.selected_task, None);
    assert_eq!(completed.detail.as_ref().map(|d| d.task), Some(task));
    assert!(completed.reachable(task, || false, |_| false));
    assert!(!completed.has_row_in(completed.room.key, task));

    let open = completed.room.tasks[0].id;
    f.ui.task = Some(open);
    f.ui.filter = "nonexistent task title".into();
    let filtered = f.shell();
    assert_eq!(filtered.selected_task, None);
    assert!(filtered.reachable(open, || false, |_| false));
    assert!(!filtered.reachable(task, || true, |_| true));
}

#[test]
fn navigation_does_not_act_on_outgoing_rows_or_closing_detail() {
    use super::motion::{Motion, Pace};
    use std::time::Duration;

    let mut f = fixtures::my_day();
    let previous = f.ui.room;
    let id = f.shell().room.tasks[0].id;
    f.ui.task = Some(id);
    let outgoing = f.shell();
    let old_detail = outgoing.detail.as_ref().expect("selected task");
    assert_eq!(outgoing.detail_layer(None), Some((old_detail, true)));
    assert!(outgoing.has_row_in(previous, id));

    // All still has that row, but the old room's row cannot act and the old detail is paint-only.
    let room = RoomKey::Smart(Smart::All);
    f.ui.select_room(&f.state, room);
    assert_eq!(f.ui.task, None);
    let incoming = f.shell();
    assert!(!incoming.has_row_in(previous, id));
    assert!(incoming.has_row_in(room, id));
    assert_eq!(
        incoming.detail_layer(Some(old_detail)),
        Some((old_detail, false))
    );

    // Even with a task selected, Space cannot reach a row before the incoming frame is visible.
    f.ui.task = Some(id);
    let selected = f.shell();
    let mut motion = Motion::default();
    motion.navigation.switch(
        outgoing.room.hue,
        incoming.room.hue,
        Duration::ZERO,
        Pace::FULL,
    );
    let reachable = |at| selected.reachable(id, || motion.row_is_visible(id, at), |_| true);
    assert!(!reachable(Duration::ZERO));
    assert!(reachable(Duration::from_secs(1)));
}

#[test]
fn a_task_being_reopened_keeps_its_completed_place_until_released() {
    let mut f = fixtures::my_day();
    f.ui.show_completed = true;
    let before = f.shell();
    let landlord = before.room.completed[0].id;
    let at = f.state.tasks[&landlord].completed_at.expect("done");
    run(
        &mut f.state,
        fixtures_now(),
        Command::UncompleteTask { id: landlord },
    );
    // Without holding it, it's open at once.
    let open = f.shell();
    assert!(open.room.tasks.iter().any(|r| r.id == landlord));
    assert_eq!(open.room.summary(), (2, 12, 10));
    // Reopening, it stays first in Completed, drawn done, and still counts as done.
    f.ui.reopening.insert(landlord, at);
    let reopening = f.shell();
    assert!(!reopening.room.tasks.iter().any(|r| r.id == landlord));
    assert_eq!(reopening.room.completed[0].id, landlord);
    assert!(reopening.room.completed[0].done);
    assert_eq!(reopening.room.summary(), before.room.summary());
    assert_eq!(reopening.sidebar, before.sidebar);
    // The detail pane tells the truth.
    f.ui.task = Some(landlord);
    assert_eq!(f.shell().detail.map(|d| d.done), Some(false));
}

#[test]
fn the_day_finishes_when_its_last_task_has_left() {
    let mut f = fixtures::my_day();
    let open: Vec<TaskId> = f.shell().room.tasks.iter().map(|r| r.id).collect();
    for id in &open {
        run(
            &mut f.state,
            fixtures_now(),
            Command::CompleteTask { id: *id },
        );
    }
    let last = *open.last().expect("open tasks");
    f.ui.task = None;
    f.ui.holding.insert(last);
    // While the last one plays, the day isn't finished yet: no dusk, no hand-over, the row still shows.
    let held = f.shell();
    assert!(!held.room.finished_day());
    assert!(held.room.handover().is_none() && held.room.plan_only().is_none());
    assert_eq!(
        held.room.tasks.iter().map(|r| r.id).collect::<Vec<_>>(),
        [last]
    );
    let (done, total, open_count) = held.room.summary();
    assert_eq!((done + 1, open_count), (total, 1));
    // Released, it is.
    f.ui.holding.clear();
    assert!(f.shell().room.finished_day());
}

fn fixtures_now() -> &'static str {
    "2026-11-21T10:36:00Z"
}

#[test]
fn the_filter_narrows_rows_with_the_query_language_or_plain_words() {
    let mut f = fixtures::my_day();
    f.ui.filter = "tag:docs".into();
    let shell = f.shell();
    assert_eq!(titles(&shell.room.tasks), ["Write schema migration notes"]);
    assert!(shell.room.filtered);
    // Diacritics and case don't matter.
    f.ui.filter = "MLEKO".into();
    assert_eq!(titles(&f.shell().room.tasks), ["Koupit rohlíky a mléko"]);
    // Text that isn't a valid query still filters by title words.
    f.ui.filter = "(dentist".into();
    assert_eq!(
        titles(&f.shell().room.tasks),
        ["Call the dentist to reschedule the check-up and ask about the bill from March"]
    );
    f.ui.filter = "zzz".into();
    let none = f.shell();
    assert!(none.room.tasks.is_empty());
    assert_eq!(none.room.empty_line.as_ref(), "Nothing matches the filter.");
    assert_eq!(
        none.room.empty_hint.as_ref(),
        "Clear the filter to see every task."
    );
    // The header still counts the whole room.
    assert_eq!(none.room.summary(), (3, 12, 9));
}

#[test]
fn a_deleted_room_falls_back_to_my_day_and_counts_follow_edits() {
    let mut f = fixtures::my_day();
    let errands = list_named(&f.state, "Errands");
    f.ui.room = RoomKey::List(errands);
    f.ui.task = None;
    let shell = f.shell();
    assert_eq!(shell.room.key, RoomKey::List(errands));
    let count_before = shell.sidebar.collections[2].lists[0].count;
    assert_eq!(count_before, 2);
    run(
        &mut f.state,
        fixtures_now(),
        Command::Delete { id: errands.into() },
    );
    let shell = f.shell();
    assert_eq!(shell.room.key, RoomKey::Smart(Smart::MyDay));
    assert_eq!(shell.sidebar.selected, RoomKey::Smart(Smart::MyDay));
    assert!(
        !shell.sidebar.collections[2]
            .lists
            .iter()
            .any(|l| l.id == errands)
    );
    // The milk task left My Day with its list.
    assert_eq!(shell.room.summary().2, 8);
}

#[test]
fn collapsing_hides_lists_but_keeps_the_count_and_current_selection() {
    let mut f = fixtures::my_day();
    let planning = list_named(&f.state, "Planning");
    let work_id = f.state.lists[&planning].collection.expect("Work");
    f.ui.room = RoomKey::List(planning);
    let expanded = f.shell();
    let work = &expanded.sidebar.collections[0];
    assert_eq!(work.id, work_id);
    assert!(!work.collapsed && !work.selected);
    assert!(work.count > 0);
    assert_eq!(work.count, work.lists.iter().map(|l| l.count).sum::<u32>());
    assert!(expanded.detail.is_some());

    f.ui.toggle_collection(work_id);
    let collapsed = f.shell();
    let hidden = &collapsed.sidebar.collections[0];
    assert!(hidden.collapsed && hidden.selected);
    assert!(hidden.lists.is_empty());
    assert_eq!(hidden.count, work.count);
    assert_eq!(
        collapsed.sidebar.collections[1..],
        expanded.sidebar.collections[1..]
    );
    assert_eq!(collapsed.sidebar.selected, RoomKey::List(planning));
    assert_eq!(collapsed.room, expanded.room);
    assert_eq!(collapsed.detail, expanded.detail);

    f.ui.toggle_collection(work_id);
    assert_eq!(f.shell().sidebar, expanded.sidebar);
}

#[test]
fn navigating_to_a_list_reveals_only_its_own_collection() {
    let mut f = fixtures::my_day();
    let all = f.shell().sidebar.collections;
    f.ui.collapsed_collections = all.iter().map(|c| c.id).collect();
    let planning = list_named(&f.state, "Planning");
    let work = RoomKey::Collection(all[0].id);
    let target = RoomKey::List(planning);

    assert!(!f.ui.select_room(&f.state, work));
    assert!(f.shell().sidebar.collections[0].collapsed);
    assert!(f.ui.select_room(&f.state, target));
    let shell = f.shell();
    assert_eq!(shell.sidebar.selected, target);
    assert_eq!(shell.sidebar.collections[0], all[0]);
    assert!(shell.sidebar.collections[1..].iter().all(|c| c.collapsed));
    assert!(!f.ui.select_room(&f.state, target));

    // An explicit navigation to the already selected list also reveals it.
    f.ui.toggle_collection(all[0].id);
    assert_eq!(f.shell().sidebar.selected, target);
    assert!(f.ui.select_room(&f.state, target));
    assert!(!f.shell().sidebar.collections[0].collapsed);
}

#[test]
fn room_resolution_keeps_live_rooms_and_defaults_for_deleted_or_unknown_ones() {
    let mut f = fixtures::my_day();
    let list = list_named(&f.state, "Office");
    let collection = f.shell().sidebar.collections[0].id;
    let smart = RoomKey::Smart(Smart::Planned);
    assert_eq!(smart.resolved(&State::default()), smart);
    for (room, entity) in [
        (RoomKey::List(list), list.into()),
        (RoomKey::Collection(collection), collection.into()),
    ] {
        assert_eq!(room.resolved(&State::default()), RoomKey::default());
        assert_eq!(room.resolved(&f.state), room);
        run(&mut f.state, fixtures_now(), Command::Delete { id: entity });
        assert_eq!(room.resolved(&f.state), RoomKey::default());
    }
}

#[test]
fn pruning_drops_deleted_and_unknown_collection_ids() {
    let mut f = fixtures::my_day();
    let all = f.shell().sidebar.collections;
    let (deleted, kept) = (all[0].id, all[1].id);
    let unknown = CollectionId::from_uuid(todo_core::ids::uuid_v7(1, [0; 10]));
    f.ui.collapsed_collections = [deleted, kept, unknown].into();
    run(
        &mut f.state,
        fixtures_now(),
        Command::Delete { id: deleted.into() },
    );
    assert!(f.ui.prune_collapsed_collections(&f.state));
    assert_eq!(f.ui.collapsed_collections, [kept].into());
    assert!(!f.ui.prune_collapsed_collections(&f.state));
}

#[test]
fn smart_list_counts_and_agent_rows() {
    let shell = fixtures::my_day().shell();
    let counts: Vec<Option<u32>> = shell.sidebar.smart.iter().map(|n| n.count).collect();
    // My Day, Important, Planned, All, Completed (open counts; Completed counts finished tasks).
    assert_eq!(counts[0], Some(9));
    assert_eq!(counts[1], Some(3));
    assert!(counts[2].unwrap_or(0) >= 10);
    assert_eq!(counts[5], None);
    assert!(shell.sidebar.smart[5].pip.is_some());
    let agents: Vec<(&str, &str)> = shell
        .sidebar
        .agents
        .iter()
        .map(|a| (a.name.as_ref(), a.status.as_ref()))
        .collect();
    assert_eq!(
        agents,
        [("Claude", "3 added"), ("Codex", "3 edited"), ("Pi", "idle")]
    );
}

#[test]
fn the_sync_line_says_what_is_going_on() {
    let now = Now::fixed("2026-11-21T10:35:00Z");
    let mut s = SyncInfo {
        paired: true,
        connection: Connection::Online,
        pending: 0,
        failed: 0,
        unauthorized: false,
        synced_at: Some("2026-11-21T10:34:00Z".parse().expect("ts")),
    };
    assert_eq!(sync_line(&s, &now), ("Synced 10:34".into(), Tone::Ok));
    s.connection = Connection::Offline;
    s.pending = 2;
    assert_eq!(
        sync_line(&s, &now),
        ("Offline, 2 waiting".into(), Tone::Problem)
    );
    s.failed = 1;
    assert_eq!(sync_line(&s, &now).0, "1 couldn't sync");
    s.unauthorized = true;
    assert_eq!(sync_line(&s, &now).0, "Signed out by the server");
    assert_eq!(sync_line(&SyncInfo::signed_out(), &now).0, "Not signed in");
}

#[test]
fn hundreds_of_tasks_derive_quickly() {
    // About the owner's size: 600 tasks, most of them done.
    let mut f = fixtures::my_day();
    let list = list_named(&f.state, "Errands");
    for i in 0..500u32 {
        let id = TaskId::from_uuid(todo_core::ids::uuid_v7(
            1_800_000_000_000 + u64::from(i),
            [7; 10],
        ));
        run(
            &mut f.state,
            "2026-11-01T10:00:00Z",
            Command::CreateTask(Box::new(todo_core::command::NewTask::new(
                id,
                list,
                format!("Task {i}"),
            ))),
        );
        if i % 5 != 0 {
            run(
                &mut f.state,
                "2026-11-02T10:00:00Z",
                Command::CompleteTask { id },
            );
        }
    }
    f.ui.room = RoomKey::Smart(Smart::All);
    f.ui.show_completed = true;
    let start = std::time::Instant::now();
    let shell = f.shell();
    let took = start.elapsed();
    assert!(shell.row_count() > 500);
    // Generous for debug builds on slow CI machines; release is a few milliseconds.
    assert!(took.as_millis() < 1500, "derive took {took:?}");
}

#[test]
fn hosts_for_untitled_links() {
    assert_eq!(host("https://www.example.com/a/b?c"), "example.com");
    assert_eq!(host("example.org"), "example.org");
}

#[test]
fn only_web_links_are_clickable() {
    let mut f = fixtures::my_day();
    let task = *f.state.tasks.keys().next().expect("a task");
    let targets = [
        "https://todo.example.com/docs",
        "file:///C:/Windows/System32/calc.exe",
        r"\server\share\setup.exe",
        "search-ms:query=invoice",
        "javascript:alert(1)",
        "not a url",
    ];
    for (i, target) in targets.iter().enumerate() {
        let id = LinkId::from_uuid(todo_core::ids::uuid_v7(1, [i as u8; 10]));
        run(
            &mut f.state,
            "2026-06-15T08:00:00Z",
            Command::AddLink {
                id,
                task,
                kind: LinkKind::Web {
                    url: "https://todo.example.com/".into(),
                    title: Some(format!("link {i}")),
                },
            },
        );
        // Imports and other clients may have stored anything: put the target in place directly.
        if let Some(link) = f.state.links.get_mut(&id) {
            link.kind = LinkKind::Web {
                url: (*target).to_string(),
                title: Some(format!("link {i}")),
            };
        }
    }
    f.ui.task = Some(task);
    let detail = f.shell().detail.expect("detail");
    let shown: Vec<(&str, Option<&str>)> = detail
        .links
        .iter()
        .filter(|l| l.title.starts_with("link "))
        .map(|l| (l.detail.as_ref(), l.url.as_deref()))
        .collect();
    assert_eq!(shown.len(), targets.len());
    for (detail, url) in shown {
        if detail.starts_with("https://") {
            assert_eq!(url, Some("https://todo.example.com/docs"));
        } else {
            assert_eq!(url, None, "{detail} must not be clickable");
        }
    }
}

#[test]
fn only_my_day_mentions_the_global_quick_add() {
    // Ctrl+Alt+Space always adds to My Day, so an empty list points at its own quick-add instead.
    let empty = fixtures::empty().shell();
    assert_eq!(empty.room.empty_line.as_ref(), "Nothing on the shelf yet.");
    assert!(!empty.room.empty_hint.contains("Ctrl+Alt+Space"));
    assert!(empty.room.empty_hint.contains("Ctrl+N"));
    let mut f = fixtures::my_day();
    assert!(f.shell().room.empty_hint.contains("Ctrl+Alt+Space"));
    f.ui.room = RoomKey::Smart(Smart::Completed);
    assert!(!f.shell().room.empty_hint.contains("Ctrl+Alt+Space"));
}

#[test]
fn names_with_line_breaks_are_drawn_on_one_line() {
    // Another client may store a name with a line break or a control character; one-line text can't
    // shape that, so every name the views draw on one line is normalised.
    let mut f = fixtures::my_day();
    let planning = list_named(&f.state, "Planning");
    let list = f.state.lists.get_mut(&planning).expect("list");
    list.name = "Plan\nning".into();
    let work = list.collection.expect("in a collection");
    f.state.collections.get_mut(&work).expect("collection").name = "Wo\r\nrk".into();
    for tag in f.state.tags.values_mut() {
        tag.name = format!("{}\t\u{2028}x", tag.name);
    }
    let q4 = f.ui.task.expect("selected");
    f.state.tasks.get_mut(&q4).expect("task").title = "Prepare\nQ4".into();
    for step in f.state.steps.values_mut() {
        step.title = format!("{}\n", step.title);
    }
    let shell = f.shell();
    let one_line = |s: &str| !s.chars().any(|c| c.is_control() || c == '\u{2028}');
    for row in shell.rows() {
        assert!(one_line(&row.title), "{:?}", row.title);
        assert!(one_line(&row.list.name), "{:?}", row.list.name);
        assert!(row.list.collection.as_deref().is_none_or(one_line));
        assert!(row.tags.iter().all(|t| one_line(t)), "{:?}", row.tags);
    }
    let q4_row = shell.rows().find(|r| r.id == q4).expect("row");
    assert_eq!(q4_row.title.as_ref(), "Prepare Q4");
    assert_eq!(q4_row.list.name.as_ref(), "Plan ning");
    for c in &shell.sidebar.collections {
        assert!(one_line(&c.name), "{:?}", c.name);
        assert!(c.lists.iter().all(|l| one_line(&l.list.name)));
    }
    let detail = shell.detail.expect("detail");
    assert_eq!(detail.list.name.as_ref(), "Plan ning");
    assert_eq!(detail.list.collection.as_deref(), Some("Wo  rk"));
    assert!(detail.steps.iter().all(|s| one_line(&s.title)));
    assert!(detail.fields.iter().all(|f| one_line(&f.value)));
    // The room title too.
    f.ui.room = RoomKey::List(planning);
    assert_eq!(f.shell().room.title.as_ref(), "Plan ning");
}

#[test]
fn done_today_counts_only_what_was_completed_since_midnight() {
    let mut f = fixtures::my_day();
    // My Day's header counts its whole plan: 3 of 12 done. All three were completed this morning.
    let shell = f.shell();
    assert_eq!(shell.room.summary().0, 3);
    assert_eq!(shell.room.done_today, 3);
    // A task in today's plan that was completed yesterday still counts for the plan, not for today.
    let open = shell.room.tasks[0].id;
    run(
        &mut f.state,
        "2026-11-20T18:00:00Z",
        Command::CompleteTask { id: open },
    );
    let shell = f.shell();
    assert_eq!(shell.room.summary().0, 4);
    assert_eq!(shell.room.done_today, 3);
    // In a list, older completions don't count either: Reading list's two were finished earlier this month.
    let reading = list_named(&f.state, "Reading list");
    f.ui.room = RoomKey::List(reading);
    f.ui.task = None;
    let shell = f.shell();
    assert_eq!(shell.room.completed_count, 2);
    assert_eq!(shell.room.done_today, 0);
}

#[test]
fn attachments_keep_their_stored_name_and_draw_a_one_line_label() {
    let mut f = fixtures::my_day();
    let q4 = f.ui.task.expect("selected");
    let pdf = f
        .state
        .attachments
        .values_mut()
        .find(|a| a.task == q4 && a.file_name.ends_with(".pdf"))
        .expect("the agenda");
    pdf.file_name = "Q4\nagenda.pdf".into();
    let detail = f.shell().detail.expect("detail");
    let row = detail
        .attachments
        .iter()
        .find(|a| a.name.ends_with(".pdf"))
        .expect("row");
    // Save, Open and the open check go by the stored name; only the label is one line.
    assert_eq!(row.name.as_ref(), "Q4\nagenda.pdf");
    assert_eq!(row.label.as_ref(), "Q4 agenda.pdf");
    assert!(crate::model::files::can_open(&row.name));
    // The save dialog offers the stored name made safe for Windows, not the label.
    assert_eq!(
        crate::model::files::safe_file_name(&row.name),
        "Q4_agenda.pdf"
    );
}

#[test]
fn a_finished_day_always_offers_planning_the_next_day() {
    // An unfinished day offers no planning.
    let shell = fixtures::my_day().shell();
    assert!(!shell.room.finished_day());
    assert_eq!(shell.room.plan_weekday, None);
    assert_eq!(shell.room.plan_only(), None);

    // Finished, with tasks due tomorrow: the hand-over carries the button.
    let mut f = fixtures::finished();
    let shell = f.shell();
    assert_eq!(shell.room.plan_weekday.as_deref(), Some("Sunday"));
    assert!(shell.room.handover().is_some());
    assert_eq!(shell.room.plan_only(), None);

    // Finished, with nothing due tomorrow: planning is still offered, without a hand-over.
    let mut empty = fixtures::finished_nothing_tomorrow();
    let shell = empty.shell();
    assert!(shell.room.handover().is_none());
    assert_eq!(shell.room.plan_only().map(|s| s.as_ref()), Some("Sunday"));

    // With the Completed section open, its rows show instead.
    empty.ui.show_completed = true;
    let shell = empty.shell();
    assert_eq!(shell.room.plan_only(), None);
    assert!(shell.row_count() > 0);

    // Filtering is no reason to plan: neither the hand-over nor the button shows, and the room says
    // nothing matches.
    for f in [&mut f, &mut empty] {
        f.ui.show_completed = false;
        f.ui.filter = "zzz".into();
        let shell = f.shell();
        assert!(shell.room.finished_day());
        assert_eq!(shell.room.plan_weekday, None);
        assert!(shell.room.handover().is_none());
        assert_eq!(shell.room.plan_only(), None);
        assert_eq!(
            shell.room.empty_line.as_ref(),
            "Nothing matches the filter."
        );
    }
}

#[test]
fn planning_names_the_day_after_today() {
    let mut f = fixtures::finished_nothing_tomorrow();
    // Saturday night at 21:40 UTC; just before midnight it is still Saturday.
    f.now = Now::fixed("2026-11-21T23:59:00Z");
    assert_eq!(f.shell().room.plan_weekday.as_deref(), Some("Sunday"));
}

#[test]
fn a_hand_over_task_completed_from_its_box_keeps_its_place_until_released() {
    let mut f = fixtures::finished();
    let before = f.shell();
    let next = before.room.handover().expect("hands over");
    let task = next.also.first().expect("another task tomorrow").id;
    let shown: Vec<TaskId> = before.shown_tasks().collect();
    assert!(shown.contains(&task), "the hand-over's tasks are shown");
    run(
        &mut f.state,
        f.now.at.to_string().as_str(),
        Command::CompleteTask { id: task },
    );
    // Without holding, it leaves the hand-over at once.
    let gone = f.shell();
    assert!(
        gone.room
            .handover()
            .is_some_and(|n| n.child(task).is_none())
    );
    // Held, it stays in its place, drawn done, while its completion plays.
    f.ui.holding.insert(task);
    let held = f.shell();
    let next = held.room.handover().expect("still hands over");
    assert_eq!(
        next.child(task),
        before.room.handover().and_then(|n| n.child(task))
    );
    assert!(next.tasks().any(|t| t.id == task && t.done));
    assert!(held.shown_tasks().any(|id| id == task));
    // The finished day stays finished: the task isn't My Day's.
    assert!(held.room.finished_day());
}

#[test]
fn the_detail_offers_due_priority_and_tags_to_edit_and_only_shows_the_rest() {
    let f = fixtures::my_day();
    let shell = f.shell();
    let detail = shell.detail.expect("a task is open");
    let edits: Vec<(&str, Option<FieldEdit>)> =
        detail.fields.iter().map(|f| (f.label, f.edit)).collect();
    assert_eq!(edits[0], ("Due", Some(FieldEdit::Due)));
    assert_eq!(edits[1], ("Priority", Some(FieldEdit::Priority)));
    // Tags come last, always there to pick from (T); reminders and repeats are only shown.
    assert_eq!(edits.last(), Some(&("Tags", Some(FieldEdit::Tags))));
    assert!(
        edits[2..edits.len() - 1]
            .iter()
            .all(|(_, edit)| edit.is_none()),
        "{edits:?}"
    );
    // The tags, by name, each with its colour.
    let names: Vec<&str> = detail.tags.iter().map(|t| t.name.as_ref()).collect();
    assert_eq!(names, ["planning", "q4"]);
    // A task without tags still has the row, with a quiet dash.
    let mut bare = fixtures::my_day();
    let id = bare.ui.task.expect("selected");
    bare.state.tasks.get_mut(&id).expect("task").tags.clear();
    let d = bare.shell().detail.expect("detail");
    let tags = d.fields.last().expect("tags");
    assert_eq!((tags.label, tags.value.as_ref()), ("Tags", "–"));
    assert_eq!(tags.kind, FieldKind::Empty);
    assert!(d.tags.is_empty());
    // The priority reads as its name, or a dash when there's none.
    let mut f = fixtures::my_day();
    for (level, value) in [
        (Priority::High, "High"),
        (Priority::Normal, "Normal"),
        (Priority::Low, "Low"),
        (Priority::None, "–"),
    ] {
        let id = f.ui.task.expect("selected");
        f.state.tasks.get_mut(&id).expect("task").priority = level;
        let d = f.shell().detail.expect("detail");
        assert_eq!(d.priority, level);
        assert_eq!(d.fields[1].value.as_ref(), value);
    }
}

#[test]
fn a_deleted_task_keeps_its_place_until_its_row_has_left() {
    let mut f = fixtures::my_day();
    let before = f.shell();
    let row = before.room.tasks[2].clone();
    let snapshot = f.state.tasks[&row.id].clone();
    run(
        &mut f.state,
        fixtures_now(),
        Command::Delete { id: row.id.into() },
    );
    // Without its snapshot, it's gone at once.
    let gone = f.shell();
    assert!(gone.rows().all(|r| r.id != row.id));
    // Leaving, it keeps its place and looks as it did; the counts change once it has left.
    f.ui.leaving.insert(row.id, snapshot);
    let leaving = f.shell();
    assert_eq!(leaving.room.tasks[2], row);
    assert_eq!(leaving.room.summary(), before.room.summary());
    assert_eq!(
        leaving.sidebar.smart[0].count,
        before.sidebar.smart[0].count
    );
    // The detail shows the truth: a deleted task has none.
    f.ui.task = Some(row.id);
    assert!(f.shell().detail.is_none());
}

#[test]
fn a_task_moved_out_of_a_list_keeps_its_place_until_its_row_has_left() {
    let mut f = fixtures::my_day();
    let office = list_named(&f.state, "Office");
    let errands = list_named(&f.state, "Errands");
    f.ui = UiState {
        room: RoomKey::List(office),
        ..UiState::default()
    };
    let before = f.shell();
    let row = before.room.tasks[0].clone();
    let snapshot = f.state.tasks[&row.id].clone();
    run(
        &mut f.state,
        fixtures_now(),
        Command::MoveTask {
            id: row.id,
            list: errands,
            placement: todo_core::command::Placement::First,
        },
    );
    assert!(f.shell().rows().all(|r| r.id != row.id));
    f.ui.leaving.insert(row.id, snapshot);
    f.ui.task = Some(row.id);
    let leaving = f.shell();
    assert_eq!(leaving.room.tasks[0], row);
    assert_eq!(leaving.room.summary(), before.room.summary());
    // The detail already says where it went.
    let detail = leaving.detail.expect("still open");
    assert_eq!(detail.list.name.as_ref(), "Errands");
}

#[test]
fn a_hand_over_task_deleted_leaves_its_line_and_one_moved_stays() {
    let mut f = fixtures::finished();
    let shell = f.shell();
    let next = shell.room.handover().expect("hands over");
    let task = next.tasks().nth(1).expect("a second task").clone();
    // The hand-over's lines count as shown, as rows do.
    assert_eq!(shell.shown_row(task.id), Some(&task));
    assert!(shell.rows().all(|r| r.id != task.id), "not a row");
    // Moved to another list, it's still tomorrow's: still shown, so it keeps the selection.
    let other = f
        .state
        .lists
        .values()
        .find(|l| l.id != f.state.tasks[&task.id].list && f.state.is_live(l.id.into()))
        .expect("another list")
        .id;
    let mut moved = f.state.clone();
    run(
        &mut moved,
        fixtures_now(),
        Command::MoveTask {
            id: task.id,
            list: other,
            placement: todo_core::command::Placement::First,
        },
    );
    let after = derive(&moved, &f.ui, &f.sync, &f.now);
    assert!(after.shown_row(task.id).is_some());
    // Deleted, it's gone, unless it's leaving: then its line keeps its place until released.
    let snapshot = f.state.tasks[&task.id].clone();
    run(
        &mut f.state,
        fixtures_now(),
        Command::Delete { id: task.id.into() },
    );
    assert!(f.shell().shown_row(task.id).is_none());
    f.ui.leaving.insert(task.id, snapshot);
    let leaving = f.shell();
    let next = leaving.room.handover().expect("still hands over");
    assert_eq!(next.tasks().nth(1).map(|t| t.id), Some(task.id));
}
