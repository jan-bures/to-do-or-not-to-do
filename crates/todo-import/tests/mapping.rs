#![allow(clippy::unwrap_used)]
//! Mapping tests on the synthetic fixture: structure, order from the cache, Flagged Emails, recurrence,
//! fields, and the bundle-versus-export verification.

mod common;

use jiff::Timestamp;
use jiff::civil::Date;
use todo_core::model::{Collection, Entity, LinkKind, List, Priority, Step, Task};
use todo_core::recurrence::Rule;
use todo_core::time::Weekday;
use todo_import::bundle::{Bundle, SourceKind};
use todo_import::verify::verify;

fn entity<'a>(b: &'a Bundle, kind: SourceKind, id: &str) -> &'a Entity {
    b.items
        .iter()
        .find(|i| i.source.kind == kind && i.source.id == id)
        .and_then(|i| i.entity())
        .unwrap_or_else(|| panic!("no {kind:?} {id}"))
}

fn task<'a>(b: &'a Bundle, id: &str) -> &'a Task {
    match entity(b, SourceKind::Task, id) {
        Entity::Task(t) => t,
        _ => unreachable!(),
    }
}

fn list<'a>(b: &'a Bundle, id: &str) -> &'a List {
    match entity(b, SourceKind::List, id) {
        Entity::List(l) => l,
        _ => unreachable!(),
    }
}

/// Source IDs of the given kind, in bundle (display) order.
fn order(b: &Bundle, kind: SourceKind, keep: impl Fn(&Entity) -> bool) -> Vec<String> {
    b.items
        .iter()
        .filter(|i| i.source.kind == kind && i.entity().is_some_and(&keep))
        .map(|i| i.source.id.clone())
        .collect()
}

fn ts(s: &str) -> Timestamp {
    s.parse().unwrap()
}

#[test]
fn counts_match_the_fixture() {
    let b = common::bundle();
    let c = &b.counts;
    assert_eq!(c.collections, 2, "the deleted group is not imported");
    assert_eq!(c.lists, 5, "the cache-only list is not imported");
    assert_eq!((c.tasks_open, c.tasks_completed), (9, 2));
    assert_eq!(c.steps, 3);
    assert_eq!((c.links, c.email_links), (2, 2));
    assert_eq!((c.attachments, c.attachment_bytes), (1, 11));
    assert_eq!(c.reminders, 1, "a reminder that is off is not imported");
    assert_eq!(c.tags, 2, "categories are deduplicated case-insensitively");
    assert_eq!(
        (c.due_dates, c.high_priority, c.notes, c.recurring),
        (3, 1, 2, 1)
    );
    assert_eq!(b.report.cache_only_lists, 1);
    assert_eq!(b.report.my_day_marks_skipped, 2);
    assert_eq!(b.report.due_date_mismatches, 0);
    assert!(b.report.warnings.is_empty(), "{:?}", b.report.warnings);
}

#[test]
fn building_is_deterministic() {
    let a = common::bundle();
    let b = common::bundle();
    assert_eq!(a, b);
    assert_eq!(a.digest().unwrap(), b.digest().unwrap());
}

#[test]
fn flagged_emails_is_an_ordinary_list_with_email_links() {
    let b = common::bundle();
    let flagged = list(&b, "L-flagged");
    assert_eq!(flagged.name, "Flagged Emails");
    assert_eq!(flagged.collection, None);
    let link = |id: &str| match entity(&b, SourceKind::Link, id) {
        Entity::Link(l) => l.clone(),
        _ => unreachable!(),
    };
    let l1 = link("LR-1");
    assert_eq!(l1.task, task(&b, "F-1").id);
    let LinkKind::Email(m) = &l1.kind else {
        panic!("not an email link")
    };
    assert_eq!(m.subject, "Re: quarterly report");
    assert_eq!(m.sender, "sender@example.com");
    assert_eq!(m.received_at, ts("2026-01-20T11:58:00Z"));
    assert_eq!(m.message_id, "MSG-1");
    assert_eq!(
        m.web_link.as_deref(),
        Some("https://mail.example.com/owa/?ItemID=MSG-1")
    );
    // Without cache metadata an Outlook link is still an email link.
    let LinkKind::Email(m2) = &link("LR-2").kind else {
        panic!("not an email link")
    };
    assert_eq!(m2.sender, "unknown sender");
    assert_eq!(m2.received_at, ts("2026-01-21T12:00:00Z"));
}

#[test]
fn order_comes_from_the_cache() {
    let b = common::bundle();
    let collections: Vec<String> = order(&b, SourceKind::Group, |_| true);
    assert_eq!(collections, ["GR-work", "GR-home"]);
    let top = order(
        &b,
        SourceKind::List,
        |e| matches!(e, Entity::List(l) if l.collection.is_none()),
    );
    assert_eq!(
        top,
        ["L-tasks", "L-someday", "L-flagged"],
        "default list first"
    );
    let home = match entity(&b, SourceKind::Group, "GR-home") {
        Entity::Collection(Collection { id, .. }) => *id,
        _ => unreachable!(),
    };
    assert_eq!(list(&b, "L-groceries").collection, Some(home));

    let in_list = |l: &str| {
        let id = list(&b, l).id;
        order(
            &b,
            SourceKind::Task,
            move |e| matches!(e, Entity::Task(t) if t.list == id),
        )
    };
    // Descending by default (new on top), by time rather than text.
    assert_eq!(in_list("L-groceries"), ["G-3", "G-1", "G-2"]);
    // This list is sorted ascending in the cache.
    assert_eq!(in_list("L-projects"), ["P-2", "P-1"]);
    // Steps: descending cache position.
    assert_eq!(order(&b, SourceKind::Step, |_| true), ["S-2", "S-3", "S-1"]);

    // Position keys increase in bundle order within each scope.
    let keys: Vec<_> = b
        .items
        .iter()
        .filter_map(|i| match i.entity() {
            Some(Entity::Step(Step { position, .. })) => Some(position.clone()),
            _ => None,
        })
        .collect();
    assert!(keys.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn orphaned_cache_steps_are_ignored() {
    let b = common::bundle();
    assert_eq!(b.report.orphan_cache_steps, 2);
    assert_eq!(b.counts.steps, 3);
    assert!(!b.items.iter().any(|i| i.source.id.starts_with("S-orphan")));
    assert_eq!(b.report.steps_without_cache_order, 0);
}

#[test]
fn recurrence_that_maps_and_that_does_not() {
    let b = common::bundle();
    let weekly = task(&b, "T-3").recurrence.clone().unwrap();
    assert_eq!(
        weekly.rule,
        Rule::Weekly {
            interval: 1,
            days: [Weekday::Mon].into_iter().collect()
        }
    );
    assert_eq!(task(&b, "T-3").due, Some(Date::constant(2026, 3, 9)));
    // Not mappable: imported without recurrence, and reported with the raw pattern.
    assert!(task(&b, "T-4").recurrence.is_none());
    let [u] = b.report.unmapped_recurrence.as_slice() else {
        panic!("expected one unmapped recurrence")
    };
    assert_eq!(u.task, "T-4");
    assert!(u.reason.contains("relativeMonthly"));
    assert_eq!(u.pattern["pattern"]["type"], "relativeMonthly");
}

#[test]
fn fields_keep_their_history() {
    let b = common::bundle();
    let t1 = task(&b, "T-1");
    assert_eq!(t1.priority, Priority::High);
    assert_eq!(t1.notes, "Pack\nboots");
    assert_eq!(
        t1.due,
        Some(Date::constant(2026, 3, 2)),
        "owner-local midnight"
    );
    assert_eq!(t1.created.at, ts("2026-01-10T08:00:00.1234567Z"));
    assert_eq!(t1.updated.at, ts("2026-01-11T08:00:00Z"));
    assert_eq!(t1.my_day, None, "My Day marks are not imported");
    assert_eq!(t1.tags.len(), 1);
    let t2 = task(&b, "T-2");
    assert_eq!(t2.completed_at, Some(ts("2026-01-05T00:00:00Z")));
    assert_eq!(t2.notes, "", "whitespace-only notes are empty");
    assert_eq!(t2.tags.len(), 2);
    assert_eq!(task(&b, "G-2").priority, Priority::Low);
    assert_eq!(task(&b, "P-2").notes, "Check & compare\nline two");
    match entity(&b, SourceKind::Step, "S-2") {
        Entity::Step(s) => assert_eq!(s.completed_at, Some(ts("2026-01-09T10:00:00.5Z"))),
        _ => unreachable!(),
    }
    match entity(&b, SourceKind::Reminder, "T-1") {
        Entity::Reminder(r) => assert_eq!(r.at, ts("2026-03-02T07:00:00Z")),
        _ => unreachable!(),
    }
    match entity(&b, SourceKind::Attachment, "A-1") {
        Entity::Attachment(a) => {
            assert_eq!(a.file_name, "list.txt");
            assert_eq!(a.size_bytes, 11);
            assert!(a.storage_key.starts_with("import/sha256/"));
            assert_eq!(b.blob(a.id).unwrap().content_base64, "aGVsbG8gd29ybGQ=");
        }
        _ => unreachable!(),
    }
    assert_eq!(list(&b, "L-tasks").color.as_deref(), Some("#2564cf"));
    assert_eq!(
        list(&b, "L-projects").color.as_deref(),
        Some("mystery_theme")
    );
}

#[test]
fn verify_passes_and_catches_differences() {
    let export = common::export();
    let b = common::bundle();
    let rep = verify(&export, &b, false);
    assert!(rep.ok(), "{}", rep.render());
    assert_eq!(rep.rows.len(), 5);
    assert!(
        rep.rows.iter().all(|r| r.name.is_none()),
        "names only on request"
    );
    assert!(rep.render().contains("verify: PASS"));

    let mut bad = b.clone();
    let pos = bad.items.iter().position(|i| i.source.id == "T-1").unwrap();
    if let todo_core::Command::ImportEntity { entity } = &mut bad.items[pos].command
        && let Entity::Task(t) = entity.as_mut()
    {
        t.title = "Something else".into();
        t.completed_at = Some(ts("2026-01-12T00:00:00Z"));
    }
    let step = bad.items.iter().position(|i| i.source.id == "S-1").unwrap();
    bad.items.remove(step);
    let rep = verify(&export, &bad, false);
    assert!(!rep.ok());
    let text = rep.render();
    assert!(text.contains("title differs"), "{text}");
    assert!(text.contains("status differs"), "{text}");
    assert!(text.contains("missing from the bundle"), "{text}");
    assert!(text.contains("counts differ"), "{text}");
    assert!(
        !text.contains("Plan the trip"),
        "reports never contain titles"
    );
}

#[test]
fn bundle_round_trips_through_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let b = common::bundle();
    let path = dir.path().join("bundle.json");
    b.write(&path).unwrap();
    assert_eq!(Bundle::read(&path).unwrap(), b);
    // Tampering with the items without updating the counts is caught.
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    v["items"].as_array_mut().unwrap().pop();
    std::fs::write(&path, v.to_string()).unwrap();
    assert!(Bundle::read(&path).is_err());
}

#[test]
fn the_cache_loads_from_a_file_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = common::cache_file(dir.path());
    let c = todo_import::cache::Cache::load(&path).unwrap();
    assert_eq!(c.groups.len(), 2);
    assert_eq!(c.orphan_steps, 2);
    assert!(todo_import::cache::Cache::load(&dir.path().join("missing.db")).is_err());
}
