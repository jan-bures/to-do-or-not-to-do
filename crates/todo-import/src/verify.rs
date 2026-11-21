//! `verify`: compares a bundle with the raw Graph export, list by list and field by field.
//!
//! The expected values are recomputed from the raw Graph JSON and compared with the entities in the
//! bundle. Checked per task: title, status, every date (creation, modification, completion, due date,
//! reminder), importance, notes, steps (title, checked, creation and checked times), links (subject or
//! URL, and with `--cache` the received time of email links), attachments (name, size, content hash,
//! time), reminders and recurrence. Order comes from the cache and cannot be checked against Graph.
//!
//! Text fields use the conversions in [`crate::mapping`]. Dates do not: this module reads every source
//! date with its own strict parser and its own time zone resolution, so a date the mapping misreads, or
//! replaces, shows up as a mismatch. A date this module cannot read either must be listed in the
//! bundle's [`Report::lossy`](crate::bundle::Report::lossy); otherwise it is a mismatch too.

use crate::bundle::{Bundle, SourceKind};
use crate::cache::Cache;
use crate::graph::{DateTimeTz, Export};
use crate::mapping;
use base64::Engine;
use jiff::Timestamp;
use jiff::tz::TimeZone;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use todo_core::ids::{ListId, TaskId};
use todo_core::model::{Entity, LinkKind};

/// Per-list tallies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Open tasks.
    pub open: usize,
    /// Completed tasks.
    pub done: usize,
    /// Steps.
    pub steps: usize,
    /// Links.
    pub links: usize,
    /// Attachments.
    pub attachments: usize,
    /// Attachment content bytes.
    pub attachment_bytes: u64,
}

impl std::ops::AddAssign for Tally {
    fn add_assign(&mut self, o: Self) {
        self.open += o.open;
        self.done += o.done;
        self.steps += o.steps;
        self.links += o.links;
        self.attachments += o.attachments;
        self.attachment_bytes += o.attachment_bytes;
    }
}

/// One row of the report.
#[derive(Clone, Debug)]
pub struct ListRow {
    /// Label of the Graph list ID.
    pub label: String,
    /// The list name (only with `--show-names`).
    pub name: Option<String>,
    /// Counted in the export.
    pub graph: Tally,
    /// Counted in the bundle.
    pub bundle: Tally,
    /// True if counts match and no field of the list's tasks differs.
    pub ok: bool,
}

/// The verification result.
#[derive(Clone, Debug, Default)]
pub struct VerifyReport {
    /// Rows, in export order.
    pub rows: Vec<ListRow>,
    /// Every mismatch, naming objects by label and fields by name.
    pub problems: Vec<String>,
    /// Known gaps that are not failures (unmapped recurrence, attachments without upload).
    pub notes: Vec<String>,
}

impl VerifyReport {
    /// True if nothing differs.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// Totals over all rows: (export, bundle).
    #[must_use]
    pub fn totals(&self) -> (Tally, Tally) {
        let mut g = Tally::default();
        let mut b = Tally::default();
        for r in &self.rows {
            g += r.graph;
            b += r.bundle;
        }
        (g, b)
    }

    /// The report as text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{:<14} {:<24} {:>11} {:>11} {:>11} {:>11} {:>9}  status",
            "list", "name", "open g/b", "done g/b", "steps g/b", "links g/b", "att g/b"
        );
        let pair = |a: usize, b: usize| format!("{a}/{b}");
        for r in &self.rows {
            let name: String = r.name.as_deref().unwrap_or("-").chars().take(24).collect();
            let _ = writeln!(
                s,
                "{:<14} {:<24} {:>11} {:>11} {:>11} {:>11} {:>9}  {}",
                r.label,
                name,
                pair(r.graph.open, r.bundle.open),
                pair(r.graph.done, r.bundle.done),
                pair(r.graph.steps, r.bundle.steps),
                pair(r.graph.links, r.bundle.links),
                pair(r.graph.attachments, r.bundle.attachments),
                if r.ok { "ok" } else { "DIFF" }
            );
        }
        let (g, b) = self.totals();
        let _ = writeln!(
            s,
            "\ntotals (export/bundle): lists {}/{}, tasks {}/{} (open {}/{}, completed {}/{}), steps {}/{}, links {}/{}, attachments {}/{} ({}/{} bytes)",
            self.rows.len(),
            self.rows.len(),
            g.open + g.done,
            b.open + b.done,
            g.open,
            b.open,
            g.done,
            b.done,
            g.steps,
            b.steps,
            g.links,
            b.links,
            g.attachments,
            b.attachments,
            g.attachment_bytes,
            b.attachment_bytes
        );
        for n in &self.notes {
            let _ = writeln!(s, "note: {n}");
        }
        for p in self.problems.iter().take(100) {
            let _ = writeln!(s, "MISMATCH: {p}");
        }
        if self.problems.len() > 100 {
            let _ = writeln!(s, "... and {} more", self.problems.len() - 100);
        }
        let _ = writeln!(
            s,
            "verify: {}",
            if self.ok() {
                "PASS".to_owned()
            } else {
                format!("FAIL ({} mismatches)", self.problems.len())
            }
        );
        s
    }
}

/// Strict readers for source dates, independent of [`crate::mapping`].
mod source {
    use crate::graph::DateTimeTz;
    use jiff::Timestamp;
    use jiff::civil::DateTime;
    use jiff::tz::{Offset, TimeZone};

    fn digits(s: &str, n: usize) -> Option<(i32, &str)> {
        let head = s.get(..n)?;
        if !head.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some((head.parse().ok()?, &s[n..]))
    }

    fn lit(s: &str, c: char) -> Option<&str> {
        s.strip_prefix(c)
    }

    /// `YYYY-MM-DDTHH:MM:SS[.f…]`, with up to nine fraction digits; returns the rest of the input.
    fn civil(s: &str) -> Option<(DateTime, &str)> {
        let (y, s) = digits(s, 4)?;
        let (mo, s) = digits(lit(s, '-')?, 2)?;
        let (d, s) = digits(lit(s, '-')?, 2)?;
        let (h, s) = digits(lit(s, 'T')?, 2)?;
        let (mi, s) = digits(lit(s, ':')?, 2)?;
        let (sec, s) = digits(lit(s, ':')?, 2)?;
        let (nanos, s) = match lit(s, '.') {
            None => (0, s),
            Some(f) => {
                let n = f.bytes().take_while(u8::is_ascii_digit).count();
                if n == 0 || n > 9 {
                    return None;
                }
                let v: i32 = format!("{:0<9}", &f[..n]).parse().ok()?;
                (v, &f[n..])
            }
        };
        let dt = DateTime::new(
            i16::try_from(y).ok()?,
            i8::try_from(mo).ok()?,
            i8::try_from(d).ok()?,
            i8::try_from(h).ok()?,
            i8::try_from(mi).ok()?,
            i8::try_from(sec).ok()?,
            nanos,
        )
        .ok()?;
        Some((dt, s))
    }

    /// An instant with `Z` or a `±HH:MM` offset, as Graph writes `createdDateTime` and friends.
    pub fn instant(s: &str) -> Option<Timestamp> {
        let (dt, rest) = civil(s.trim())?;
        let offset = match rest {
            "Z" | "z" => Offset::UTC,
            _ => {
                let sign = match rest.chars().next()? {
                    '+' => 1,
                    '-' => -1,
                    _ => return None,
                };
                let (h, r) = digits(&rest[1..], 2)?;
                let (m, r) = digits(lit(r, ':')?, 2)?;
                if !r.is_empty() || h > 23 || m > 59 {
                    return None;
                }
                Offset::from_seconds(sign * (h * 3600 + m * 60)).ok()?
            }
        };
        offset.to_timestamp(dt).ok()
    }

    /// A time zone named the way Graph may name one: `UTC`, IANA, or Windows (CLDR).
    pub fn zone(name: &str) -> Option<TimeZone> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("utc") {
            return Some(TimeZone::UTC);
        }
        TimeZone::get(name)
            .ok()
            .or_else(|| crate::windows_zones::iana(name).and_then(|iana| TimeZone::get(iana).ok()))
    }

    /// The instant of a Graph `dateTimeTimeZone` (a wall-clock time without offset, plus a zone).
    pub fn local(v: &DateTimeTz) -> Option<Timestamp> {
        let (dt, rest) = civil(v.date_time.trim())?;
        if !rest.is_empty() {
            return None;
        }
        zone(&v.time_zone)?
            .to_ambiguous_timestamp(dt)
            .compatible()
            .ok()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_graph_dates_strictly() {
            let t = instant("2026-01-10T08:00:00.1234567Z").unwrap();
            assert_eq!(t.to_string(), "2026-01-10T08:00:00.1234567Z");
            assert_eq!(
                instant("2026-01-10T09:30:00+01:30").unwrap().to_string(),
                "2026-01-10T08:00:00Z"
            );
            for bad in [
                "",
                "2026-01-10",
                "2026-01-10T08:00:00",
                "2026-01-10 08:00:00Z",
                "2026-02-30T08:00:00Z",
                "2026-01-10T08:00:00.Z",
                "2026-01-10T08:00:00.1234567890Z",
                "2026-01-10T08:00:00Zjunk",
            ] {
                assert_eq!(instant(bad), None, "{bad}");
            }
            let v = |date_time: &str, time_zone: &str| DateTimeTz {
                date_time: date_time.into(),
                time_zone: time_zone.into(),
            };
            assert_eq!(
                local(&v("2026-07-01T12:00:00.0000000", "W. Europe Standard Time"))
                    .unwrap()
                    .to_string(),
                "2026-07-01T10:00:00Z"
            );
            assert_eq!(
                local(&v("2026-07-01T12:00:00", "UTC")).unwrap().to_string(),
                "2026-07-01T12:00:00Z"
            );
            assert_eq!(local(&v("2026-07-01T12:00:00", "Mars Standard Time")), None);
            assert_eq!(local(&v("2026-07-01T12:00:00Z", "UTC")), None);
        }
    }
}

/// What the export says about one date.
enum Expect<T> {
    /// The export has no value (and none is required).
    Absent,
    /// The value, as read by [`source`].
    Value(T),
    /// The export's value is unreadable, or missing where one is required.
    Unreadable,
}

impl<T> Expect<T> {
    fn map<U>(self, f: impl FnOnce(T) -> U) -> Expect<U> {
        match self {
            Self::Absent => Expect::Absent,
            Self::Value(v) => Expect::Value(f(v)),
            Self::Unreadable => Expect::Unreadable,
        }
    }

    fn value(&self) -> Option<&T> {
        match self {
            Self::Value(v) => Some(v),
            _ => None,
        }
    }
}

/// A required instant: unreadable when missing.
fn required(s: Option<&str>) -> Expect<Timestamp> {
    match s.map(source::instant) {
        Some(Some(at)) => Expect::Value(at),
        _ => Expect::Unreadable,
    }
}

/// An optional `dateTimeTimeZone`.
fn optional_local(v: Option<&DateTimeTz>) -> Expect<Timestamp> {
    match v {
        None => Expect::Absent,
        Some(v) => source::local(v).map_or(Expect::Unreadable, Expect::Value),
    }
}

/// Compares dates and tracks the lossy ones the bundle reports.
struct Dates<'a> {
    lossy: BTreeSet<(SourceKind, &'a str, &'a str)>,
    /// Unreadable dates the bundle reports as lossy.
    lossy_seen: usize,
}

impl Dates<'_> {
    /// `dropped`: the bundle drops the value when it is unreadable (instead of substituting one).
    #[allow(clippy::too_many_arguments)]
    fn check<T: PartialEq>(
        &mut self,
        problems: &mut Vec<String>,
        kind: SourceKind,
        id: &str,
        field: &str,
        expect: Expect<T>,
        got: Option<&T>,
        dropped: bool,
    ) {
        let what = || format!("{} {}: {field}", kind.as_str(), crate::label(id));
        match expect {
            Expect::Absent => {
                if got.is_some() {
                    problems.push(format!("{}: in the bundle but not in the export", what()));
                }
            }
            Expect::Value(v) => {
                if got != Some(&v) {
                    problems.push(format!("{} differs", what()));
                }
            }
            Expect::Unreadable => {
                if !self.lossy.contains(&(kind, id, field)) {
                    problems.push(format!(
                        "{}: unreadable or missing in the export, and not reported as lossy",
                        what()
                    ));
                } else {
                    self.lossy_seen += 1;
                    if dropped && got.is_some() {
                        problems.push(format!("{}: reported as dropped, but present", what()));
                    }
                }
            }
        }
    }
}

/// Compares `bundle` with `export`. With `cache` (the copy the bundle was built from), the received
/// times of email links, which come from the cache, are checked too.
#[must_use]
pub fn verify(
    export: &Export,
    cache: Option<&Cache>,
    bundle: &Bundle,
    show_names: bool,
) -> VerifyReport {
    let mut rep = VerifyReport::default();
    let tz = match TimeZone::get(&bundle.time_zone) {
        Ok(tz) => tz,
        Err(e) => {
            rep.problems
                .push(format!("bundle time zone `{}`: {e}", bundle.time_zone));
            return rep;
        }
    };
    let mut by_source: BTreeMap<(SourceKind, &str), &Entity> = BTreeMap::new();
    for item in &bundle.items {
        let Some(e) = item.entity() else {
            rep.problems.push(format!(
                "{} {}: not an import command",
                item.source.kind.as_str(),
                crate::label(&item.source.id)
            ));
            continue;
        };
        if by_source
            .insert((item.source.kind, item.source.id.as_str()), e)
            .is_some()
        {
            rep.problems.push(format!(
                "{} {}: appears twice in the bundle",
                item.source.kind.as_str(),
                crate::label(&item.source.id)
            ));
        }
    }
    // Bundle tallies per list ID.
    let mut task_list: BTreeMap<TaskId, ListId> = BTreeMap::new();
    let mut tally: BTreeMap<ListId, Tally> = BTreeMap::new();
    for e in by_source.values() {
        if let Entity::Task(t) = e {
            task_list.insert(t.id, t.list);
            let x = tally.entry(t.list).or_default();
            if t.completed_at.is_some() {
                x.done += 1;
            } else {
                x.open += 1;
            }
        }
    }
    for e in by_source.values() {
        let task = match e {
            Entity::Step(s) => s.task,
            Entity::Link(l) => l.task,
            Entity::Attachment(a) => a.task,
            _ => continue,
        };
        let Some(list) = task_list.get(&task) else {
            continue;
        };
        let x = tally.entry(*list).or_default();
        match e {
            Entity::Step(_) => x.steps += 1,
            Entity::Link(_) => x.links += 1,
            Entity::Attachment(a) => {
                x.attachments += 1;
                x.attachment_bytes += bundle.blob(a.id).map_or(0, |b| b.size_bytes);
            }
            _ => {}
        }
    }

    let mut dates = Dates {
        lossy: bundle
            .report
            .lossy
            .iter()
            .map(|l| (l.kind, l.id.as_str(), l.field.as_str()))
            .collect(),
        lossy_seen: 0,
    };
    let mut emails_unchecked: usize = 0;
    let unmapped: BTreeMap<&str, &str> = bundle
        .report
        .unmapped_recurrence
        .iter()
        .map(|u| (u.task.as_str(), u.reason.as_str()))
        .collect();
    let mut seen: usize = 0;

    for l in &export.lists {
        let before = rep.problems.len();
        let ll = crate::label(&l.list.id);
        let mut graph = Tally::default();
        let list_id = match by_source.get(&(SourceKind::List, l.list.id.as_str())) {
            Some(Entity::List(bl)) => {
                seen += 1;
                if bl.name != mapping::name(&l.list.display_name) {
                    rep.problems.push(format!("list {ll}: name differs"));
                }
                Some(bl.id)
            }
            _ => {
                rep.problems
                    .push(format!("list {ll}: missing from the bundle"));
                None
            }
        };
        for t in &l.tasks {
            let tl = crate::label(&t.id);
            let ch = l.children_of(&t.id);
            if t.is_completed() {
                graph.done += 1;
            } else {
                graph.open += 1;
            }
            graph.steps += ch.checklist_items.len();
            graph.links += ch.linked_resources.len();
            graph.attachments += ch.attachments.len();
            let Some(Entity::Task(bt)) = by_source.get(&(SourceKind::Task, t.id.as_str())) else {
                rep.problems
                    .push(format!("task {tl}: missing from the bundle"));
                continue;
            };
            seen += 1;
            if Some(bt.list) != list_id {
                rep.problems.push(format!("task {tl}: in the wrong list"));
            }
            const TASK: SourceKind = SourceKind::Task;
            let id = t.id.as_str();
            let p = &mut rep.problems;
            let created = required(Some(&t.created_date_time));
            let task_created = created.value().copied();
            dates.check(
                p,
                TASK,
                id,
                "createdDateTime",
                created,
                Some(&bt.created.at),
                false,
            );
            let updated = required(t.last_modified_date_time.as_deref());
            dates.check(
                p,
                TASK,
                id,
                "lastModifiedDateTime",
                updated,
                Some(&bt.updated.at),
                false,
            );
            let completed = if t.is_completed() {
                match optional_local(t.completed_date_time.as_ref()) {
                    Expect::Absent => Expect::Unreadable,
                    e => e,
                }
            } else {
                Expect::Absent
            };
            dates.check(
                p,
                TASK,
                id,
                "completedDateTime",
                completed,
                bt.completed_at.as_ref(),
                false,
            );
            let due = optional_local(t.due_date_time.as_ref()).map(|at| tz.to_datetime(at).date());
            let due_date = due.value().copied();
            dates.check(p, TASK, id, "dueDateTime", due, bt.due.as_ref(), true);
            let reminder = by_source
                .get(&(SourceKind::Reminder, id))
                .and_then(|e| match e {
                    Entity::Reminder(r) => Some(r),
                    _ => None,
                });
            let expect_reminder = if t.is_reminder_on {
                match optional_local(t.reminder_date_time.as_ref()) {
                    Expect::Absent => Expect::Unreadable,
                    e => e,
                }
            } else {
                Expect::Absent
            };
            dates.check(
                p,
                TASK,
                id,
                "reminderDateTime",
                expect_reminder,
                reminder.map(|r| &r.at),
                true,
            );
            if let Some(r) = reminder {
                seen += 1;
                if r.task != bt.id || r.created.at != bt.created.at {
                    p.push(format!("task {tl}: reminder differs"));
                }
            }

            let (notes, _) = crate::bundle::task_notes(t);
            let mut diff = |field: &str, same: bool| {
                if !same {
                    rep.problems.push(format!("task {tl}: {field} differs"));
                }
            };
            diff("title", bt.title == mapping::title(t.title.as_deref()));
            diff("status", bt.completed_at.is_some() == t.is_completed());
            diff(
                "importance",
                bt.priority == mapping::priority(&t.importance),
            );
            diff("notes", bt.notes == notes);
            diff("tags", bt.tags.len() == t.categories.len());
            match t.recurrence.as_ref().filter(|r| !r.is_null()) {
                None => diff("recurrence", bt.recurrence.is_none()),
                Some(raw) => match (mapping::recurrence(raw, due_date), &bt.recurrence) {
                    (Ok(r), Some(b)) => diff("recurrence", &r == b),
                    (Err(_), None) if unmapped.contains_key(id) => {}
                    _ => diff("recurrence", false),
                },
            }
            // Times derived from the task's own creation time.
            let derived = task_created.unwrap_or(bt.created.at);

            for s in &ch.checklist_items {
                let sl = crate::label(&s.id);
                let Some(Entity::Step(bs)) = by_source.get(&(SourceKind::Step, s.id.as_str()))
                else {
                    rep.problems
                        .push(format!("step {sl}: missing from the bundle"));
                    continue;
                };
                seen += 1;
                let ok = bs.task == bt.id
                    && bs.title == mapping::title(Some(&s.display_name))
                    && bs.completed_at.is_some() == s.is_checked;
                if !ok {
                    rep.problems.push(format!("step {sl}: differs"));
                }
                const STEP: SourceKind = SourceKind::Step;
                let p = &mut rep.problems;
                let created = required(Some(&s.created_date_time));
                dates.check(
                    p,
                    STEP,
                    &s.id,
                    "createdDateTime",
                    created,
                    Some(&bs.created.at),
                    false,
                );
                let checked = if s.is_checked {
                    required(s.checked_date_time.as_deref())
                } else {
                    Expect::Absent
                };
                dates.check(
                    p,
                    STEP,
                    &s.id,
                    "checkedDateTime",
                    checked,
                    bs.completed_at.as_ref(),
                    false,
                );
            }
            for lr in &ch.linked_resources {
                let kl = crate::label(&lr.id);
                let Some(Entity::Link(bl)) = by_source.get(&(SourceKind::Link, lr.id.as_str()))
                else {
                    rep.problems
                        .push(format!("link {kl}: missing from the bundle"));
                    continue;
                };
                seen += 1;
                let subject = lr.display_name.clone().unwrap_or_default();
                if let LinkKind::Email(m) = &bl.kind {
                    match cache {
                        Some(cache) => {
                            let received = lr
                                .external_id
                                .as_ref()
                                .and_then(|x| cache.emails.get(x))
                                .and_then(|m| m.received.as_deref());
                            dates.check(
                                &mut rep.problems,
                                SourceKind::Link,
                                &lr.id,
                                "receivedDateTime",
                                required(received),
                                Some(&m.received_at),
                                false,
                            );
                        }
                        None => {
                            emails_unchecked += 1;
                            if dates.lossy.contains(&(
                                SourceKind::Link,
                                lr.id.as_str(),
                                "receivedDateTime",
                            )) {
                                dates.lossy_seen += 1;
                            }
                        }
                    }
                }
                let ok = bl.task == bt.id
                    && bl.created.at == derived
                    && match &bl.kind {
                        LinkKind::Email(m) => {
                            m.subject == crate::bundle::clip_field(&subject)
                                && m.web_link == crate::bundle::web_url(lr)
                        }
                        LinkKind::Web { url, title } => {
                            Some(url) == crate::bundle::web_url(lr).as_ref()
                                && title.as_deref().unwrap_or("") == subject.trim()
                        }
                    };
                if !ok {
                    rep.problems.push(format!("link {kl}: differs"));
                }
            }
            for a in &ch.attachments {
                let al = crate::label(&a.id);
                let Some(Entity::Attachment(ba)) =
                    by_source.get(&(SourceKind::Attachment, a.id.as_str()))
                else {
                    rep.problems
                        .push(format!("attachment {al}: missing from the bundle"));
                    continue;
                };
                seen += 1;
                let content = export.attachment_content(a).ok().flatten();
                graph.attachment_bytes += content.as_ref().map_or(0, |c| c.len() as u64);
                let blob_ok = match (&content, bundle.blob(ba.id)) {
                    (Some(c), Some(b)) => {
                        let sha = hex::encode(Sha256::digest(c));
                        let decoded = base64::engine::general_purpose::STANDARD
                            .decode(&b.content_base64)
                            .ok();
                        b.sha256 == sha
                            && b.storage_key == ba.storage_key
                            && decoded.as_deref() == Some(c.as_slice())
                            && ba.size_bytes == c.len() as u64
                    }
                    (None, None) => true,
                    _ => false,
                };
                if ba.task != bt.id || ba.file_name != mapping::file_name(&a.name) || !blob_ok {
                    rep.problems.push(format!("attachment {al}: differs"));
                }
                dates.check(
                    &mut rep.problems,
                    SourceKind::Attachment,
                    &a.id,
                    "lastModifiedDateTime",
                    required(a.last_modified_date_time.as_deref()),
                    Some(&ba.created.at),
                    false,
                );
            }
        }
        let bundle_tally = list_id
            .and_then(|id| tally.get(&id).copied())
            .unwrap_or_default();
        if graph != bundle_tally {
            rep.problems.push(format!("list {ll}: counts differ"));
        }
        rep.rows.push(ListRow {
            label: ll,
            name: show_names.then(|| l.list.display_name.clone()),
            graph,
            bundle: bundle_tally,
            ok: rep.problems.len() == before,
        });
    }

    let structural = by_source
        .keys()
        .filter(|(k, _)| matches!(k, SourceKind::Group | SourceKind::Category))
        .count();
    if seen + structural != by_source.len() {
        rep.problems.push(format!(
            "the bundle has {} item(s) that are not in the export",
            by_source.len() - seen - structural
        ));
    }
    if dates.lossy_seen > 0 {
        rep.notes.push(format!(
            "{} date(s) could not be read and are listed as lossy in the bundle report",
            dates.lossy_seen
        ));
    }
    if dates.lossy_seen != bundle.report.lossy.len() {
        rep.problems.push(format!(
            "the bundle reports {} lossy date(s), but {} of them are unreadable in the export",
            bundle.report.lossy.len(),
            dates.lossy_seen
        ));
    }
    if emails_unchecked > 0 {
        rep.notes.push(format!(
            "{emails_unchecked} email link received time(s) come from the cache and were not checked; pass --cache to check them"
        ));
    }
    for u in &bundle.report.unmapped_recurrence {
        rep.notes.push(format!(
            "task {}: recurrence not imported ({})",
            crate::label(&u.task),
            u.reason
        ));
    }
    if bundle.counts.attachments > 0 {
        rep.notes.push(format!(
            "{} attachment(s): references are imported; content stays in the bundle until the server has an upload path",
            bundle.counts.attachments
        ));
    }
    rep
}
