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
