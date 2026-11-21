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
    /// Nothing that takes keys: only the window's Ctrl shortcuts work.
    Elsewhere,
}

/// What the hints depend on besides the focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HintState {
    pub focus: Focus,
    /// The list shows rows to move through.
    pub rows: bool,
    /// A task is selected (open in the detail pane).
    pub selected: bool,
    /// The room is a list, whose own order Alt+Up and Alt+Down change.
    pub reorder: bool,
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
            if s.selected {
                hints.extend([
                    ("Space", "complete"),
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
        Focus::Elsewhere => {}
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
            selected,
            reorder,
        }
    }

    fn keys(s: HintState) -> Vec<&'static str> {
        key_hints(s).into_iter().map(|(k, _)| k).collect()
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
                selected: true,
                reorder: true,
            });
            assert_eq!(k, ["Enter", "Esc"], "{focus:?}");
        }
    }

    #[test]
    fn without_the_list_focused_the_letters_dont_work() {
        let k = keys(HintState {
            focus: Focus::Elsewhere,
            rows: true,
            selected: true,
            reorder: true,
        });
        assert_eq!(k, ["Ctrl K"]);
    }
}
