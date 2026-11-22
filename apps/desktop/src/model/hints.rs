//! The status bar's keyboard hints: only the keys that work where the keyboard is now.

use super::attachments::Control as FileControl;

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
    /// The detail pane's link field.
    Link,
    /// Renaming a list, a collection or a step in place.
    Rename,
    /// A resize divider next to the list.
    Splitter,
    /// An open menu: the arrows walk its items.
    Menu,
    /// A button that opens a menu (the room's "more", the sync line's "couldn't sync", the account
    /// gear), reached with Tab.
    Control,
    /// A button that acts at once (the Activity room's Refresh, Clear filter and Load older), reached
    /// with Tab.
    Button,
    /// Nothing that takes keys: only the window's Ctrl shortcuts work.
    Elsewhere,
}

impl Focus {
    /// Ctrl+Z undoes the last task change from anywhere that isn't editing text: the list, a divider, a
    /// menu, a menu's button, or nowhere in particular. A text field keeps Ctrl+Z for its own text.
    pub fn allows_task_undo(self) -> bool {
        match self {
            Self::List
            | Self::Splitter
            | Self::Menu
            | Self::Control
            | Self::Button
            | Self::Elsewhere => true,
            Self::QuickAdd
            | Self::Filter
            | Self::Title
            | Self::Notes
            | Self::Step
            | Self::Due
            | Self::Link
            | Self::Rename => false,
        }
    }
}

/// The places Tab moves the keyboard between, outside text fields and open menus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    /// The divider between the sidebar and the list.
    SidebarSplitter,
    /// The task list, where the keyboard starts.
    List,
    /// The undo toast's Undo, while the toast is up.
    Undo,
    /// The room header's "more" button, in rooms that have a menu.
    RoomMenu,
    /// The Activity room's Clear filter, while the feed shows one agent's changes.
    ActivityFilter,
    /// The Activity room's Refresh activity.
    ActivityRefresh,
    /// The button at the end of the Activity feed: Load older activity, or after a failed read, Try
    /// again (or Skip this change).
    ActivityMore,
    /// The divider between the list and the docked detail pane, while it shows.
    DetailSplitter,
    /// My Day toggle in the detail toolbar.
    DetailMyDay,
    /// Important toggle in the detail toolbar.
    DetailImportant,
    /// Reminder, when a task is open.
    Reminder,
    /// Repeat, when a task is open.
    Repeat,
    /// The task's Add file button, followed by each file's controls.
    Attachment(FileControl),
    /// The sidebar footer's New button.
    New,
    /// The sidebar footer's "N couldn't sync", while something couldn't sync.
    SyncFailures,
    /// The account gear in the sidebar footer.
    Account,
}

/// Every Tab stop, in the order Tab goes round them (Shift+Tab goes the other way). A stop that isn't
/// there now (Undo without its toast, the room's button in a room without a menu, the detail divider
/// without a docked detail pane, the sync line with nothing refused) is skipped. A new stop takes its
/// place in this table; nothing else changes.
pub const TAB_ORDER: [Stop; 16] = [
    Stop::SidebarSplitter,
    Stop::List,
    Stop::Undo,
    Stop::RoomMenu,
    Stop::ActivityFilter,
    Stop::ActivityRefresh,
    Stop::ActivityMore,
    Stop::DetailSplitter,
    Stop::DetailMyDay,
    Stop::DetailImportant,
    Stop::Reminder,
    Stop::Repeat,
    Stop::Attachment(FileControl::Add),
    Stop::New,
    Stop::SyncFailures,
    Stop::Account,
];

/// A scrolled pane a Tab stop can sit in, below its fold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollPane {
    Detail,
    /// The Activity room's feed.
    Feed,
}

/// Where a stop that can be scrolled out of view sits, and the probe that records where it was
/// painted: landing on it with Tab scrolls it into view (`layout::reveal_offset`). The others are
/// always in view.
pub fn reveal(stop: Stop) -> Option<(ScrollPane, &'static str)> {
    match stop {
        Stop::DetailMyDay | Stop::DetailImportant => Some((ScrollPane::Detail, "detail-toolbar")),
        Stop::Reminder => Some((ScrollPane::Detail, "reminder-row")),
        Stop::Repeat => Some((ScrollPane::Detail, "repeat-row")),
        // Load older activity (or Try again) ends the feed, below the last change read.
        Stop::ActivityMore => Some((ScrollPane::Feed, "activity-more")),
        Stop::SidebarSplitter
        | Stop::List
        | Stop::Undo
        | Stop::RoomMenu
        | Stop::ActivityFilter
        | Stop::ActivityRefresh
        | Stop::DetailSplitter
        // Attachment controls have row-ID-based probes rather than fixed names.
        | Stop::Attachment(_)
        // The footer is pinned below the sidebar's scrolled rooms.
        | Stop::New
        | Stop::SyncFailures
        | Stop::Account => None,
    }
}

/// Where Tab (`forward`) or Shift+Tab goes from `from`: the next stop round [`TAB_ORDER`] that is
/// `present`. The list is always there; from a stop that has just gone, it goes on from that stop's
/// place in the order.
#[cfg(test)]
fn tab_stop(from: Stop, forward: bool, present: impl Fn(Stop) -> bool) -> Stop {
    next_stop(&TAB_ORDER, from, forward, present)
}

fn next_stop(order: &[Stop], from: Stop, forward: bool, present: impl Fn(Stop) -> bool) -> Stop {
    let n = order.len();
    let at = order.iter().position(|&s| s == from).unwrap_or(0);
    (1..n)
        .map(|k| {
            if forward {
                order[(at + k) % n]
            } else {
                order[(at + n - k) % n]
            }
        })
        .find(|&s| s == Stop::List || present(s))
        .unwrap_or(Stop::List)
}

/// Where navigation Tab goes from where the keyboard is (`focused`, the stop it's on, if any). Off the
/// stops (a text field, an open menu) Tab isn't the shell's: `None`, and the focused element keeps it.
pub fn tab_target(
    focused: Option<Stop>,
    forward: bool,
    present: impl Fn(Stop) -> bool,
    files: impl IntoIterator<Item = FileControl>,
) -> Option<Stop> {
    let from = focused?;
    let mut order = Vec::new();
    let mut files = Some(files);
    for stop in TAB_ORDER {
        order.push(stop);
        if stop == Stop::Attachment(FileControl::Add)
            && let Some(files) = files.take()
        {
            order.extend(files.into_iter().map(Stop::Attachment));
        }
    }
    Some(next_stop(&order, from, forward, present))
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
    /// The room is Activity: the list's keys walk and undo changes instead of tasks.
    pub activity: Option<ActivityHints>,
}

/// What the list's keys can do in the Activity room.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityHints {
    /// There are changes to move through.
    pub rows: bool,
    /// A change is selected.
    pub selected: bool,
    /// The selected change can be undone.
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
        Focus::List if s.activity.is_some() => {
            let a = s.activity.unwrap_or_default();
            if a.rows {
                hints.push(("↑↓", "move"));
            }
            if a.undo {
                hints.push(("Enter", "undo"));
            }
            hints.push(("R", "refresh"));
            if a.selected {
                hints.push(("Esc", "clear"));
            }
        }
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
                    ("Shift P", "pick priority"),
                    ("M", "move"),
                    ("Shift R", "remind"),
                    ("R", "repeat"),
                    ("T", "tags"),
                    ("L", "link"),
                    ("Del", "delete"),
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
        Focus::Link => hints.extend([("Enter", "add link"), ("Esc", "back to the list")]),
        Focus::Rename => hints.extend([("Enter", "save"), ("Esc", "cancel")]),
        Focus::Splitter => hints.extend([
            ("← →", "resize"),
            ("Shift ← →", "larger steps"),
            ("Home / Enter", "reset"),
            ("Esc", "back to the list"),
        ]),
        Focus::Menu => hints.extend([("↑↓", "choose"), ("Enter", "select"), ("Esc", "close")]),
        Focus::Control => hints.extend([
            ("Enter", "open"),
            ("Tab", "next"),
            ("Esc", "back to the list"),
        ]),
        Focus::Button => hints.extend([
            ("Enter", "press"),
            ("Tab", "next"),
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

/// The hints that give way, in this order, when the status bar is too narrow for all of them: the keys
/// that have another way in first (the detail pane's Tags, links and Priority rows, the row's context
/// menu), then the letters. Space, Escape and Ctrl+Z always stay.
const GIVE_WAY: [&str; 12] = [
    "Shift R",
    "R",
    "T",
    "L",
    "Shift P",
    "Del",
    "M",
    "Alt ↑↓",
    "D",
    "P",
    "E",
    "J K",
];

/// The hints that fit in `room` pixels, in their order, `gap` apart, each `width(hint)` wide: those in
/// [`GIVE_WAY`] are left out, in its order, until the rest fit.
pub fn fit(hints: Vec<Hint>, room: f32, gap: f32, width: impl Fn(Hint) -> f32) -> Vec<Hint> {
    let total = |hints: &[Hint]| -> f32 {
        let n = hints.len() as f32;
        hints.iter().map(|&h| width(h)).sum::<f32>() + gap * (n - 1.).max(0.)
    };
    let mut shown = hints;
    for key in GIVE_WAY {
        if total(&shown) <= room {
            break;
        }
        shown.retain(|(k, _)| *k != key);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_narrow_status_bar_drops_the_keys_with_another_way_in_first() {
        let all = key_hints(list(true, true, true));
        // Every hint 50 wide, 10 apart.
        let width = |_: Hint| 50.;
        let n = all.len() as f32;
        let full = 50. * n + 10. * (n - 1.);
        assert_eq!(fit(all.clone(), full, 10., width), all);
        let keys = |h: Vec<Hint>| h.into_iter().map(|(k, _)| k).collect::<Vec<_>>();
        assert_eq!(
            keys(fit(all.clone(), full - 61., 10., width)),
            [
                "J K",
                "Space",
                "E",
                "D",
                "P",
                "Shift P",
                "M",
                "T",
                "L",
                "Del",
                "Alt ↑↓",
                "Esc"
            ]
        );
        assert_eq!(
            keys(fit(all.clone(), full - 250., 10., width)),
            ["J K", "Space", "E", "D", "P", "M", "Del", "Alt ↑↓", "Esc"]
        );
        assert_eq!(
            keys(fit(all.clone(), full - 370., 10., width)),
            ["J K", "Space", "E", "D", "P", "Alt ↑↓", "Esc"]
        );
        // However narrow, moving, Space and Escape stay.
        assert_eq!(keys(fit(all, 0., 10., width)), ["Space", "Esc"]);
    }

    fn list(rows: bool, selected: bool, reorder: bool) -> HintState {
        HintState {
            focus: Focus::List,
            rows,
            selected: selected.then_some(Selected { done: false }),
            reorder,
            undo: false,
            activity: None,
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
            [
                "J K", "Space", "E", "D", "P", "Shift P", "M", "Shift R", "R", "T", "L", "Del",
                "Esc"
            ]
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
            Focus::Link,
            Focus::Rename,
        ] {
            let k = keys(HintState {
                focus,
                rows: true,
                selected: Some(Selected { done: false }),
                reorder: true,
                undo: true,
                activity: None,
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
            activity: None,
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
    fn menus_and_menu_buttons_show_their_own_keys() {
        let state = |focus| HintState {
            focus,
            rows: true,
            selected: Some(Selected { done: false }),
            reorder: true,
            undo: false,
            activity: None,
        };
        assert_eq!(keys(state(Focus::Menu)), ["↑↓", "Enter", "Esc"]);
        assert_eq!(keys(state(Focus::Control)), ["Enter", "Tab", "Esc"]);
    }

    #[test]
    fn ctrl_z_undoes_a_task_change_from_menus_and_their_buttons_but_never_from_text() {
        for focus in [
            Focus::List,
            Focus::Splitter,
            Focus::Menu,
            Focus::Control,
            Focus::Elsewhere,
        ] {
            assert!(focus.allows_task_undo(), "{focus:?}");
            let k = keys(HintState {
                focus,
                rows: true,
                selected: None,
                reorder: false,
                undo: true,
                activity: None,
            });
            assert_eq!(k.last(), Some(&"Ctrl Z"), "{focus:?}");
        }
        for focus in [
            Focus::QuickAdd,
            Focus::Filter,
            Focus::Title,
            Focus::Notes,
            Focus::Step,
            Focus::Due,
            Focus::Link,
            Focus::Rename,
        ] {
            assert!(!focus.allows_task_undo(), "{focus:?}");
        }
    }

    /// Every stop Tab reaches from `start`, going round once, and Shift+Tab's from the same place.
    fn round(start: Stop, forward: bool, present: impl Fn(Stop) -> bool + Copy) -> Vec<Stop> {
        let mut at = start;
        let mut seen = Vec::new();
        loop {
            at = tab_stop(at, forward, present);
            seen.push(at);
            if at == start || seen.len() > TAB_ORDER.len() {
                return seen;
            }
        }
    }

    #[test]
    fn tab_and_shift_tab_go_all_the_way_round_every_stop() {
        use Stop::*;
        let all = |_: Stop| true;
        // Forwards from each stop, it comes back to it having passed every other stop once, in order.
        for start in TAB_ORDER {
            let at = TAB_ORDER
                .iter()
                .position(|&s| s == start)
                .expect("in the order");
            let expected: Vec<Stop> = (1..=TAB_ORDER.len())
                .map(|k| TAB_ORDER[(at + k) % TAB_ORDER.len()])
                .collect();
            assert_eq!(round(start, true, all), expected, "from {start:?}");
            // Shift+Tab is exactly the same journey backwards.
            let mut back = round(start, false, all);
            back.pop();
            back.reverse();
            back.push(start);
            assert_eq!(back, expected, "back from {start:?}");
        }
        // With every conditional stop present: the whole table, both ways.
        assert_eq!(
            round(List, true, all),
            [
                Undo,
                RoomMenu,
                ActivityFilter,
                ActivityRefresh,
                ActivityMore,
                DetailSplitter,
                DetailMyDay,
                DetailImportant,
                Reminder,
                Repeat,
                Attachment(FileControl::Add),
                New,
                SyncFailures,
                Account,
                SidebarSplitter,
                List
            ]
        );
        assert_eq!(
            round(List, false, all),
            [
                SidebarSplitter,
                Account,
                SyncFailures,
                New,
                Attachment(FileControl::Add),
                Repeat,
                Reminder,
                DetailImportant,
                DetailMyDay,
                DetailSplitter,
                ActivityMore,
                ActivityRefresh,
                ActivityFilter,
                RoomMenu,
                Undo,
                List
            ]
        );
        // Shift+Tab from Undo leaves it for the list, and the list goes on backwards: never stuck
        // between the two.
        assert_eq!(tab_stop(Undo, false, all), List);
        assert_eq!(tab_stop(List, false, all), SidebarSplitter);
    }

    #[test]
    fn tab_skips_the_stops_that_are_not_there() {
        use Stop::*;
        let extra_detail_control = |s| matches!(s, DetailMyDay | DetailImportant | Attachment(_));
        // Without the toast, a room menu, a docked detail or anything refused: the dividers' and the
        // fixed stops only (New is always there). (Below, nothing couldn't
        // sync unless said.)
        let fixed = |s: Stop| {
            !extra_detail_control(s)
                && !matches!(
                    s,
                    Undo | RoomMenu
                        | DetailSplitter
                        | Reminder
                        | Repeat
                        | ActivityFilter
                        | ActivityRefresh
                        | ActivityMore
                        | SyncFailures
                )
        };
        assert_eq!(
            round(List, true, fixed),
            [New, Account, SidebarSplitter, List]
        );
        assert_eq!(
            round(List, false, fixed),
            [SidebarSplitter, Account, New, List]
        );
        // Without the detail divider (narrow window, or no task open), both ways.
        let no_detail = |s: Stop| {
            !extra_detail_control(s)
                && !matches!(
                    s,
                    DetailSplitter
                        | Reminder
                        | Repeat
                        | ActivityFilter
                        | ActivityRefresh
                        | ActivityMore
                        | SyncFailures
                )
        };
        assert_eq!(
            round(RoomMenu, true, no_detail),
            [New, Account, SidebarSplitter, List, Undo, RoomMenu]
        );
        assert_eq!(
            round(RoomMenu, false, no_detail),
            [Undo, List, SidebarSplitter, Account, New, RoomMenu]
        );
        // A room without a menu skips its button both ways.
        let no_menu = |s: Stop| {
            !extra_detail_control(s)
                && !matches!(
                    s,
                    RoomMenu
                        | Reminder
                        | Repeat
                        | ActivityFilter
                        | ActivityRefresh
                        | ActivityMore
                        | SyncFailures
                )
        };
        assert_eq!(
            round(Undo, true, no_menu),
            [DetailSplitter, New, Account, SidebarSplitter, List, Undo]
        );
        assert_eq!(
            round(Undo, false, no_menu),
            [List, SidebarSplitter, Account, New, DetailSplitter, Undo]
        );
        // Without the toast, Undo is skipped.
        let no_undo = |s: Stop| {
            !extra_detail_control(s)
                && !matches!(
                    s,
                    Undo | Reminder
                        | Repeat
                        | ActivityFilter
                        | ActivityRefresh
                        | ActivityMore
                        | SyncFailures
                )
        };
        assert_eq!(
            round(List, true, no_undo),
            [
                RoomMenu,
                DetailSplitter,
                New,
                Account,
                SidebarSplitter,
                List
            ]
        );
        assert_eq!(
            round(List, false, no_undo),
            [
                SidebarSplitter,
                Account,
                New,
                DetailSplitter,
                RoomMenu,
                List
            ]
        );
        // From a stop that has just gone (the toast expired while Undo had the keyboard, the detail
        // closed under its divider), it goes on from that stop's place.
        assert_eq!(tab_stop(Undo, true, no_undo), RoomMenu);
        assert_eq!(tab_stop(Undo, false, no_undo), List);
        assert_eq!(tab_stop(DetailSplitter, true, no_detail), New);
        assert_eq!(tab_stop(DetailSplitter, false, no_detail), RoomMenu);
        // Something couldn't sync: its line comes just before the account gear, both ways.
        let refused = |s: Stop| {
            !extra_detail_control(s)
                && !matches!(
                    s,
                    Undo | RoomMenu
                        | DetailSplitter
                        | Reminder
                        | Repeat
                        | ActivityFilter
                        | ActivityRefresh
                        | ActivityMore
                )
        };
        assert_eq!(
            round(List, true, refused),
            [New, SyncFailures, Account, SidebarSplitter, List]
        );
        assert_eq!(
            round(List, false, refused),
            [SidebarSplitter, Account, SyncFailures, New, List]
        );
        // The list is always there, even if asked otherwise.
        assert_eq!(tab_stop(Account, true, |_| false), List);
        assert_eq!(tab_stop(Account, false, |_| false), List);
    }

    #[test]
    fn stops_below_a_fold_are_scrolled_into_view_in_their_own_pane() {
        assert_eq!(
            reveal(Stop::ActivityMore),
            Some((ScrollPane::Feed, "activity-more"))
        );
        assert_eq!(
            reveal(Stop::Repeat),
            Some((ScrollPane::Detail, "repeat-row"))
        );
        assert_eq!(
            reveal(Stop::Reminder),
            Some((ScrollPane::Detail, "reminder-row"))
        );
        // The toolbar's buttons sit above the feed and never scroll away.
        assert_eq!(reveal(Stop::ActivityRefresh), None);
        assert_eq!(reveal(Stop::ActivityFilter), None);
        // The footer is pinned under the sidebar's scrolled rooms, always in view.
        assert_eq!(reveal(Stop::New), None);
    }

    #[test]
    fn shift_tab_from_reminder_reveals_the_detail_toolbar() {
        let mut from = Stop::Reminder;
        for expected in [Stop::DetailImportant, Stop::DetailMyDay] {
            let next = tab_stop(from, false, |_| true);
            assert_eq!(next, expected);
            assert_eq!(reveal(next), Some((ScrollPane::Detail, "detail-toolbar")));
            from = next;
        }
    }

    #[test]
    fn the_activity_rooms_buttons_come_after_its_menu_button() {
        use Stop::*;
        // Activity filtered to an agent, with older changes to read; no toast, detail or refusals.
        let activity = |s: Stop| {
            !matches!(
                s,
                Undo | DetailSplitter
                    | DetailMyDay
                    | DetailImportant
                    | Reminder
                    | Repeat
                    | Attachment(_)
                    | SyncFailures
            )
        };
        assert_eq!(
            round(List, true, activity),
            [
                RoomMenu,
                ActivityFilter,
                ActivityRefresh,
                ActivityMore,
                New,
                Account,
                SidebarSplitter,
                List
            ]
        );
        // Unfiltered, at the end of the feed: only Refresh.
        let plain = |s: Stop| activity(s) && !matches!(s, ActivityFilter | ActivityMore);
        assert_eq!(
            round(List, false, plain),
            [
                SidebarSplitter,
                Account,
                New,
                ActivityRefresh,
                RoomMenu,
                List
            ]
        );
        assert!(Focus::Button.allows_task_undo());
        assert_eq!(
            keys(HintState {
                focus: Focus::Button,
                rows: false,
                selected: None,
                reorder: false,
                undo: false,
                activity: Some(ActivityHints::default()),
            }),
            ["Enter", "Tab", "Esc"]
        );
    }

    #[test]
    fn in_activity_the_list_keys_walk_and_undo_changes() {
        let state = |activity| HintState {
            focus: Focus::List,
            rows: false,
            selected: None,
            reorder: false,
            undo: false,
            activity: Some(activity),
        };
        assert_eq!(keys(state(ActivityHints::default())), ["R"]);
        assert_eq!(
            keys(state(ActivityHints {
                rows: true,
                selected: true,
                undo: true
            })),
            ["↑↓", "Enter", "R", "Esc"]
        );
        assert_eq!(
            keys(state(ActivityHints {
                rows: true,
                selected: true,
                undo: false
            })),
            ["↑↓", "R", "Esc"]
        );
    }

    #[test]
    fn attachment_controls_tab_in_row_order_and_back_to_add() {
        let d = crate::fixtures::my_day().shell().detail.expect("detail");
        let controls: Vec<_> = super::super::attachments::file_controls(&d.attachments).collect();
        let add = Stop::Attachment(FileControl::Add);
        let first = Stop::Attachment(*controls.first().expect("first file control"));
        let last = Stop::Attachment(*controls.last().expect("last file control"));
        let tab =
            |from, forward| tab_target(Some(from), forward, |_| true, controls.iter().copied());
        assert_eq!(tab(Stop::Repeat, true), Some(add));
        assert_eq!(tab(add, true), Some(first));
        assert_eq!(tab(first, false), Some(add));
        assert_eq!(tab(last, true), Some(Stop::NewList));
        assert_eq!(tab(Stop::NewList, false), Some(last));
    }

    #[test]
    fn tab_off_the_stops_is_left_to_the_focused_element() {
        let all = |_: Stop| true;
        // A text field or an open menu: not a stop, so the shell doesn't take Tab.
        assert_eq!(tab_target(None, true, all, []), None);
        assert_eq!(tab_target(None, false, all, []), None);
        assert_eq!(
            tab_target(Some(Stop::Account), true, all, []),
            Some(Stop::SidebarSplitter)
        );
        assert_eq!(
            tab_target(Some(Stop::SidebarSplitter), false, all, []),
            Some(Stop::Account)
        );
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
                activity: None,
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
