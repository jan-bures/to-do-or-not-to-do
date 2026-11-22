//! Android bindings for the shared organize and repeat intents.
use super::*;
use todo_client::intent::repeat::{self, Preset};
use todo_core::{ids::TagId, model::Task, recurrence::Rule, time::Weekday};

#[derive(Debug, uniffi::Object)]
pub struct OrganizeChange(TrackedCommand, Undone);

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
            Self::Custom {
                draft:
                    CustomRepeat {
                        unit,
                        interval,
                        weekdays,
                    },
            } => {
                return Some(match unit {
                    RepeatUnit::Day => Rule::Daily { interval },
                    RepeatUnit::Year => Rule::Yearly { interval },
                    RepeatUnit::Week => Rule::Weekly {
                        interval,
                        days: Weekday::ALL
                            .into_iter()
                            .filter(|d| weekdays & (1 << d.offset()) != 0)
                            .collect(),
                    },
                    RepeatUnit::Month => {
                        let Rule::Monthly { day, .. } =
                            Preset::Monthly.rule_for(anchor, t.recurrence.as_ref())
                        else {
                            unreachable!()
                        };
                        Rule::Monthly { interval, day }
                    }
                });
            }
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

#[uniffi::export]
pub fn max_repeat_interval() -> u32 {
    todo_core::recurrence::MAX_INTERVAL
}

pub(crate) fn presets(t: &Task, today: Date) -> Vec<RepeatOption> {
    use RepeatChoice as C;
    [C::Daily, C::Weekdays, C::Weekly, C::Monthly, C::Yearly]
        .into_iter()
        .map(|choice| {
            let rule = choice.rule(t, today);
            let label = match (choice, &rule) {
                (C::Daily, _) => "Daily".into(),
                (C::Weekdays, _) => "Weekdays".into(),
                (C::Weekly, _) => format!(
                    "Weekly on {}",
                    crate::labels::weekday(t.due.unwrap_or(today))
                ),
                (C::Monthly, Some(Rule::Monthly { day, .. })) => format!("Monthly on day {day}"),
                _ => "Yearly".into(),
            };
            RepeatOption {
                choice,
                label,
                selected: t
                    .recurrence
                    .as_ref()
                    .is_some_and(|r| Some(&r.rule) == rule.as_ref()),
            }
        })
        .collect()
}

pub(crate) fn draft(t: &Task, today: Date) -> CustomRepeat {
    let mut weekdays = 1 << Weekday::of(t.due.unwrap_or(today)).offset();
    let (unit, interval) = match t.recurrence.as_ref().map(|r| &r.rule) {
        None => (RepeatUnit::Day, 1),
        Some(Rule::Daily { interval }) => (RepeatUnit::Day, *interval),
        Some(Rule::Weekly { interval, days }) => {
            weekdays = days.iter().fold(0, |mask, d| mask | (1 << d.offset()));
            (RepeatUnit::Week, *interval)
        }
        Some(Rule::Monthly { interval, .. }) => (RepeatUnit::Month, *interval),
        Some(Rule::Yearly { interval }) => (RepeatUnit::Year, *interval),
    };
    CustomRepeat {
        unit,
        interval,
        weekdays,
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
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        Ok(cmd
            .map(|cmd| {
                self.client
                    .submit_tracked(cmd)
                    .map(|t| Arc::new(OrganizeChange(t, Undone::Change(what, noun))))
            })
            .transpose()?)
    }
}
#[uniffi::export]
impl TodoEngine {
    pub fn set_repeat(
        &self,
        id: String,
        choice: RepeatChoice,
        now: LocalNow,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let (id, s) = self.live_task(&id)?;
        let today = Now::new(now).today();
        let rule = choice.rule(&s.tasks[&id], today);
        let cmd = repeat::set_repeat(&s, id, rule, today).map_err(|e| invalid(e.to_string()))?;
        self.track_change(cmd, "Undo changing repeat", "task")
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
        self.track_change(Some(cmd), "Undo adding a link", "link")
    }
    pub fn remove_link(&self, id: String) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let cmd = intent::delete_link(
            &self.state(),
            id.parse().map_err(|_| invalid("No such link."))?,
        );
        self.track_change(cmd, "Undo removing a link", "link")
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
        let cmd = intent::move_list(
            &self.state(),
            list_id(&id)?,
            collection.as_deref().map(collection_id).transpose()?,
        );
        self.track_change(cmd, "Undo moving a list", "list")
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
        let (cmd, what, noun) = match id {
            EntityId::Collection(c) => (
                intent::delete_collection(&s, c),
                "Undo deleting a collection",
                "collection",
            ),
            EntityId::Tag(t) => (Some(intent::delete(t)), "Undo deleting a tag", "tag"),
            _ => return Err(invalid("Cannot delete this item.")),
        };
        let change = self
            .track_change(cmd, what, noun)?
            .ok_or_else(|| invalid("Already deleted."))?;
        Ok(OrganizeDeletion::Deleted { change })
    }
}
