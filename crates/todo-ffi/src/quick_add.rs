//! Value types for the Android composer's shared draft and calendar.

use todo_client::intent::{calendar, quick_add};
use todo_core::{model::Priority, time::Date};

use crate::engine::workflow::{ListFeatures, OpenStage};
use crate::labels::{LocalNow, Now};

#[derive(Clone, Debug, uniffi::Record)]
pub struct QuickAddDraft {
    pub text: String,
    pub picked_due: Option<String>,
    pub cleared_due: bool,
    pub picked_priority: Option<u8>,
}

impl From<QuickAddDraft> for quick_add::Draft {
    fn from(d: QuickAddDraft) -> Self {
        Self {
            text: d.text,
            due: if d.cleared_due {
                quick_add::DueChoice::Clear
            } else {
                d.picked_due
                    .and_then(|date| date.parse().ok())
                    .map_or(quick_add::DueChoice::Default, quick_add::DueChoice::Date)
            },
            priority: d.picked_priority.and_then(priority),
            ..Self::default()
        }
    }
}

impl From<quick_add::Draft> for QuickAddDraft {
    fn from(d: quick_add::Draft) -> Self {
        Self {
            text: d.text,
            picked_due: match d.due {
                quick_add::DueChoice::Date(date) => Some(date.to_string()),
                _ => None,
            },
            cleared_due: d.due == quick_add::DueChoice::Clear,
            picked_priority: d.priority.map(|p| p as u8),
        }
    }
}

fn priority(level: u8) -> Option<Priority> {
    Some(match level {
        0 => Priority::None,
        1 => Priority::Low,
        2 => Priority::Normal,
        3 => Priority::High,
        _ => return None,
    })
}

#[derive(Debug, uniffi::Record)]
pub struct QuickAddPreview {
    pub due: Option<String>,
    pub due_label: Option<String>,
    pub priority: Option<u8>,
}

#[uniffi::export]
pub fn quick_add_preview(draft: QuickAddDraft, now: LocalNow) -> QuickAddPreview {
    let now = Now::new(now);
    let parsed = quick_add::Draft::from(draft).parsed(&now.eval());
    QuickAddPreview {
        due: parsed.due.map(|date| date.to_string()),
        due_label: parsed.due.map(|date| now.dates().weekday(date)),
        priority: parsed.priority.map(|level| level as u8),
    }
}

#[uniffi::export]
pub fn quick_add_edit(draft: QuickAddDraft, text: String, now: LocalNow) -> QuickAddDraft {
    let mut draft = quick_add::Draft::from(draft);
    draft.edit(text, &Now::new(now).eval());
    draft.into()
}

#[uniffi::export]
pub fn quick_add_pick_due(
    draft: QuickAddDraft,
    date: Option<String>,
    now: LocalNow,
) -> QuickAddDraft {
    let date = match date {
        Some(iso) => match iso.parse() {
            Ok(date) => Some(date),
            Err(_) => return draft,
        },
        None => None,
    };
    let mut draft = quick_add::Draft::from(draft);
    draft.pick_due(date, &Now::new(now).eval());
    draft.into()
}

#[uniffi::export]
pub fn quick_add_pick_priority(draft: QuickAddDraft, level: u8, now: LocalNow) -> QuickAddDraft {
    let Some(level) = priority(level) else {
        return draft;
    };
    let mut draft = quick_add::Draft::from(draft);
    draft.pick_priority(level, &Now::new(now).eval());
    draft.into()
}

/// Feature-aware draft for workflow and start-date composers.
#[derive(Clone, Debug, uniffi::Record)]
pub struct WorkflowQuickAddDraft {
    pub base: QuickAddDraft,
    pub start_date: Option<String>,
    pub status: Option<OpenStage>,
}

impl From<WorkflowQuickAddDraft> for quick_add::Draft {
    fn from(d: WorkflowQuickAddDraft) -> Self {
        Self {
            start_date: d.start_date.and_then(|d| d.parse().ok()),
            status: d.status.map(Into::into),
            ..d.base.into()
        }
    }
}

impl From<quick_add::Draft> for WorkflowQuickAddDraft {
    fn from(d: quick_add::Draft) -> Self {
        Self {
            start_date: d.start_date.map(|d| d.to_string()),
            status: d.status.map(Into::into),
            base: d.into(),
        }
    }
}

#[derive(Debug, uniffi::Record)]
pub struct WorkflowQuickAddPreview {
    pub title: String,
    pub due: Option<String>,
    pub date_label: Option<String>,
    pub start_date: Option<String>,
    pub priority: Option<u8>,
    pub status: Option<OpenStage>,
}

#[uniffi::export]
pub fn workflow_quick_add_preview(
    draft: WorkflowQuickAddDraft,
    now: LocalNow,
    features: ListFeatures,
) -> WorkflowQuickAddPreview {
    let now = Now::new(now);
    let parsed = quick_add::Draft::from(draft).parsed_with_features(&now.eval(), features.into());
    WorkflowQuickAddPreview {
        title: parsed.title,
        due: parsed.due.map(|d| d.to_string()),
        date_label: quick_add::date_preview(parsed.start_date, parsed.due, now.dates()),
        start_date: parsed.start_date.map(|d| d.to_string()),
        priority: parsed.priority.map(|p| p as u8),
        status: parsed.status.map(Into::into),
    }
}

#[uniffi::export]
pub fn workflow_quick_add_edit(
    draft: WorkflowQuickAddDraft,
    text: String,
    now: LocalNow,
    features: ListFeatures,
) -> WorkflowQuickAddDraft {
    let mut draft = quick_add::Draft::from(draft);
    draft.edit_with_features(text, &Now::new(now).eval(), features.into());
    draft.into()
}

#[derive(Debug, uniffi::Enum)]
pub enum WorkflowQuickAddPick {
    Due { date: Option<String> },
    Start { date: Option<String> },
    Status { status: Option<OpenStage> },
    Priority { level: u8 },
}

#[uniffi::export]
pub fn workflow_quick_add_pick(
    draft: WorkflowQuickAddDraft,
    pick: WorkflowQuickAddPick,
    now: LocalNow,
    features: ListFeatures,
) -> Result<WorkflowQuickAddDraft, crate::FfiError> {
    let parse = |date: Option<String>| {
        date.map(|d| d.parse::<Date>())
            .transpose()
            .map_err(|_| crate::engine::invalid("That isn't a date."))
    };
    let pick = match pick {
        WorkflowQuickAddPick::Due { date } => quick_add::Pick::Due(parse(date)?),
        WorkflowQuickAddPick::Start { date } => quick_add::Pick::Start(parse(date)?),
        WorkflowQuickAddPick::Status { status } => quick_add::Pick::Status(status.map(Into::into)),
        WorkflowQuickAddPick::Priority { level } => quick_add::Pick::Priority(
            priority(level).ok_or_else(|| crate::engine::invalid("That isn't a priority."))?,
        ),
    };
    let mut draft = quick_add::Draft::from(draft);
    draft.pick(pick, &Now::new(now).eval(), features.into());
    Ok(draft.into())
}

#[derive(Debug, PartialEq, Eq, uniffi::Record)]
pub struct QuickAddPreset {
    pub label: String,
    pub date: Option<String>,
}

#[uniffi::export]
pub fn quick_add_presets(now: LocalNow) -> Vec<QuickAddPreset> {
    presets(Now::new(now).today())
}

fn presets(today: Date) -> Vec<QuickAddPreset> {
    calendar::presets(today)
        .into_iter()
        .map(|preset| QuickAddPreset {
            label: preset.label.into(),
            date: preset.date.map(|d| d.to_string()),
        })
        .collect()
}

#[derive(Debug, uniffi::Record)]
pub struct QuickAddDay {
    pub date: String,
    pub day: String,
    pub label: String,
    pub today: bool,
    pub selected: bool,
}

#[derive(Debug, uniffi::Record)]
pub struct QuickAddMonth {
    pub label: String,
    pub cursor: String,
    pub weekdays: Vec<String>,
    pub rows: Vec<Vec<Option<QuickAddDay>>>,
    pub previous: Option<String>,
    pub next: Option<String>,
    pub presets: Vec<QuickAddPreset>,
}

/// `cursor` is a day of the requested month, not necessarily the selected date.
#[uniffi::export]
pub fn quick_add_month(
    now: LocalNow,
    selected: Option<String>,
    cursor: Option<String>,
) -> QuickAddMonth {
    let dates = Now::new(now).dates();
    let today = dates.today;
    let selected = selected.and_then(|iso| iso.parse::<Date>().ok());
    let month = calendar::Month::new(
        cursor
            .and_then(|iso| iso.parse().ok())
            .or(selected)
            .unwrap_or(today),
    );
    QuickAddMonth {
        label: dates.month(month.cursor),
        cursor: month.cursor.to_string(),
        weekdays: month.weekdays().into_iter().map(str::to_owned).collect(),
        rows: month
            .rows()
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|date| {
                        date.map(|date| QuickAddDay {
                            date: date.to_string(),
                            day: date.day().to_string(),
                            label: calendar::day_label(date),
                            today: date == today,
                            selected: Some(date) == selected,
                        })
                    })
                    .collect()
            })
            .collect(),
        previous: month.shifted(-1).map(|m| m.cursor.to_string()),
        next: month.shifted(1).map(|m| m.cursor.to_string()),
        presets: presets(today),
    }
}

/// Shared warning for the detail calendar of a repeating task.
#[uniffi::export]
pub fn calendar_repeat_clear_note() -> String {
    calendar::REPEAT_CLEAR_NOTE.into()
}

/// Move the calendar's keyboard cursor by a day or week, including across a month boundary.
#[uniffi::export]
pub fn calendar_move_days(cursor: String, days: i32) -> Option<String> {
    let mut month = calendar::Month::new(cursor.parse().ok()?);
    month.move_days(i64::from(days));
    Some(month.cursor.to_string())
}

#[cfg(test)]
#[test]
fn calendar_cursor_crosses_month_boundaries() {
    assert_eq!(
        calendar_move_days("2026-11-30".into(), 1).as_deref(),
        Some("2026-12-01")
    );
    assert_eq!(
        calendar_move_days("2026-12-01".into(), -7).as_deref(),
        Some("2026-11-24")
    );
    assert_eq!(calendar_move_days("not a date".into(), 1), None);
}

#[cfg(test)]
#[test]
fn standalone_presets_match_the_calendar_in_device_time() {
    let now = LocalNow {
        epoch_ms: 0,
        utc_offset_secs: -18_000,
        zone_id: "America/New_York".into(),
        local_day: None,
        date_format: None,
    };
    let calendar = quick_add_month(now.clone(), None, Some("1971-01-01".into()));
    let presets = quick_add_presets(now);
    assert_eq!(presets, calendar.presets);
    assert_eq!(presets[0].date.as_deref(), Some("1969-12-31"));
    assert_eq!(presets[1].date.as_deref(), Some("1970-01-01"));
}

#[derive(Debug, uniffi::Record)]
pub struct QuickAddPriority {
    pub level: u8,
    pub label: String,
    pub mark: String,
}

#[uniffi::export]
pub fn quick_add_priorities() -> Vec<QuickAddPriority> {
    quick_add::PRIORITIES
        .into_iter()
        .map(|p| QuickAddPriority {
            level: p.level as u8,
            label: p.label.into(),
            mark: p.mark.into(),
        })
        .collect()
}
