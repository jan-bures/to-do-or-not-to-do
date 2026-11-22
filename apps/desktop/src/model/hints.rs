//! The status bar's keyboard hints: only the keys that work where the keyboard is now.

/// Where the keyboard is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    /// The task list: the single-letter keys work here, and only here.
    List,
    /// The room's quick-add.
    QuickAdd,
    /// The filter in the title bar.
    Filter,
    /// The detail pane's title.
    Title,
    /// The detail pane's notes.
    Notes,
    /// The detail pane's "Add step".
    Step,
    /// The due-date field.
    Due,
    /// Renaming a list, a collection or a step in place.
    Rename,
    /// A resize divider next to the list.
    Splitter,
    /// Nothing that takes keys: only the window's Ctrl shortcuts work.
    Elsewhere,
}

impl Focus {
    /// Never take the task shortcut from an input that is editing text.
    pub fn allows_task_undo(self) -> bool {
        matches!(self, Self::List | Self::Splitter | Self::Elsewhere)
    }
}

/// What the hints depend on besides the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HintState {
    pub focus: Focus,
    /// The list shows tasks to move through (rows, or the finished day's hand-over).
    pub rows: bool,
    /// The selected task (open in the detail pane), if any: whether it is completed, which turns Space's
    /// "complete" into "reopen".
    pub selected: Option<Selected>,
    /// The room is a list, whose own order Alt+Up and Alt+Down change.
    pub reorder: bool,
    /// A task change can still be undone, even if its toast has expired.
    pub undo: bool,
}

/// The selected task, as far as the hints care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selected {
    pub done: bool,
}

/// What Space does to a task: `commands::toggle_complete` reopens a completed one.
pub fn space_label(done: bool) -> &'static str {
    if done { "reopen" } else { "complete" }
}

/// A keycap and what it does.
pub type Hint = (&'static str, &'static str);

/// The hints for the status bar, in order.
pub fn key_hints(s: HintState) -> Vec<Hint> {
    let mut hints = Vec::new();
    match s.focus {
        Focus::List => {
            if s.rows {
                hints.push(("J K", "move"));
            }
            if let Some(selected) = s.selected {
                hints.extend([
                    ("Space", space_label(selected.done)),
                    ("E", "edit"),
                    ("D", "due"),
                    ("P", "priority"),
                ]);
                if s.reorder {
                    hints.push(("Alt ↑↓", "reorder"));
                }
                hints.push(("Esc", "close"));
            }
        }
        Focus::QuickAdd => hints.extend([("Enter", "add"), ("Esc", "back to the list")]),
        Focus::Filter => hints.extend([("Enter", "to the list"), ("Esc", "clear")]),
        Focus::Title => hints.extend([("Enter", "save"), ("Esc", "undo changes")]),
        Focus::Notes => hints.extend([("Enter", "new line"), ("Esc", "save")]),
        Focus::Step => hints.extend([("Enter", "add step"), ("Esc", "back to the list")]),
        Focus::Due => hints.extend([("Enter", "set"), ("Esc", "cancel")]),
        Focus::Rename => hints.extend([("Enter", "save"), ("Esc", "cancel")]),
        Focus::Splitter => hints.extend([
            ("← →", "resize"),
            ("Shift ← →", "larger steps"),
            ("Home / Enter", "reset"),
            ("Esc", "back to the list"),
        ]),
        Focus::Elsewhere => {}
    }
    if s.undo && s.focus.allows_task_undo() {
        hints.push(("Ctrl Z", "undo"));
    }
    if hints.is_empty() {
        // Nothing else to do from here: the filter (Ctrl N, adding a task, is on the right).
        hints.push(("Ctrl K", "filter"));
    }
    hints
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(rows: bool, selected: bool, reorder: bool) -> HintState {
        HintState {
            focus: Focus::List,
            rows,
            selected: selected.then_some(Selected { done: false }),
            reorder,
            undo: false,
        }
    }

    fn keys(s: HintState) -> Vec<&'static str> {
        key_hints(s).into_iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn splitter_focus_replaces_list_keys_with_resize_controls() {
        let mut s = list(true, true, true);
        s.focus = Focus::Splitter;
        assert_eq!(keys(s), ["← →", "Shift ← →", "Home / Enter", "Esc"]);
    }

    #[test]
    fn an_empty_list_offers_no_task_keys() {
        assert_eq!(keys(list(false, false, true)), ["Ctrl K"]);
    }

    #[test]
    fn task_keys_need_a_selected_task() {
        assert_eq!(keys(list(true, false, true)), ["J K"]);
        assert_eq!(
            keys(list(true, true, false)),
            ["J K", "Space", "E", "D", "P", "Esc"]
        );
    }

    #[test]
    fn reordering_shows_only_in_a_list() {
        assert!(keys(list(true, true, true)).contains(&"Alt ↑↓"));
        assert!(!keys(list(true, true, false)).contains(&"Alt ↑↓"));
    }

    #[test]
    fn typing_shows_the_fields_own_keys_not_the_lists() {
        for focus in [
            Focus::QuickAdd,
            Focus::Filter,
            Focus::Title,
            Focus::Notes,
            Focus::Step,
            Focus::Due,
            Focus::Rename,
        ] {
            let k = keys(HintState {
                focus,
                rows: true,
                selected: Some(Selected { done: false }),
                reorder: true,
                undo: true,
            });
            assert_eq!(k, ["Enter", "Esc"], "{focus:?}");
        }
    }

    #[test]
    fn without_the_list_focused_the_letters_dont_work() {
        let k = keys(HintState {
            focus: Focus::Elsewhere,
            rows: true,
            selected: Some(Selected { done: false }),
            reorder: true,
            undo: false,
        });
        assert_eq!(k, ["Ctrl K"]);
    }

    #[test]
    fn undo_hint_does_not_depend_on_the_toast_or_a_selected_task() {
        let mut state = list(false, false, false);
        state.undo = true;
        assert_eq!(keys(state), ["Ctrl Z"]);
    }

    #[test]
    fn space_says_what_toggling_does() {
        use crate::fixtures;
        use crate::model::commands;
        use todo_core::Command;
        let f = fixtures::my_day();
        for task in f.state.tasks.values() {
            let hint = key_hints(HintState {
                focus: Focus::List,
                rows: true,
                selected: Some(Selected {
                    done: task.is_done(),
                }),
                reorder: false,
                undo: false,
            });
            let space = hint.iter().find(|(k, _)| *k == "Space").map(|(_, w)| *w);
            let expected = match commands::toggle_complete(&f.state, task.id) {
                Some(Command::CompleteTask { .. }) => "complete",
                Some(Command::UncompleteTask { .. }) => "reopen",
                other => panic!("{other:?}"),
            };
            assert_eq!(space, Some(expected), "{}", task.title);
        }
        assert!(f.state.tasks.values().any(|t| t.is_done()));
        assert!(f.state.tasks.values().any(|t| !t.is_done()));
    }
}
