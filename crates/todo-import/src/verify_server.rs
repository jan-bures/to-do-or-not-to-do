//! `verify-server`: reads everything back over REST and checks it against the bundle.
//!
//! For every imported entity the server must hold an equal, live entity. Stamps are compared without
//! their actor (the server records the caller) and position keys are not compared directly (the server
//! may have re-keyed an entity whose key was taken); instead the display order of the imported entities
//! must equal the bundle's. Inside imported lists and tasks, any entity the bundle does not have is
//! reported: that is how a duplicated import would show.

use crate::bundle::Bundle;
use crate::http::{Client, HttpError};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use todo_core::Actor;
use todo_core::ids::{CollectionId, EntityId, ListId, TaskId};
use todo_core::model::{
    AttachmentRef, Collection, Entity, Link, List, Reminder, Stamp, Step, Tag, Task,
};
use todo_core::ordering::PosKey;

#[derive(Deserialize)]
struct Page<T> {
    items: Vec<T>,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct Detail {
    task: Task,
    steps: Vec<Step>,
    links: Vec<Link>,
    attachments: Vec<AttachmentRef>,
    reminders: Vec<Reminder>,
    truncated: bool,
}

/// The result.
#[derive(Clone, Debug, Default)]
pub struct ServerReport {
    /// Entities found and equal.
    pub matched: usize,
    /// Entities checked.
    pub checked: usize,
    /// Attachments with content in the bundle whose reference is on the server.
    pub contents: usize,
    /// Of those, the ones whose content the server holds.
    pub contents_stored: usize,
    /// Every problem, naming objects by source label.
    pub problems: Vec<String>,
}

impl ServerReport {
    /// True if everything landed.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }

    /// The report as text.
    #[must_use]
    pub fn render(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "checked {} entities, {} match; {} attachment contents, {} stored",
            self.checked, self.matched, self.contents, self.contents_stored
        );
        for p in self.problems.iter().take(100) {
            let _ = writeln!(s, "MISMATCH: {p}");
        }
        if self.problems.len() > 100 {
            let _ = writeln!(s, "... and {} more", self.problems.len() - 100);
        }
        let _ = writeln!(
            s,
            "verify-server: {}",
            if self.ok() {
                "PASS".to_owned()
            } else {
                format!("FAIL ({} mismatches)", self.problems.len())
            }
        );
        s
    }
}

fn all<T: DeserializeOwned>(client: &Client, what: &str, path: &str) -> Result<Vec<T>, HttpError> {
    let mut out = Vec::new();
    let sep = if path.contains('?') { '&' } else { '?' };
    let mut cursor: Option<String> = None;
    loop {
        let url = match &cursor {
            None => format!("{path}{sep}limit=500"),
            Some(c) => format!("{path}{sep}limit=500&cursor={c}"),
        };
        let page: Page<T> = client.get(what, &url)?;
        out.extend(page.items);
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => return Ok(out),
        }
    }
}

/// An entity with the parts the server may legitimately change neutralised.
fn normal(e: &Entity) -> Entity {
    let blank = |s: &mut Stamp| s.by = Actor::Importer;
    let key = || PosKey::new("V").unwrap_or_else(|_| unreachable!());
    let mut e = e.clone();
    match &mut e {
        Entity::Collection(x) => {
            blank(&mut x.created);
            x.position = key();
        }
        Entity::List(x) => {
            blank(&mut x.created);
            x.position = key();
        }
        Entity::Task(x) => {
            blank(&mut x.created);
            blank(&mut x.updated);
            x.position = key();
        }
        Entity::Step(x) => {
            blank(&mut x.created);
            x.position = key();
        }
        Entity::Tag(x) => blank(&mut x.created),
        Entity::Link(x) => blank(&mut x.created),
        Entity::Attachment(x) => blank(&mut x.created),
        Entity::Reminder(x) => blank(&mut x.created),
    }
    e
}

struct Checker<'a> {
    bundle: &'a Bundle,
    /// Source label by entity ID.
    labels: BTreeMap<EntityId, String>,
    expected: BTreeMap<EntityId, &'a Entity>,
    found: BTreeSet<EntityId>,
    rep: ServerReport,
}

impl Checker<'_> {
    fn label(&self, id: EntityId) -> String {
        self.labels
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    fn compare(&mut self, got: Entity) {
        let id = got.id();
        let Some(want) = self.expected.get(&id) else {
            return;
        };
        self.found.insert(id);
        if got.deleted().is_some() {
            self.rep
                .problems
                .push(format!("{}: deleted on the server", self.label(id)));
        } else if normal(&got) != normal(want) {
            self.rep
                .problems
                .push(format!("{}: differs on the server", self.label(id)));
        } else {
            self.rep.matched += 1;
        }
    }

    /// Checks that the imported entities among `got` (in server order) are in bundle order, and, if
    /// `owned`, that there are no others.
    fn order(&mut self, scope: &str, got: &[EntityId], want: &[EntityId], owned: bool) {
        let imported: Vec<EntityId> = got
            .iter()
            .copied()
            .filter(|id| self.expected.contains_key(id))
            .collect();
        if imported != want && imported.len() == want.len() {
            self.rep.problems.push(format!("{scope}: order differs"));
        }
        if owned {
            let extra = got
                .iter()
                .filter(|id| !self.expected.contains_key(id))
                .count();
            if extra > 0 {
                self.rep.problems.push(format!(
                    "{scope}: {extra} entities the bundle does not have"
                ));
            }
        }
    }
}

/// Reads back everything the bundle imports and compares.
pub fn verify_server(bundle: &Bundle, client: &Client) -> Result<ServerReport, HttpError> {
    let mut c = Checker {
        bundle,
        labels: BTreeMap::new(),
        expected: BTreeMap::new(),
        found: BTreeSet::new(),
        rep: ServerReport::default(),
    };
    // Bundle order per scope.
    let mut collections: Vec<EntityId> = Vec::new();
    let mut lists: BTreeMap<Option<CollectionId>, Vec<EntityId>> = BTreeMap::new();
    let mut tasks: BTreeMap<ListId, Vec<EntityId>> = BTreeMap::new();
    let mut steps: BTreeMap<TaskId, Vec<EntityId>> = BTreeMap::new();
    let mut others: BTreeMap<TaskId, BTreeSet<EntityId>> = BTreeMap::new();
    for item in &c.bundle.items {
        let Some(e) = item.entity() else { continue };
        let id = e.id();
        c.labels.insert(
            id,
            format!(
                "{} {}",
                item.source.kind.as_str(),
                crate::label(&item.source.id)
            ),
        );
        c.expected.insert(id, e);
        match e {
            Entity::Collection(_) => collections.push(id),
            Entity::List(l) => lists.entry(l.collection).or_default().push(id),
            Entity::Task(t) => tasks.entry(t.list).or_default().push(id),
            Entity::Step(s) => steps.entry(s.task).or_default().push(id),
            Entity::Link(l) => {
                others.entry(l.task).or_default().insert(id);
            }
            Entity::Attachment(a) => {
                others.entry(a.task).or_default().insert(id);
            }
            Entity::Reminder(r) => {
                others.entry(r.task).or_default().insert(id);
            }
            Entity::Tag(_) => {}
        }
    }
    c.rep.checked = c.expected.len();

    let got: Vec<Collection> = all(client, "GET /api/v1/collections", "/api/v1/collections")?;
    let ids: Vec<EntityId> = got.iter().map(|x| x.id.into()).collect();
    c.order("collections", &ids, &collections, false);
    got.into_iter()
        .for_each(|x| c.compare(Entity::Collection(x)));

    let got: Vec<List> = all(client, "GET /api/v1/lists", "/api/v1/lists")?;
    for (scope, want) in &lists {
        let ids: Vec<EntityId> = got
            .iter()
            .filter(|l| l.collection == *scope)
            .map(|l| l.id.into())
            .collect();
        c.order("lists", &ids, want, false);
    }
    got.into_iter().for_each(|x| c.compare(Entity::List(x)));

    let got: Vec<Tag> = all(client, "GET /api/v1/tags", "/api/v1/tags")?;
    got.into_iter().for_each(|x| c.compare(Entity::Tag(x)));

    for (list, want) in &tasks {
        let scope = c.label((*list).into());
        let got: Vec<Task> = match all(
            client,
            "GET /api/v1/lists/{id}/tasks",
            &format!("/api/v1/lists/{list}/tasks"),
        ) {
            Ok(v) => v,
            // The list itself is missing; that, and each of its tasks, is reported below.
            Err(HttpError::Status { status: 404, .. }) => continue,
            Err(e) => return Err(e),
        };
        let ids: Vec<EntityId> = got.iter().map(|x| x.id.into()).collect();
        c.order(&format!("tasks of {scope}"), &ids, want, true);
        for t in got {
            let tid = t.id;
            if !c.expected.contains_key(&tid.into()) {
                continue;
            }
            c.compare(Entity::Task(t));
            let d: Detail =
                client.get("GET /api/v1/tasks/{id}", &format!("/api/v1/tasks/{tid}"))?;
            let tl = c.label(tid.into());
            if d.truncated {
                c.rep
                    .problems
                    .push(format!("{tl}: detail truncated by the server"));
            }
            let _ = d.task;
            let sids: Vec<EntityId> = d.steps.iter().map(|s| s.id.into()).collect();
            c.order(
                &format!("steps of {tl}"),
                &sids,
                steps.get(&tid).map_or(&[][..], Vec::as_slice),
                true,
            );
            d.steps.into_iter().for_each(|x| c.compare(Entity::Step(x)));
            let mut rest: BTreeSet<EntityId> = BTreeSet::new();
            rest.extend(d.links.iter().map(|x| EntityId::from(x.id)));
            rest.extend(d.attachments.iter().map(|x| EntityId::from(x.id)));
            rest.extend(d.reminders.iter().map(|x| EntityId::from(x.id)));
            let want = others.get(&tid).cloned().unwrap_or_default();
            let extra = rest.difference(&want).count();
            if extra > 0 {
                c.rep.problems.push(format!(
                    "{tl}: {extra} links, attachments or reminders the bundle does not have"
                ));
            }
            d.links.into_iter().for_each(|x| c.compare(Entity::Link(x)));
            d.attachments
                .into_iter()
                .for_each(|x| c.compare(Entity::Attachment(x)));
            d.reminders
                .into_iter()
                .for_each(|x| c.compare(Entity::Reminder(x)));
        }
    }

    let missing: Vec<EntityId> = c
        .expected
        .keys()
        .copied()
        .filter(|id| !c.found.contains(id))
        .collect();
    for id in missing {
        let l = c.label(id);
        c.rep.problems.push(format!("{l}: not on the server"));
    }

    // Content: a HEAD of the download route fetches nothing and charges nothing on the server.
    for blob in &c.bundle.blobs {
        let id = EntityId::from(blob.attachment);
        if !c.found.contains(&id) {
            continue; // Reported above.
        }
        c.rep.contents += 1;
        let l = c.label(id);
        match client.head(
            "HEAD /api/v1/attachments/{id}",
            &format!("/api/v1/attachments/{}", blob.attachment),
        )? {
            Some(len) if len == blob.size_bytes => c.rep.contents_stored += 1,
            Some(len) => c.rep.problems.push(format!(
                "{l}: the server's content has {len} bytes, the bundle's {}",
                blob.size_bytes
            )),
            None => c.rep.problems.push(format!(
                "{l}: content missing on the server (run apply --attachments)"
            )),
        }
    }
    Ok(c.rep)
}
