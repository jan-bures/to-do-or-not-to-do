//! Desktop presentation and input mapping for the shared workflow model.

use gpui::KeyDownEvent;
use todo_core::command::{ListFeaturesPatch, Placement};
use todo_core::ids::TaskId;
use todo_core::model::{ListFeatures, ListView, WorkflowStatus};
use todo_core::query::QueryStatus;
use todo_core::time::Date;

use super::labels;
use crate::theme::Icon;

pub const STAGES: [QueryStatus; 4] = [
    QueryStatus::Backlog,
    QueryStatus::Todo,
    QueryStatus::InProgress,
    QueryStatus::Done,
];

pub fn status(stage: QueryStatus) -> (&'static str, Icon) {
    match stage {
        QueryStatus::Backlog => ("Backlog", Icon::CircleDashed),
        QueryStatus::Todo => ("To do", Icon::Circle),
        QueryStatus::InProgress => ("In progress", Icon::CircleProgress),
        QueryStatus::Done => ("Done", Icon::StatusDone),
    }
}

pub fn open_stage(stage: QueryStatus) -> Option<WorkflowStatus> {
    match stage {
        QueryStatus::Backlog => Some(WorkflowStatus::Backlog),
        QueryStatus::Todo => Some(WorkflowStatus::Todo),
        QueryStatus::InProgress => Some(WorkflowStatus::InProgress),
        QueryStatus::Done => None,
    }
}

pub fn stage(value: WorkflowStatus) -> QueryStatus {
    match value {
        WorkflowStatus::Backlog => QueryStatus::Backlog,
        WorkflowStatus::Todo => QueryStatus::Todo,
        WorkflowStatus::InProgress => QueryStatus::InProgress,
    }
}

pub const VIEWS: [ListView; 3] = [ListView::List, ListView::Board, ListView::Timeline];

pub fn view_label(view: ListView) -> (&'static str, Icon) {
    match view {
        ListView::List => ("List", Icon::List),
        ListView::Board => ("Board", Icon::Columns3),
        ListView::Timeline => ("Timeline", Icon::GanttChart),
    }
}

pub fn available(features: ListFeatures, view: ListView) -> bool {
    // Timeline can already be opted into (and set as the synced default on other clients).
    features.allows(view) && view != ListView::Timeline
}

pub fn effective(features: ListFeatures, selected: Option<ListView>) -> ListView {
    selected
        .filter(|&v| available(features, v))
        .unwrap_or_else(|| {
            if available(features, features.default_view) {
                features.default_view
            } else {
                ListView::List
            }
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    Workflow,
    Timeline,
    Default(ListView),
}

impl Setting {
    pub fn patch(self, features: ListFeatures) -> ListFeaturesPatch {
        let mut patch = ListFeaturesPatch::default();
        match self {
            Self::Workflow => patch.workflow = Some(!features.workflow),
            Self::Timeline => patch.timeline = Some(!features.timeline),
            Self::Default(view) => patch.default_view = Some(view),
        }
        patch
    }
}

/// Visible anchors, never a raw index in the underlying list. The shared resolver checks that
/// the anchor is still in the target stage and maps the slot onto the global position key.
pub fn slot(card: Option<(TaskId, bool)>) -> Placement {
    match card {
        Some((id, true)) => Placement::After(id.into()),
        Some((id, false)) => Placement::Before(id.into()),
        None => Placement::Last,
    }
}

pub fn span(start: Date, end: Date, today: Date) -> String {
    if start == end {
        return labels::short_date(end, today);
    }
    if start.year() == end.year() && start.month() == end.month() {
        format!("{}–{}", start.day(), labels::day_month(end, today))
    } else {
        format!(
            "{}–{}",
            labels::day_month(start, today),
            labels::day_month(end, today)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    CycleView,
    View(ListView),
    Column(isize),
    Row(isize),
    MoveColumn(isize),
    MoveRow(isize),
    Status,
    CycleStatus,
    Starts,
    Due,
    Add,
    Complete,
    Open,
    Edit,
    Escape,
}

pub fn key(event: &KeyDownEvent, board: bool) -> Option<(Key, bool)> {
    let m = event.keystroke.modifiers;
    if m.control || m.platform || m.function {
        return None;
    }
    let key = match (event.keystroke.key.as_str(), m.alt, m.shift) {
        ("v", false, false) => Key::CycleView,
        ("1", false, false) => Key::View(ListView::List),
        ("2", false, false) => Key::View(ListView::Board),
        ("3", false, false) => Key::View(ListView::Timeline),
        ("s", false, false) => Key::Status,
        ("s", false, true) => Key::CycleStatus,
        ("d", false, true) => Key::Starts,
        ("d", false, false) if board => Key::Due,
        ("h" | "left", false, false) if board => Key::Column(-1),
        ("l" | "right", false, false) if board => Key::Column(1),
        ("k" | "up", false, false) if board => Key::Row(-1),
        ("j" | "down", false, false) if board => Key::Row(1),
        ("left", true, false) if board => Key::MoveColumn(-1),
        ("right", true, false) if board => Key::MoveColumn(1),
        ("up", true, false) if board => Key::MoveRow(-1),
        ("down", true, false) if board => Key::MoveRow(1),
        ("n", false, false) if board => Key::Add,
        ("space", false, false) if board => Key::Complete,
        ("enter", false, false) if board => Key::Open,
        ("e", false, false) if board => Key::Edit,
        ("escape", false, false) if board => Key::Escape,
        _ => return None,
    };
    Some((
        key,
        !event.is_held || matches!(key, Key::Column(_) | Key::Row(_)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};

    fn event(k: &str, alt: bool, shift: bool, held: bool) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers {
                    alt,
                    shift,
                    ..Default::default()
                },
                key: k.into(),
                key_char: None,
            },
            is_held: held,
            prefer_character_input: false,
        }
    }

    #[test]
    fn board_only_keys_do_not_steal_list_actions_or_repeat_writes() {
        assert_eq!(
            key(&event("l", false, false, true), true),
            Some((Key::Column(1), true))
        );
        assert_eq!(key(&event("l", false, false, false), false), None);
        assert_eq!(
            key(&event("left", true, false, true), true),
            Some((Key::MoveColumn(-1), false))
        );
        assert_eq!(
            key(&event("d", false, true, false), false),
            Some((Key::Starts, true))
        );
    }

    #[test]
    fn disabling_board_resets_the_synced_default_and_ignores_a_stale_override() {
        let mut f = crate::fixtures::by_name("board").expect("board fixture");
        let super::super::RoomKey::List(id) = f.ui.room else {
            panic!("list room")
        };
        let features = f.state.lists[&id].features;
        let patch = Setting::Workflow.patch(features);
        assert_eq!(patch.timeline, None);
        let command = todo_client::intent::workflow::set_list_features(&f.state, id, patch)
            .expect("disable workflow");
        let actor = todo_core::Actor::owner("desktop-test").expect("owner");
        for event in
            todo_core::decide(&f.state, &actor, &command, f.now.at).expect("valid settings change")
        {
            todo_core::apply(&mut f.state, &event).expect("apply settings change");
        }
        let next = f.state.lists[&id].features;
        assert!(next.timeline);
        assert_eq!(next.default_view, ListView::List);
        assert_eq!(effective(next, Some(ListView::Board)), ListView::List);
        let next = Setting::Default(ListView::Timeline).patch(next).merge(next);
        assert_eq!(next.default_view, ListView::Timeline);
        assert_eq!(effective(next, None), ListView::List);
    }

    #[test]
    fn card_halves_map_to_anchors_and_empty_space_to_column_end() {
        let id = TaskId::derive(todo_core::time::Timestamp::UNIX_EPOCH, b"anchor");
        assert_eq!(slot(Some((id, false))), Placement::Before(id.into()));
        assert_eq!(slot(Some((id, true))), Placement::After(id.into()));
        assert_eq!(slot(None), Placement::Last);
    }
}
