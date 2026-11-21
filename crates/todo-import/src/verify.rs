//! `verify`: compares a bundle with the raw Graph export, list by list and field by field.
//!
//! The expected values are recomputed from the raw Graph JSON with the conversions in
//! [`crate::mapping`]; they are compared with the entities in the bundle. Checked per task: title,
//! status, completion time, due date, importance, notes, steps (title, checked, checked time), links
//! (subject or URL), attachments (name, size, content hash), reminders and recurrence. Order comes from
//! the cache and cannot be checked against Graph.

use crate::bundle::{Bundle, SourceKind, task_fields};
use crate::graph::Export;
use crate::mapping;
use base64::Engine;
use jiff::tz::TimeZone;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
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

/// Compares `bundle` with `export`.
#[must_use]
pub fn verify(export: &Export, bundle: &Bundle, show_names: bool) -> VerifyReport {
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

    let fallback = export
        .lists
        .iter()
        .flat_map(|l| &l.tasks)
        .filter_map(|t| mapping::instant(&t.created_date_time).ok())
        .min()
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
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
            let mut ignored = Vec::new();
            let f = task_fields(t, &tz, fallback, &mut ignored);
            let mut diff = |field: &str, same: bool| {
                if !same {
                    rep.problems.push(format!("task {tl}: {field} differs"));
                }
            };
            diff("title", bt.title == f.title);
            diff("status", bt.completed_at.is_some() == t.is_completed());
            diff("completion time", bt.completed_at == f.completed_at);
            diff("due date", bt.due == f.due);
            diff("importance", bt.priority == f.priority);
            diff("notes", bt.notes == f.notes);
            diff("creation time", bt.created.at == f.created);
            diff("modification time", bt.updated.at == f.updated);
            diff("tags", bt.tags.len() == t.categories.len());
            match t.recurrence.as_ref().filter(|r| !r.is_null()) {
                None => diff("recurrence", bt.recurrence.is_none()),
                Some(raw) => match (mapping::recurrence(raw, f.due), &bt.recurrence) {
                    (Ok(r), Some(b)) => diff("recurrence", &r == b),
                    (Err(_), None) if unmapped.contains_key(t.id.as_str()) => {}
                    _ => diff("recurrence", false),
                },
            }
            match (t.is_reminder_on, &t.reminder_date_time) {
                (true, Some(r)) => match by_source.get(&(SourceKind::Reminder, t.id.as_str())) {
                    Some(Entity::Reminder(br)) => {
                        seen += 1;
                        diff(
                            "reminder",
                            br.task == bt.id && mapping::date_time(r).ok() == Some(br.at),
                        );
                    }
                    _ => diff("reminder", false),
                },
                _ => diff(
                    "reminder",
                    !by_source.contains_key(&(SourceKind::Reminder, t.id.as_str())),
                ),
            }

            for s in &ch.checklist_items {
                let sl = crate::label(&s.id);
                let Some(Entity::Step(bs)) = by_source.get(&(SourceKind::Step, s.id.as_str()))
                else {
                    rep.problems
                        .push(format!("step {sl}: missing from the bundle"));
                    continue;
                };
                seen += 1;
                let checked_at = s
                    .checked_date_time
                    .as_deref()
                    .and_then(|c| mapping::instant(c).ok());
                let ok = bs.task == bt.id
                    && bs.title == mapping::title(Some(&s.display_name))
                    && bs.completed_at.is_some() == s.is_checked
                    && (!s.is_checked || checked_at.is_none() || bs.completed_at == checked_at);
                if !ok {
                    rep.problems.push(format!("step {sl}: differs"));
                }
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
                let ok = bl.task == bt.id
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
