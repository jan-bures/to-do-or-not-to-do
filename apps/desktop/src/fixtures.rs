//! What the shell shows: the view model, and the fixtures `--screenshot` renders.
//!
//! Until sync lands (plan step 4b) the running app shows an empty My Day. Every fixture is invented sample
//! data, never real tasks.

use gpui::SharedString;

use crate::theme::{Agent, hue};

/// The line above the room title.
#[derive(Clone, Debug, PartialEq)]
pub enum Eyebrow {
    /// Smart lists: the weekday in italics, then the date and the week.
    Day {
        weekday: String,
        date: String,
        week: String,
    },
    /// Collections and lists: an italic kind, then a mono count.
    Kind { kind: &'static str, detail: String },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListRef {
    pub collection: &'static str,
    pub name: &'static str,
    pub hue: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Due {
    None,
    /// Soon: accent colour.
    Soon(&'static str),
    Plain(&'static str),
    Overdue(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub title: SharedString,
    pub list: Option<ListRef>,
    pub tags: &'static [&'static str],
    pub steps: Option<(u32, u32)>,
    pub agent: Option<Agent>,
    /// 0 to 3 exclamation marks.
    pub priority: u8,
    pub due: Due,
    pub from_mail: bool,
}

/// The room the main pane shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Room {
    pub title: &'static str,
    pub eyebrow: Eyebrow,
    pub hue: u16,
    /// Smart lists show the list's collection in chips ("Work · Finance"); collections only the list.
    pub smart: bool,
    pub done: u32,
    pub total: u32,
    pub estimate: Option<&'static str>,
    pub tasks: Vec<Task>,
    /// Shown instead of rows when there are no tasks.
    pub empty_line: &'static str,
    pub quick_add_placeholder: &'static str,
    /// Text typed into the quick-add, for fixtures that show it focused.
    pub quick_add_text: Option<&'static str>,
    pub completed: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NavItem {
    pub icon: &'static str,
    pub label: &'static str,
    pub hue: u16,
    pub count: Option<u32>,
    /// "3 new" pip instead of a count.
    pub pip: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Collection {
    pub glyph: &'static str,
    pub name: &'static str,
    pub hue: u16,
    pub count: u32,
    pub lists: Vec<(ListRef, &'static str, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentRow {
    pub agent: Agent,
    pub name: &'static str,
    pub status: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sidebar {
    pub smart: Vec<NavItem>,
    pub collections: Vec<Collection>,
    pub agents: Vec<AgentRow>,
    /// Index into `smart` or a list name that is selected.
    pub selected: Selected,
    pub synced: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Selected {
    Smart(usize),
    Collection(&'static str),
    List(&'static str),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub title: &'static str,
    pub done: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub list: ListRef,
    pub title: &'static str,
    pub subtitle: &'static str,
    pub steps: Vec<Step>,
    pub notes: &'static str,
    pub fields: Vec<(&'static str, &'static str, &'static str)>,
}

/// Everything one window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Shell {
    pub room: Room,
    pub sidebar: Sidebar,
    pub detail: Option<Detail>,
    /// Index of the selected task row.
    pub selected_task: Option<usize>,
}

const fn list(collection: &'static str, name: &'static str, hue: u16) -> ListRef {
    ListRef {
        collection,
        name,
        hue,
    }
}

const PLANNING: ListRef = list("Work", "Planning", hue::PLANNING);
const FINANCE: ListRef = list("Work", "Finance", hue::FINANCE);
const OFFICE: ListRef = list("Work", "Office", hue::OFFICE);
const EVENTS: ListRef = list("Work", "Events", hue::EVENTS);
const TODO_APP: ListRef = list("Projects", "To-do app", hue::TODO_APP);
const HOME: ListRef = list("Projects", "Home renovation", hue::HOME);
const READING: ListRef = list("Projects", "Reading list", hue::READING);
const ERRANDS: ListRef = list("Personal", "Errands", hue::ERRANDS);
const HEALTH: ListRef = list("Personal", "Health", hue::HEALTH);
const TRAVEL: ListRef = list("Personal", "Travel", hue::TRAVEL);
const ADMIN: ListRef = list("Personal", "Admin", hue::ADMIN);

fn task(title: &'static str, list: ListRef) -> Task {
    Task {
        title: title.into(),
        list: Some(list),
        tags: &[],
        steps: None,
        agent: None,
        priority: 0,
        due: Due::None,
        from_mail: false,
    }
}

fn sidebar(selected: Selected) -> Sidebar {
    let nav = |icon, label, hue, count| NavItem {
        icon,
        label,
        hue,
        count: Some(count),
        pip: None,
    };
    Sidebar {
        smart: vec![
            nav("sun", "My Day", hue::MY_DAY, 9),
            nav("star", "Important", hue::IMPORTANT, 4),
            nav("cal", "Planned", hue::PLANNED, 17),
            nav("list", "All", hue::ALL, 38),
            nav("done", "Completed", hue::COMPLETED, 112),
            NavItem {
                icon: "spark",
                label: "Agent activity",
                hue: hue::AGENT,
                count: None,
                pip: Some("3 new"),
            },
        ],
        collections: vec![
            Collection {
                glyph: "work",
                name: "Work",
                hue: hue::WORK,
                count: 15,
                lists: vec![
                    (PLANNING, "planning", 6),
                    (FINANCE, "finance", 3),
                    (OFFICE, "office", 4),
                    (EVENTS, "events", 2),
                ],
            },
            Collection {
                glyph: "projects",
                name: "Projects",
                hue: hue::PROJECTS,
                count: 16,
                lists: vec![
                    (TODO_APP, "todoapp", 9),
                    (HOME, "home", 5),
                    (READING, "reading", 2),
                ],
            },
            Collection {
                glyph: "personal",
                name: "Personal",
                hue: hue::PERSONAL,
                count: 9,
                lists: vec![
                    (ERRANDS, "errands", 3),
                    (HEALTH, "health", 2),
                    (TRAVEL, "travel", 4),
                ],
            },
        ],
        agents: vec![
            AgentRow {
                agent: Agent::Claude,
                name: "Claude",
                status: "2 added",
            },
            AgentRow {
                agent: Agent::Codex,
                name: "Codex",
                status: "3 moved",
            },
            AgentRow {
                agent: Agent::Pi,
                name: "Pi",
                status: "idle",
            },
        ],
        selected,
        synced: "Synced 10:34",
    }
}

fn detail() -> Detail {
    Detail {
        list: PLANNING,
        title: "Prepare Q4 roadmap review",
        subtitle: "Created 14 Nov / Edited today 09:12",
        steps: vec![
            Step {
                title: "Collect shipped features from the changelog",
                done: true,
            },
            Step {
                title: "Draft three themes for next quarter",
                done: true,
            },
            Step {
                title: "Estimate effort per theme",
                done: false,
            },
            Step {
                title: "Review the draft with Alex",
                done: false,
            },
            Step {
                title: "Publish the final deck to the team wiki",
                done: false,
            },
        ],
        notes: "Keep it to one page. Compare against the Q3 goals and flag anything that slipped, with a one-line reason each.",
        fields: vec![
            ("cal", "Due", "Sat 21 Nov, 17:00"),
            ("bell", "Reminder", "Today, 15:30"),
            ("repeat", "Repeat", "Every Monday"),
            ("hash", "Tags", "#planning #q4"),
        ],
    }
}

/// My Day, 3 of 12 done (the F2 light mockup).
pub fn my_day() -> Shell {
    let tasks = vec![
        Task {
            tags: &["#tax"],
            priority: 3,
            due: Due::Overdue("2d overdue"),
            ..task("Pay quarterly VAT filing", FINANCE)
        },
        Task {
            from_mail: true,
            due: Due::Soon("in 1h 25m"),
            ..task("Send venue booking confirmation", EVENTS)
        },
        Task {
            tags: &["#q4"],
            steps: Some((2, 5)),
            agent: Some(Agent::Codex),
            priority: 2,
            due: Due::Plain("Today 17:00"),
            ..task("Prepare Q4 roadmap review", PLANNING)
        },
        Task {
            tags: &["#domácnost"],
            due: Due::Plain("Today 18:30"),
            ..task("Koupit rohlíky a mléko", ERRANDS)
        },
        Task {
            tags: &["#review"],
            agent: Some(Agent::Claude),
            priority: 3,
            due: Due::Plain("Today"),
            ..task("Review sync-engine pull request", TODO_APP)
        },
        Task {
            priority: 1,
            due: Due::Plain("Tomorrow"),
            ..task("Renew passport appointment", ADMIN)
        },
        Task {
            tags: &["#pojištění"],
            agent: Some(Agent::Pi),
            priority: 1,
            due: Due::Plain("Mon 23 Nov"),
            ..task(
                "Vyřídit žádost o přeplatek na zdravotním pojištění a doložit potvrzení od lékaře",
                HEALTH,
            )
        },
        Task {
            tags: &["#docs"],
            steps: Some((1, 3)),
            agent: Some(Agent::Codex),
            due: Due::Plain("Mon 23 Nov"),
            ..task("Write schema migration notes", TODO_APP)
        },
        Task {
            ..task("Call the dentist to reschedule", HEALTH)
        },
    ];
    Shell {
        room: Room {
            title: "My Day",
            eyebrow: Eyebrow::Day {
                weekday: "Saturday".into(),
                date: "21 Nov 2026".into(),
                week: "W47".into(),
            },
            hue: hue::MY_DAY,
            smart: true,
            done: 3,
            total: 12,
            estimate: Some("5h 40m"),
            tasks,
            empty_line: "Nothing planned yet.",
            quick_add_placeholder: "Add a task…",
            quick_add_text: None,
            completed: 3,
        },
        sidebar: sidebar(Selected::Smart(0)),
        detail: Some(detail()),
        selected_task: Some(2),
    }
}

/// Work, 7 of 22 done, with quick-add typing (the F2 dark mockup).
pub fn work() -> Shell {
    let tasks = vec![
        Task {
            tags: &["#tax"],
            priority: 3,
            due: Due::Overdue("2d overdue"),
            ..task("Pay quarterly VAT filing", FINANCE)
        },
        Task {
            tags: &["#retro"],
            agent: Some(Agent::Claude),
            due: Due::Soon("in 1h 20m"),
            ..task("Confirm catering numbers for the retro", EVENTS)
        },
        Task {
            tags: &["#q4"],
            steps: Some((2, 5)),
            agent: Some(Agent::Codex),
            priority: 2,
            due: Due::Plain("Today 17:00"),
            ..task("Prepare Q4 roadmap review", PLANNING)
        },
        Task {
            tags: &["#retro"],
            due: Due::Plain("Tue 24 Nov"),
            ..task("Book a room for the retro", EVENTS)
        },
        Task {
            tags: &["#budget"],
            steps: Some((0, 4)),
            agent: Some(Agent::Claude),
            priority: 2,
            due: Due::Plain("Wed 25 Nov"),
            ..task("Draft Q1 budget summary", FINANCE)
        },
        Task {
            tags: &["#docs"],
            steps: Some((1, 3)),
            priority: 1,
            due: Due::Plain("Thu 26 Nov"),
            ..task("Update the onboarding checklist", PLANNING)
        },
        Task {
            tags: &["#legal"],
            agent: Some(Agent::Codex),
            priority: 3,
            due: Due::Plain("Fri 27 Nov"),
            ..task("Review the vendor contract", OFFICE)
        },
        Task {
            tags: &["#supplies"],
            agent: Some(Agent::Pi),
            ..task("Order printer toner", OFFICE)
        },
        Task {
            ..task("Archive the old design files", OFFICE)
        },
    ];
    Shell {
        room: Room {
            title: "Work",
            eyebrow: Eyebrow::Kind {
                kind: "Collection",
                detail: "4 lists".into(),
            },
            hue: hue::WORK,
            smart: false,
            done: 7,
            total: 22,
            estimate: Some("9h 20m"),
            tasks,
            empty_line: "Nothing here yet.",
            quick_add_placeholder: "Add a task to Work…",
            quick_add_text: Some("Draft the changelog for 0.4 tomorrow"),
            completed: 3,
        },
        sidebar: sidebar(Selected::Collection("Work")),
        detail: Some(detail()),
        selected_task: Some(2),
    }
}

/// An empty list: scenery only, no sun (nothing to keep score of).
pub fn empty() -> Shell {
    Shell {
        room: Room {
            title: "Reading list",
            eyebrow: Eyebrow::Kind {
                kind: "Projects",
                detail: "0 open".into(),
            },
            hue: hue::READING,
            smart: false,
            done: 0,
            total: 0,
            estimate: None,
            tasks: Vec::new(),
            empty_line: "Nothing on the shelf yet.",
            quick_add_placeholder: "Add a book or an article…",
            quick_add_text: None,
            completed: 2,
        },
        sidebar: {
            // The shelf is empty here, so the sidebar agrees.
            let mut sidebar = sidebar(Selected::List("Reading list"));
            for collection in &mut sidebar.collections {
                if let Some(entry) = collection
                    .lists
                    .iter_mut()
                    .find(|(list, _, _)| list.name == "Reading list")
                {
                    collection.count -= entry.2;
                    entry.2 = 0;
                }
            }
            sidebar
        },
        detail: None,
        selected_task: None,
    }
}

/// What the running app shows before sync exists: an empty My Day for today.
pub fn startup(weekday: String, date: String, week: String) -> Shell {
    let mut shell = my_day();
    shell.room.eyebrow = Eyebrow::Day {
        weekday,
        date,
        week,
    };
    shell.room.tasks.clear();
    shell.room.done = 0;
    shell.room.total = 0;
    shell.room.estimate = None;
    shell.room.completed = 0;
    shell.detail = None;
    shell.selected_task = None;
    shell.sidebar = Sidebar {
        collections: Vec::new(),
        agents: Vec::new(),
        synced: "Not signed in",
        ..shell.sidebar
    };
    for item in &mut shell.sidebar.smart {
        item.count = item.count.map(|_| 0);
        item.pip = None;
    }
    shell
}

/// The fixture with this name (see `args::FIXTURES`). `quick-add` reuses My Day for its room colours.
pub fn by_name(name: &str) -> Option<Shell> {
    match name {
        "my-day" | "quick-add" => Some(my_day()),
        "work" => Some(work()),
        "empty" => Some(empty()),
        _ => None,
    }
}

/// Header summary numbers: (done, total, open).
pub fn summary(room: &Room) -> (u32, u32, u32) {
    (room.done, room.total, room.total.saturating_sub(room.done))
}

/// "Saturday", "21 Nov 2026", "W47" for a civil date (ISO 8601 week numbering).
pub fn eyebrow_for(year: i32, month: u32, day: u32) -> (String, String, String) {
    const WEEKDAYS: [&str; 7] = [
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
        "Sunday",
    ];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = days_from_civil(year, month, day);
    // 1970-01-01 was a Thursday (index 3).
    let weekday = (days + 3).rem_euclid(7) as usize;
    let week = iso_week(year, days, weekday);
    let month_name = MONTHS
        .get(month.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("?");
    (
        WEEKDAYS[weekday].to_string(),
        format!("{day} {month_name} {year}"),
        format!("W{week}"),
    )
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn iso_week(year: i32, days: i64, weekday: usize) -> u32 {
    // The Thursday of this week decides the ISO year.
    let thursday = days - weekday as i64 + 3;
    let mut iso_year = year;
    if thursday < days_from_civil(year, 1, 1) {
        iso_year -= 1;
    } else if thursday >= days_from_civil(year + 1, 1, 1) {
        iso_year += 1;
    }
    ((thursday - days_from_civil(iso_year, 1, 1)) / 7 + 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_empty_fixture_counts_no_open_books() {
        let shell = empty();
        let projects = shell
            .sidebar
            .collections
            .iter()
            .find(|c| c.name == "Projects")
            .expect("projects");
        let reading = projects
            .lists
            .iter()
            .find(|(l, _, _)| l.name == "Reading list")
            .expect("reading list");
        assert_eq!(reading.2, 0);
        assert_eq!(projects.count, 14);
    }

    #[test]
    fn eyebrow_dates() {
        assert_eq!(
            eyebrow_for(2026, 11, 21),
            ("Saturday".into(), "21 Nov 2026".into(), "W47".into())
        );
        assert_eq!(eyebrow_for(2021, 1, 1).2, "W53"); // Friday, still in 2020's last week.
        assert_eq!(eyebrow_for(2024, 12, 30).2, "W1"); // Monday of 2025's first week.
        assert_eq!(eyebrow_for(2024, 2, 29).0, "Thursday");
    }

    #[test]
    fn every_fixture_exists_and_keeps_its_numbers_consistent() {
        for name in crate::args::FIXTURES {
            let shell = by_name(name).unwrap_or_else(|| panic!("{name}"));
            let (done, total, open) = summary(&shell.room);
            assert!(done <= total);
            assert_eq!(done + open, total);
            if let Some(i) = shell.selected_task {
                assert!(i < shell.room.tasks.len());
            }
        }
        assert!(by_name("inbox").is_none());
    }

    #[test]
    fn the_running_app_starts_empty() {
        let shell = startup("Monday".into(), "1 Jan 2029".into(), "W1".into());
        assert!(shell.room.tasks.is_empty() && shell.detail.is_none());
        assert_eq!(summary(&shell.room), (0, 0, 0));
    }

    #[test]
    fn fixtures_carry_czech_text() {
        let titles: Vec<SharedString> = my_day()
            .room
            .tasks
            .iter()
            .map(|t| t.title.clone())
            .collect();
        assert!(titles.iter().any(|t| t.contains('ř')));
    }
}
