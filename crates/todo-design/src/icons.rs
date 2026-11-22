//! Icons. One set, used consistently: [Lucide](https://lucide.dev) (ISC), stroke icons on a 24-unit grid, bundled
//! in `assets/icons/lucide/` with the stroke width set to 1.6 to match the mockups. Only the icons M1 needs are
//! bundled; add more from the same Lucide release (see `assets/icons/README.md`).
//!
//! The hand-drawn list glyphs ([`Glyph`]) are the app's own: slightly irregular single-stroke drawings that give
//! lists character without colour blocks. They are drawn in the list's `glyph` colour.
//!
//! Both are single-colour SVGs (`stroke="currentColor"`, no fills), so GPUI can tint them. The SVG source is
//! embedded with `include_str!`.

macro_rules! svg_set {
    ($(#[$meta:meta])* $name:ident, $dir:literal { $( $(#[doc = $doc:literal])* $variant:ident => $file:literal, )* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $( $(#[doc = $doc])* $variant, )*
        }

        impl $name {
            /// Every variant, in file-name order.
            pub const ALL: &'static [$name] = &[$( $name::$variant ),*];

            /// The file name without `.svg`, which is also the stable key.
            pub const fn key(self) -> &'static str {
                match self { $( $name::$variant => $file, )* }
            }

            /// The path relative to the repository root.
            pub fn path(self) -> String {
                format!(concat!($dir, "/{}.svg"), self.key())
            }

            /// The SVG source.
            pub const fn svg(self) -> &'static str {
                match self { $( $name::$variant => include_str!(concat!("../../../", $dir, "/", $file, ".svg")), )* }
            }

            /// Looks a variant up by key.
            pub fn from_key(key: &str) -> Option<$name> {
                Self::ALL.iter().copied().find(|v| v.key() == key)
            }
        }
    };
}

svg_set! {
    /// A Lucide interface icon. Draw it at [`crate::layout::component::ICON`] (18) in toolbars and rows, 14 in
    /// chips and metadata.
    Icon, "assets/icons/lucide" {
        /// Sort.
        ArrowDownUp => "arrow-down-up",
        /// Send (phone composer).
        ArrowUp => "arrow-up",
        /// Open the source of a task.
        ArrowUpRight => "arrow-up-right",
        /// Reminder.
        Bell => "bell",
        /// Planned; due date.
        Calendar => "calendar",
        /// Done; confirm.
        Check => "check",
        /// Expand; menus.
        ChevronDown => "chevron-down",
        /// Back.
        ChevronLeft => "chevron-left",
        /// Collapsed group; next.
        ChevronRight => "chevron-right",
        /// Collapse.
        ChevronUp => "chevron-up",
        /// Error; conflict.
        CircleAlert => "circle-alert",
        /// Completed.
        CircleCheck => "circle-check",
        /// Time.
        Clock => "clock",
        /// Offline.
        CloudOff => "cloud-off",
        /// Copy; window restore.
        Copy => "copy",
        /// Enter.
        CornerDownLeft => "corner-down-left",
        /// Download an attachment.
        Download => "download",
        /// More actions.
        Ellipsis => "ellipsis",
        /// Open a link outside the app.
        ExternalLink => "external-link",
        /// Show a token.
        Eye => "eye",
        /// Hide a token.
        EyeOff => "eye-off",
        /// Priority.
        Flag => "flag",
        /// New collection.
        FolderPlus => "folder-plus",
        /// Reorder handle.
        GripVertical => "grip-vertical",
        /// Tags.
        Hash => "hash",
        /// Shortcuts.
        Keyboard => "keyboard",
        /// Links.
        Link => "link",
        /// All.
        List => "list",
        /// New list.
        ListPlus => "list-plus",
        /// Sign in.
        LogIn => "log-in",
        /// Sign out.
        LogOut => "log-out",
        /// From email.
        Mail => "mail",
        /// Menu (phone).
        Menu => "menu",
        /// Dictation (phone).
        Mic => "mic",
        /// Window minimise.
        Minus => "minus",
        /// Dark theme.
        Moon => "moon",
        /// Attachments.
        Paperclip => "paperclip",
        /// Rename; edit.
        Pencil => "pencil",
        /// Add.
        Plus => "plus",
        /// Redo.
        RedoTwo => "redo-2",
        /// Sync now.
        RefreshCw => "refresh-cw",
        /// Recurrence.
        Repeat => "repeat",
        /// Search.
        Search => "search",
        /// Settings.
        Settings => "settings",
        /// View options.
        SlidersHorizontal => "sliders-horizontal",
        /// Agent activity; suggestions.
        Sparkles => "sparkles",
        /// Window maximise.
        Square => "square",
        /// Important.
        Star => "star",
        /// My Day; light theme.
        Sun => "sun",
        /// Tomorrow's first task.
        Sunrise => "sunrise",
        /// Finished day.
        Sunset => "sunset",
        /// Notes.
        Text => "text",
        /// Delete.
        TrashTwo => "trash-2",
        /// Undo.
        UndoTwo => "undo-2",
        /// Close; window close.
        X => "x",
    }
}

svg_set! {
    /// A hand-drawn list glyph. Draw it at [`crate::layout::component::GLYPH`] (16) in the list's glyph colour.
    Glyph, "assets/icons/glyphs" {
        /// For errands, shopping.
        Basket => "basket",
        /// For reading.
        Book => "book",
        /// For work.
        Briefcase => "briefcase",
        /// For events.
        CalendarStar => "calendar-star",
        /// For software projects.
        Code => "code",
        /// For money, finance.
        Coin => "coin",
        /// For admin, paperwork.
        Document => "document",
        /// For planning, goals.
        Flag => "flag",
        /// For health.
        Heart => "heart",
        /// For home, personal.
        House => "house",
        /// For projects.
        Layers => "layers",
        /// For travel.
        PaperPlane => "paper-plane",
        /// For office.
        Paperclip => "paperclip",
        /// For My Day.
        Sun => "sun",
        /// For repairs, home improvement.
        Wrench => "wrench",
    }
}

impl Glyph {
    /// Picks a glyph for a list name already folded to lowercase without accents, or a document.
    #[must_use]
    pub fn for_list(folded: &str) -> Self {
        Self::for_folded_name(folded).unwrap_or(Self::Document)
    }

    /// Picks a glyph for a collection name already folded to lowercase without accents, or layers.
    #[must_use]
    pub fn for_collection(folded: &str) -> Self {
        Self::for_folded_name(folded).unwrap_or(Self::Layers)
    }

    fn for_folded_name(name: &str) -> Option<Self> {
        // First match wins, as in "Work projects" or "Plan my trip".
        const WORDS: &[(Glyph, &[&str])] = &[
            (Glyph::Sun, &["my day", "today", "dnes"]),
            (Glyph::Briefcase, &["work", "job", "prace", "zamestn"]),
            (
                Glyph::Coin,
                &[
                    "financ", "money", "budget", "tax", "invoice", "penize", "dane", "ucty",
                    "faktur",
                ],
            ),
            (Glyph::Paperclip, &["office", "kancel"]),
            (
                Glyph::CalendarStar,
                &["event", "party", "akce", "udalost", "oslav"],
            ),
            (
                Glyph::Code,
                &["code", "dev", "app", "software", "program", "github"],
            ),
            (
                Glyph::Wrench,
                &[
                    "renovat", "repair", "diy", "fix", "oprav", "rekonstr", "dilna",
                ],
            ),
            (
                Glyph::Book,
                &["read", "book", "kniha", "knihy", "cteni", "studi", "learn"],
            ),
            (
                Glyph::Basket,
                &[
                    "shop", "grocer", "errand", "buy", "nakup", "potravin", "obchod",
                ],
            ),
            (
                Glyph::Heart,
                &[
                    "health", "doctor", "fitness", "sport", "zdravi", "lekar", "cvic",
                ],
            ),
            (
                Glyph::PaperPlane,
                &[
                    "travel", "trip", "vacation", "holiday", "cest", "dovolen", "vylet",
                ],
            ),
            (
                Glyph::Document,
                &["admin", "document", "paper", "doklad", "urad", "formular"],
            ),
            (Glyph::Flag, &["plan", "goal", "cil", "milestone"]),
            (Glyph::Layers, &["project", "projekt"]),
            (
                Glyph::House,
                &[
                    "personal", "home", "house", "family", "osobni", "domov", "doma", "rodina",
                ],
            ),
        ];
        WORDS
            .iter()
            .find(|(_, words)| words.iter().any(|word| name.contains(word)))
            .map(|(glyph, _)| *glyph)
    }
}

#[cfg(test)]
mod tests {
    use super::Glyph;

    #[test]
    fn folded_glyph_keywords_match_in_order() {
        assert_eq!(Glyph::for_collection("work projects"), Glyph::Briefcase);
        assert_eq!(Glyph::for_collection("prace"), Glyph::Briefcase);
        assert_eq!(Glyph::for_collection("projects"), Glyph::Layers);
        assert_eq!(Glyph::for_collection("osobni"), Glyph::House);
        assert_eq!(Glyph::for_list("nakupy"), Glyph::Basket);
        assert_eq!(Glyph::for_list("zdravi"), Glyph::Heart);
        assert_eq!(Glyph::for_list("home renovation"), Glyph::Wrench);
        assert_eq!(Glyph::for_list("reading list"), Glyph::Book);
    }

    #[test]
    fn unmatched_names_have_a_fallback_for_each_kind() {
        assert_eq!(Glyph::for_list("misc"), Glyph::Document);
        assert_eq!(Glyph::for_collection("misc"), Glyph::Layers);
    }
}
