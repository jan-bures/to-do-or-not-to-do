//! Android bindings for the shared organize and repeat intents.
use super::*;
use crate::labels::one_line;
use todo_client::intent::repeat::{self, Preset};
use todo_core::{
    ids::TagId,
    model::{LinkKind, Task},
    recurrence::Rule,
};

#[derive(Debug, uniffi::Object)]
pub struct OrganizeChange(TrackedCommand, Undone, String);

#[uniffi::export]
impl OrganizeChange {
    pub fn toast_text(&self) -> String {
        self.2.clone()
    }
}

#[derive(Clone, Debug, uniffi::Enum)]
pub enum ColorTarget {
    List { id: String },
    Collection { id: String },
    Tag { id: String },
}
impl ColorTarget {
    fn entity(&self) -> Result<EntityId, FfiError> {
        Ok(match self {
            Self::List { id } => list_id(id)?.into(),
            Self::Collection { id } => collection_id(id)?.into(),
            Self::Tag { id } => tag_id(id)?.into(),
        })
    }
}
fn tag_id(id: &str) -> Result<TagId, FfiError> {
    id.parse().map_err(|_| invalid("No such tag."))
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TagChoice {
    pub id: String,
    pub name: String,
    pub hue: String,
    pub selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RepeatUnit {
    Day,
    Week,
    Month,
    Year,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct CustomRepeat {
    pub unit: RepeatUnit,
    pub interval: u32,
    /// Monday is the least significant bit.
    pub weekdays: u8,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum RepeatChoice {
    Daily,
    Weekdays,
    Weekly,
    Monthly,
    Yearly,
    Custom { draft: CustomRepeat },
    Stop,
}
impl RepeatChoice {
    fn rule(self, t: &Task, today: Date) -> Option<Rule> {
        let anchor = t.due.unwrap_or(today);
        let preset = match self {
            Self::Daily => Preset::Daily,
            Self::Weekdays => Preset::Weekdays,
            Self::Weekly => Preset::Weekly,
            Self::Monthly => Preset::Monthly,
            Self::Yearly => Preset::Yearly,
            Self::Stop => return None,
            Self::Custom { draft } => return Some(draft.fields(t, today).rule()),
        };
        Some(preset.rule_for(anchor, t.recurrence.as_ref()))
    }
}
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct RepeatOption {
    pub choice: RepeatChoice,
    pub label: String,
    pub selected: bool,
}

impl CustomRepeat {
    fn fields(self, t: &Task, today: Date) -> repeat::RepeatDraft {
        let mut draft = repeat::RepeatDraft::new(t.due, t.recurrence.as_ref(), today);
        draft.unit = match self.unit {
            RepeatUnit::Day => repeat::Unit::Day,
            RepeatUnit::Week => repeat::Unit::Week,
            RepeatUnit::Month => repeat::Unit::Month,
            RepeatUnit::Year => repeat::Unit::Year,
        };
        draft.interval = self.interval;
        draft.days = std::array::from_fn(|i| self.weekdays & (1 << i) != 0);
        draft
    }
}
#[derive(Debug, uniffi::Record)]
pub struct RepeatPreview {
    pub description: String,
    pub due_note: Option<String>,
    pub valid: bool,
}

pub(crate) fn presets(t: &Task, today: Date) -> Vec<RepeatOption> {
    use RepeatChoice as C;
    repeat::presets(t.due, t.recurrence.as_ref(), today)
        .into_iter()
        .map(|(p, _, label, selected)| RepeatOption {
            choice: match p {
                Preset::Daily => C::Daily,
                Preset::Weekdays => C::Weekdays,
                Preset::Weekly => C::Weekly,
                Preset::Monthly => C::Monthly,
                Preset::Yearly => C::Yearly,
            },
            label,
            selected,
        })
        .collect()
}

pub(crate) fn draft(t: &Task, today: Date) -> CustomRepeat {
    let d = repeat::RepeatDraft::new(t.due, t.recurrence.as_ref(), today);
    CustomRepeat {
        unit: match d.unit {
            repeat::Unit::Day => RepeatUnit::Day,
            repeat::Unit::Week => RepeatUnit::Week,
            repeat::Unit::Month => RepeatUnit::Month,
            repeat::Unit::Year => RepeatUnit::Year,
        },
        interval: d.interval,
        weekdays: d
            .days
            .iter()
            .enumerate()
            .fold(0, |mask, (i, &on)| mask | (u8::from(on) << i)),
    }
}
#[derive(Debug, uniffi::Enum)]
pub enum OrganizeDeletion {
    Deleted { change: Arc<OrganizeChange> },
    Changed { question: Option<String> },
}

fn question(s: &State, id: EntityId) -> Option<String> {
    match id {
        EntityId::Collection(id) => intent::confirm::collection_question(s, id),
        EntityId::Tag(id) => intent::confirm::tag_question(s, id),
        _ => None,
    }
}
impl TodoEngine {
    fn track_change(
        &self,
        cmd: Option<Command>,
        what: &'static str,
        noun: &'static str,
        toast: String,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        Ok(cmd
            .map(|cmd| {
                self.client
                    .submit_tracked(cmd)
                    .map(|t| Arc::new(OrganizeChange(t, Undone::Change(what, noun), toast)))
            })
            .transpose()?)
    }
}
#[uniffi::export]
impl TodoEngine {
    pub fn tag_named(&self, name: String) -> bool {
        intent::tag_named(&self.state(), &name).is_some()
    }
    pub fn preview_repeat(&self, id: String, draft: CustomRepeat, now: LocalNow) -> RepeatPreview {
        let invalid = |description| RepeatPreview {
            description,
            due_note: None,
            valid: false,
        };
        let Ok((id, s)) = self.live_task(&id) else {
            return invalid("No such task.".into());
        };
        let t = &s.tasks[&id];
        let today = Now::new(now).today();
        match repeat::preview(
            draft.fields(t, today).rule(),
            t.due,
            t.recurrence.as_ref().and_then(|r| r.until),
            today,
        ) {
            Ok((description, first)) => RepeatPreview {
                description,
                due_note: first
                    .map(|day| format!("Due will be {}.", intent::quick_add::long_date(day))),
                valid: true,
            },
            Err(e) => invalid(repeat::error_text(e)),
        }
    }
    pub fn set_repeat(
        &self,
        id: String,
        choice: RepeatChoice,
        now: LocalNow,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let (id, s) = self.live_task(&id)?;
        let today = Now::new(now).today();
        let rule = choice.rule(&s.tasks[&id], today);
        let cmd =
            repeat::set_repeat(&s, id, rule, today).map_err(|e| invalid(repeat::error_text(e)))?;
        self.track_change(
            cmd,
            "Undo changing repeat",
            "task",
            format!("Changed schedule for “{}”", one_line(&s.tasks[&id].title)),
        )
    }
    pub fn undo_organize(&self, change: Arc<OrganizeChange>) -> Result<(), FfiError> {
        self.undo_tracked(&change.0, change.1)
    }
    pub fn add_web_link(
        &self,
        task: String,
        address: String,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let cmd = intent::add_link(&self.state(), task_id(&task)?, &address, || {
            self.client.new_uuid()
        })
        .map_err(|e| invalid(e.to_string()))?;
        let Command::AddLink {
            kind: LinkKind::Web { url, .. },
            ..
        } = &cmd
        else {
            return Err(invalid("Not a web link."));
        };
        let toast = format!("Added link “{}”", one_line(url));
        self.track_change(Some(cmd), "Undo adding a link", "link", toast)
    }
    pub fn remove_link(&self, id: String) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let s = self.state();
        let id = id.parse().map_err(|_| invalid("No such link."))?;
        let Some(link) = s.links.get(&id) else {
            return Ok(None);
        };
        let name = match &link.kind {
            LinkKind::Web { url, title } => title
                .as_ref()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(url),
            LinkKind::Email(m) => &m.subject,
        };
        self.track_change(
            intent::delete_link(&s, id),
            "Undo removing a link",
            "link",
            format!("Removed link “{}”", one_line(name)),
        )
    }
    pub fn toggle_tag(&self, task: String, id: String) -> Result<bool, FfiError> {
        self.submit_some(intent::toggle_tag(
            &self.state(),
            task_id(&task)?,
            tag_id(&id)?,
        ))
    }
    pub fn tag_task_named(&self, task: String, name: String) -> Result<(), FfiError> {
        let (_, cmds) = intent::tag_task_named(&self.state(), task_id(&task)?, &name, || {
            self.client.new_uuid()
        })
        .map_err(|e| invalid(e.to_string()))?;
        for cmd in cmds {
            self.client.submit(cmd)?;
        }
        Ok(())
    }
    pub fn rename_tag(&self, id: String, name: String) -> Result<bool, FfiError> {
        self.submit_some(
            intent::rename_tag(&self.state(), tag_id(&id)?, &name)
                .map_err(|e| invalid(e.to_string()))?,
        )
    }
    pub fn set_swatch(&self, target: ColorTarget, key: String) -> Result<bool, FfiError> {
        self.submit_some(intent::set_color(&self.state(), target.entity()?, &key))
    }
    pub fn create_collection(&self, name: String) -> Result<RoomKey, FfiError> {
        let id = CollectionId::from_uuid(self.client.new_uuid());
        self.client.submit(intent::create_collection(id, &name))?;
        Ok(RoomKey::Collection { id: id.to_string() })
    }
    pub fn move_list(
        &self,
        id: String,
        collection: Option<String>,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let s = self.state();
        let id = list_id(&id)?;
        let dest = collection.as_deref().map(collection_id).transpose()?;
        let Some(cmd) = intent::move_list(&s, id, dest) else {
            return Ok(None);
        };
        let list = &s.lists[&id];
        let place = if let Some(c) = dest {
            format!("to {}", one_line(&s.collections[&c].name))
        } else {
            list.collection
                .and_then(|c| s.collections.get(&c))
                .map_or_else(
                    || "out of its collection".into(),
                    |c| format!("out of {}", one_line(&c.name)),
                )
        };
        self.track_change(
            Some(cmd),
            "Undo moving a list",
            "list",
            format!("Moved “{}” {place}", one_line(&list.name)),
        )
    }
    pub fn organize_question(&self, target: ColorTarget) -> Option<String> {
        question(&self.state(), target.entity().ok()?)
    }
    pub fn confirm_organize_delete(
        &self,
        target: ColorTarget,
        asked: String,
    ) -> Result<OrganizeDeletion, FfiError> {
        let s = self.state();
        let id = target.entity()?;
        let current = question(&s, id);
        if current.as_deref() != Some(&asked) {
            return Ok(OrganizeDeletion::Changed { question: current });
        }
        let (cmd, what, noun, name) = match id {
            EntityId::Collection(c) => (
                intent::delete_collection(&s, c),
                "Undo deleting a collection",
                "collection",
                &s.collections[&c].name,
            ),
            EntityId::Tag(t) => (
                Some(intent::delete(t)),
                "Undo deleting a tag",
                "tag",
                &s.tags[&t].name,
            ),
            _ => return Err(invalid("Cannot delete this item.")),
        };
        let change = self
            .track_change(
                cmd,
                what,
                noun,
                format!("Deleted {noun} “{}”", one_line(name)),
            )?
            .ok_or_else(|| invalid("Already deleted."))?;
        Ok(OrganizeDeletion::Deleted { change })
    }
}
