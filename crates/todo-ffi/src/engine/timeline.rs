//! Compose-facing adapters for the shared timeline projection, axis and scheduling intent.

use std::{collections::BTreeSet, sync::Arc};

use todo_client::{
    date_format::DateLabels,
    intent::workflow::{self, Dates, Schedule},
    timeline::{self, Axis, SpanKind, Zoom},
};
use todo_core::{
    query::QueryStatus,
    time::{Date, Weekday, days_between},
};

use super::{
    TodoEngine, Undone, invalid, list_id, organize::OrganizeChange, task_id,
    workflow::WorkflowStage,
};
use crate::{
    FfiError,
    labels::{LocalNow, Now, one_line},
    view::{self, Chip, RoomKey, RowPlacement},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TimelineBar {
    Range,
    Milestone,
    OpenEnded,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, uniffi::Record)]
pub struct TimelineDates {
    pub start: Option<String>,
    pub due: Option<String>,
}

impl TimelineDates {
    fn parse(self) -> Result<Dates, FfiError> {
        Ok(Dates {
            start: self.start.map(parse_date).transpose()?,
            due: self.due.map(parse_date).transpose()?,
        })
    }
}

fn parse_date(date: String) -> Result<Date, FfiError> {
    date.parse().map_err(|_| invalid("That isn't a date."))
}

impl From<Dates> for TimelineDates {
    fn from(dates: Dates) -> Self {
        Self {
            start: dates.start.map(|d| d.to_string()),
            due: dates.due.map(|d| d.to_string()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TimelineSpan {
    /// Day offsets relative to today. Both ends are inclusive.
    pub first: i32,
    pub last: i32,
    pub bar: TimelineBar,
    pub label: String,
}

fn offset(today: Date, date: Date) -> i32 {
    // The entire supported calendar is less than i32::MAX days wide.
    days_between(today, date) as i32
}

fn span(dates: Dates, today: Date, labels: DateLabels) -> Option<TimelineSpan> {
    let span = timeline::Span::from_dates(dates.start, dates.due)?;
    Some(TimelineSpan {
        first: offset(today, span.first),
        last: offset(today, span.last),
        bar: match span.kind {
            SpanKind::Range => TimelineBar::Range,
            SpanKind::Milestone => TimelineBar::Milestone,
            SpanKind::OpenEnded => TimelineBar::OpenEnded,
        },
        label: match (dates.start, dates.due) {
            (Some(start), Some(due)) if start != due => labels.range(start, due),
            (Some(start), None) => format!("Starts {}", labels.date(start)),
            _ => labels.date(span.first),
        },
    })
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TimelineTask {
    pub task_id: String,
    pub list_id: String,
    pub title: String,
    pub chip: Option<Chip>,
    pub stage: WorkflowStage,
    pub repeats: bool,
    pub dates: TimelineDates,
    pub span: Option<TimelineSpan>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct TimelineView {
    pub lanes: Vec<TimelineTask>,
    /// None in Planned. An empty list means the room supports the tray but currently has no items.
    pub unscheduled: Option<Vec<TimelineTask>>,
}

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TimelineEdit {
    Move { days: i32 },
    StartEdge { days: i32 },
    DueEdge { days: i32 },
    Place { day: i32 },
    PickStart { date: Option<String> },
    PickDue { date: Option<String> },
}

impl TimelineEdit {
    fn schedule(self, today: Date) -> Option<Schedule> {
        let pick = |date: Option<String>| date.map(|s| s.parse::<Date>()).transpose().ok();
        Some(match self {
            Self::Move { days } => Schedule::Move(days.into()),
            Self::StartEdge { days } => Schedule::StartEdge(days.into()),
            Self::DueEdge { days } => Schedule::DueEdge(days.into()),
            Self::Place { day } => Schedule::Place(todo_core::time::add_days(today, day.into())?),
            Self::PickStart { date } => Schedule::PickStart(pick(date)?),
            Self::PickDue { date } => Schedule::PickDue(pick(date)?),
        })
    }
}

#[uniffi::export]
pub fn timeline_preview(
    before: TimelineDates,
    edit: TimelineEdit,
    now: LocalNow,
) -> Option<TimelineSpan> {
    let now = Now::new(now);
    let dates = edit.schedule(now.today())?.preview(before.parse().ok()?)?;
    span(dates, now.today(), now.dates())
}

#[uniffi::export]
impl TodoEngine {
    pub fn timeline_available(&self, room: RoomKey) -> bool {
        view::shared_key(&room).is_some_and(|key| timeline::available(&self.client.view(), key))
    }

    pub fn timeline(&self, room: RoomKey, now: LocalNow) -> Option<TimelineView> {
        self.timeline_placed(room, String::new(), false, RowPlacement::default(), now)
    }

    pub fn timeline_placed(
        &self,
        room: RoomKey,
        filter: String,
        show_completed: bool,
        placement: RowPlacement,
        now: LocalNow,
    ) -> Option<TimelineView> {
        let key = view::shared_key(&room)?;
        let mut state = self.client.view();
        if !placement.is_empty() {
            state = Arc::new(view::placed(&state, &placement));
        }
        let now = Now::new(now);
        let ctx = now.eval();
        let timeline = timeline::room(&state, key, &ctx)?;
        let planned = matches!(room, RoomKey::Smart { .. });
        let cross_list = !matches!(room, RoomKey::List { .. });
        let allowed: BTreeSet<_> = view::filter(
            &state,
            &filter,
            &ctx,
            timeline
                .lanes
                .iter()
                .map(|l| l.task)
                .chain(timeline.unscheduled.iter().copied())
                .filter_map(|id| state.tasks.get(&id))
                .collect(),
        )
        .into_iter()
        .map(|task| task.id)
        .collect();
        let task = |id, stage: QueryStatus| {
            let task = state.tasks.get(&id)?;
            if !allowed.contains(&id) {
                return None;
            }
            let dates = Dates::of(task);
            Some(TimelineTask {
                task_id: task.id.to_string(),
                list_id: task.list.to_string(),
                title: one_line(&task.title),
                chip: if cross_list {
                    view::list_chip(&state, task.list)
                } else {
                    None
                },
                stage: stage.into(),
                repeats: task.recurrence.is_some(),
                dates: dates.into(),
                span: span(dates, now.today(), now.dates()),
            })
        };
        Some(TimelineView {
            lanes: timeline
                .lanes
                .into_iter()
                .filter(|lane| show_completed || lane.stage != QueryStatus::Done)
                .filter_map(|lane| task(lane.task, lane.stage))
                .collect(),
            unscheduled: (!planned).then(|| {
                timeline
                    .unscheduled
                    .into_iter()
                    .filter_map(|id| {
                        let current = state.tasks.get(&id)?;
                        (!current.is_done())
                            .then(|| task(id, QueryStatus::of(current)))
                            .flatten()
                    })
                    .collect()
            }),
        })
    }

    /// Commit the same edit that produced the preview, checked again under the outbox write lock.
    pub fn schedule_timeline_task(
        &self,
        room: RoomKey,
        list: String,
        id: String,
        before: TimelineDates,
        edit: TimelineEdit,
        today: String,
    ) -> Result<Option<Arc<OrganizeChange>>, FfiError> {
        let Some(room) = view::shared_key(&room) else {
            return Ok(None);
        };
        let (list, id, before) = (list_id(&list)?, task_id(&id)?, before.parse()?);
        let Some(edit) = edit.schedule(parse_date(today)?) else {
            return Ok(None);
        };
        let schedule = |state: &todo_core::State| {
            let task = state.tasks.get(&id)?;
            (task.list == list)
                .then(|| workflow::schedule(state, room, id, before, edit))
                .flatten()
        };
        let Some(cmd) = schedule(&self.client.view()) else {
            return Ok(None);
        };
        let tracked = self
            .client
            .submit_tracked_if(cmd.clone(), |state| schedule(state).as_ref() == Some(&cmd))?;
        Ok(tracked.map(|t| {
            Arc::new(OrganizeChange::new(
                t,
                Undone::Change("Undo rescheduling a task", "task"),
                "Schedule changed".into(),
            ))
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum TimelineZoom {
    Week,
    TwoWeeks,
    Month,
    Quarter,
}

impl From<TimelineZoom> for Zoom {
    fn from(value: TimelineZoom) -> Self {
        match value {
            TimelineZoom::Week => Self::Week,
            TimelineZoom::TwoWeeks => Self::TwoWeeks,
            TimelineZoom::Month => Self::Month,
            TimelineZoom::Quarter => Self::Quarter,
        }
    }
}

#[derive(Debug, uniffi::Record)]
pub struct TimelineScale {
    pub days: i32,
    pub today_offset: i32,
    pub min_offset: i32,
    pub max_offset: i32,
}

#[uniffi::export]
pub fn timeline_scale(zoom: TimelineZoom, today: String) -> Result<TimelineScale, FfiError> {
    let today = parse_date(today)?;
    let zoom = Zoom::from(zoom);
    Ok(TimelineScale {
        days: zoom.days() as i32,
        today_offset: offset(today, zoom.around_today(today)),
        min_offset: offset(today, Date::MIN),
        max_offset: offset(today, Date::MAX) + 1,
    })
}

#[derive(Debug, uniffi::Record)]
pub struct TimelineColumn {
    pub day: i32,
    pub days: i32,
    pub label: String,
    pub month: String,
    pub week: String,
    /// Calendar days through Sunday, including this column's first day.
    pub week_remaining: i32,
    pub weekend: bool,
}

/// Unclipped ruler columns for the visible day offsets; the Compose viewport clips partial cells.
#[uniffi::export]
pub fn timeline_columns(
    zoom: TimelineZoom,
    from: i32,
    through: i32,
    now: LocalNow,
) -> Vec<TimelineColumn> {
    let now = Now::new(now);
    let today = now.today();
    let zoom = Zoom::from(zoom);
    let axis = Axis {
        epoch: today,
        zoom,
        offset: 0.,
        width: zoom.days() as f32,
    };
    let first = axis.date_at(from as f32).unwrap_or(Date::MIN);
    let last = axis.date_at(through as f32).unwrap_or(Date::MAX);
    axis.columns(first, last)
        .map(|(day, x, width)| TimelineColumn {
            day: x as i32,
            days: width as i32,
            label: now.dates().date(day),
            month: now.dates().month(day),
            week: format!("W{:02}", day.iso_week_date().week()),
            week_remaining: (7 - Weekday::of(day).offset()) as i32,
            weekend: matches!(Weekday::of(day), Weekday::Sat | Weekday::Sun),
        })
        .collect()
}
