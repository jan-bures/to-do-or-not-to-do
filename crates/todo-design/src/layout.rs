//! Spacing, radii, elevation and the fixed metrics of rows, columns and panes. Everything sits on a 4 px grid
//! (half steps only inside text and strokes). Values are logical pixels (dp on Android).

/// Spacing steps.
pub mod space {
    /// The grid unit. Every layout length is a multiple of it.
    pub const GRID: f32 = 4.0;
    /// 4
    pub const S1: f32 = 4.0;
    /// 8
    pub const S2: f32 = 8.0;
    /// 12
    pub const S3: f32 = 12.0;
    /// 16
    pub const S4: f32 = 16.0;
    /// 20
    pub const S5: f32 = 20.0;
    /// 24
    pub const S6: f32 = 24.0;
    /// 32
    pub const S8: f32 = 32.0;
    /// 40
    pub const S10: f32 = 40.0;

    /// Every step, for the exports.
    pub const ALL: [(&str, f32); 8] = [
        ("1", S1),
        ("2", S2),
        ("3", S3),
        ("4", S4),
        ("5", S5),
        ("6", S6),
        ("8", S8),
        ("10", S10),
    ];
}

/// Corner radii. Pills are reserved for list chips, the overdue badge and phone room pills; everything else uses
/// the small radii so inputs, feedback and chips stay distinct.
pub mod radius {
    /// 3: text highlights in the quick-add.
    pub const XS: f32 = 3.0;
    /// 4: keycaps, the parsed-token strip.
    pub const SM: f32 = 4.0;
    /// 6: buttons, icon buttons, the suggestion chip, sidebar items, the search field.
    pub const MD: f32 = 6.0;
    /// 10: the quick-add, toasts, cards (the email source card), popovers.
    pub const LG: f32 = 10.0;
    /// 14: reserved for large sheets.
    pub const XL: f32 = 14.0;

    /// Checkboxes and agent marks are circles: use half their size (`rounded_full` in GPUI, `CircleShape` in
    /// Compose, `50%` in CSS).
    pub const CHECKBOX: &str = "circle";
    /// List chips and the overdue badge (20 tall): a pill.
    pub const CHIP: f32 = 10.0;
    /// Phone room pills (32 tall): a pill.
    pub const ROOM_PILL: f32 = 16.0;
    /// The quick-add field.
    pub const QUICK_ADD: f32 = LG;
    /// Toasts and tooltips.
    pub const TOAST: f32 = LG;
    /// Buttons and the suggestion chip.
    pub const BUTTON: f32 = MD;
    /// The phone composer.
    pub const COMPOSER: f32 = 12.0;

    /// Named radii, for the exports.
    pub const ALL: [(&str, f32); 11] = [
        ("xs", XS),
        ("sm", SM),
        ("md", MD),
        ("lg", LG),
        ("xl", XL),
        ("chip", CHIP),
        ("room-pill", ROOM_PILL),
        ("quick-add", QUICK_ADD),
        ("toast", TOAST),
        ("button", BUTTON),
        ("composer", COMPOSER),
    ];
}

/// Elevation. The design is flat: surfaces are separated by colour and 1 px lines, never by shadows. There is
/// exactly one shadow, for transient overlays (menus, popovers, the date picker), and no blur behind anything.
pub mod elevation {
    use crate::color::{Rgb, Rgba};

    /// Line width of hairlines and of the inset ring around the selected sidebar item and agent marks.
    pub const HAIRLINE: f32 = 1.0;
    /// Width of the focus ring drawn outside a focused input, in the palette's `tint`. Its border turns `accent`.
    pub const FOCUS_RING: f32 = 3.0;
    /// Width of the selection bar on the left edge of the selected row, in `accent`.
    pub const SELECTION_BAR: f32 = 3.0;

    /// One layer of a drop shadow.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Shadow {
        /// Vertical offset.
        pub y: f32,
        /// Blur radius.
        pub blur: f32,
        /// Spread (negative pulls the shadow in).
        pub spread: f32,
        /// Colour.
        pub color: Rgba,
    }

    const LIGHT_SHADOW: Rgb = Rgb::from_u32(0x241c15);
    const DARK_SHADOW: Rgb = Rgb::from_u32(0x000000);

    /// The overlay shadow in light mode, two layers.
    pub const OVERLAY_LIGHT: [Shadow; 2] = [
        Shadow {
            y: 1.0,
            blur: 2.0,
            spread: 0.0,
            color: LIGHT_SHADOW.with_alpha(0.08),
        },
        Shadow {
            y: 8.0,
            blur: 24.0,
            spread: -4.0,
            color: LIGHT_SHADOW.with_alpha(0.14),
        },
    ];
    /// The overlay shadow in dark mode. Overlays also get a 1 px `line-strong` border, because shadows barely
    /// read on dark surfaces.
    pub const OVERLAY_DARK: [Shadow; 2] = [
        Shadow {
            y: 1.0,
            blur: 2.0,
            spread: 0.0,
            color: DARK_SHADOW.with_alpha(0.30),
        },
        Shadow {
            y: 8.0,
            blur: 24.0,
            spread: -4.0,
            color: DARK_SHADOW.with_alpha(0.45),
        },
    ];
}

/// Desktop window metrics.
pub mod desktop {
    /// Sidebar width.
    pub const SIDEBAR: f32 = 248.0;
    /// Task list width at the reference window size (1440 × 900). The list column flexes; the others don't.
    pub const LIST: f32 = 800.0;
    /// Detail pane width.
    pub const DETAIL: f32 = 392.0;
    /// Title bar height.
    pub const TITLEBAR: f32 = 36.0;
    /// Width of a Windows caption button.
    pub const CAPTION_WIDTH: f32 = 46.0;
    /// Size of a Windows caption glyph.
    pub const CAPTION_GLYPH: f32 = 10.0;
    /// Optical stroke of a Windows caption glyph.
    pub const CAPTION_STROKE: f32 = 1.0;
    /// Width of the quiet scrollbar thumb.
    pub const SCROLLBAR_WIDTH: f32 = 6.0;
    /// Width of the hovered thumb, and the clickable track.
    pub const SCROLLBAR_HOVER_WIDTH: f32 = 8.0;
    /// Gap between the scrollbar and each edge of the viewport.
    pub const SCROLLBAR_INSET: f32 = 2.0;
    /// Shortest usable thumb when the content is much taller than the viewport.
    pub const SCROLLBAR_MIN_THUMB: f32 = 24.0;
    /// Status bar height.
    pub const STATUSBAR: f32 = 28.0;
    /// Horizontal padding of the list column (header, quick-add, rows).
    pub const LIST_PAD_X: f32 = 32.0;
    /// Header padding above the eyebrow.
    pub const HEADER_PAD_TOP: f32 = 20.0;
    /// Height of the eyebrow row (day name, date, actions).
    pub const EYEBROW: f32 = 28.0;
    /// Height of the landscape strip under the header.
    pub const LANDSCAPE: f32 = 48.0;
    /// Quick-add height.
    pub const QUICK_ADD: f32 = 44.0;
    /// Vertical margin around the quick-add.
    pub const QUICK_ADD_MARGIN: f32 = 16.0;
    /// Task row height.
    pub const ROW: f32 = 56.0;
    /// Space above the title inside a row.
    pub const ROW_PAD_TOP: f32 = 8.0;
    /// Gap between row columns.
    pub const ROW_GAP: f32 = 12.0;
    /// Gap between the title line and the metadata line.
    pub const ROW_LINE_GAP: f32 = 4.0;
    /// Row columns: checkbox, title (flex), agent, priority, due. The title truncates before the agent column.
    pub const COL_CHECK: f32 = 20.0;
    /// Agent mark column.
    pub const COL_AGENT: f32 = 20.0;
    /// Priority column.
    pub const COL_PRIORITY: f32 = 24.0;
    /// Due column, right-aligned.
    pub const COL_DUE: f32 = 104.0;
    /// The widest a list chip gets in a row's metadata line; a longer list name ends in an ellipsis inside it.
    pub const CHIP_MAX: f32 = 180.0;
    /// The reserved strip at the bottom of the list for "Completed n" and the undo toast; it never covers a row.
    pub const TOAST_INSET: f32 = 56.0;
    /// Toast height.
    pub const TOAST: f32 = 40.0;
    /// Smart-list rows in the sidebar.
    pub const NAV_ROW: f32 = 32.0;
    /// List rows inside a collection, and collection headers.
    pub const NAV_SUB_ROW: f32 = 28.0;
    /// Sidebar footer (sync state).
    pub const SIDEBAR_FOOTER: f32 = 44.0;
    /// Detail pane top bar (breadcrumb and actions).
    pub const DETAIL_TOP: f32 = 48.0;
    /// Detail pane horizontal padding.
    pub const DETAIL_PAD_X: f32 = 24.0;
    /// Step rows.
    pub const STEP_ROW: f32 = 30.0;
    /// Property rows (due, reminder, repeat, tags).
    pub const PROP_ROW: f32 = 34.0;

    /// Every metric, for the exports.
    pub const ALL: [(&str, f32); 36] = [
        ("sidebar", SIDEBAR),
        ("list", LIST),
        ("detail", DETAIL),
        ("titlebar", TITLEBAR),
        ("caption-width", CAPTION_WIDTH),
        ("caption-glyph", CAPTION_GLYPH),
        ("caption-stroke", CAPTION_STROKE),
        ("scrollbar-width", SCROLLBAR_WIDTH),
        ("scrollbar-hover-width", SCROLLBAR_HOVER_WIDTH),
        ("scrollbar-inset", SCROLLBAR_INSET),
        ("scrollbar-min-thumb", SCROLLBAR_MIN_THUMB),
        ("statusbar", STATUSBAR),
        ("list-pad-x", LIST_PAD_X),
        ("header-pad-top", HEADER_PAD_TOP),
        ("eyebrow", EYEBROW),
        ("landscape", LANDSCAPE),
        ("quick-add", QUICK_ADD),
        ("quick-add-margin", QUICK_ADD_MARGIN),
        ("row", ROW),
        ("row-pad-top", ROW_PAD_TOP),
        ("row-gap", ROW_GAP),
        ("row-line-gap", ROW_LINE_GAP),
        ("col-check", COL_CHECK),
        ("col-agent", COL_AGENT),
        ("col-priority", COL_PRIORITY),
        ("col-due", COL_DUE),
        ("chip-max", CHIP_MAX),
        ("toast-inset", TOAST_INSET),
        ("toast", TOAST),
        ("nav-row", NAV_ROW),
        ("nav-sub-row", NAV_SUB_ROW),
        ("sidebar-footer", SIDEBAR_FOOTER),
        ("detail-top", DETAIL_TOP),
        ("detail-pad-x", DETAIL_PAD_X),
        ("step-row", STEP_ROW),
        ("prop-row", PROP_ROW),
    ];
}

/// Phone metrics (Android, dp). The cover screen is about 412 dp wide; the inner screen splits into a 450 dp list
/// and the detail pane.
pub mod phone {
    /// Task row height.
    pub const ROW: f32 = 64.0;
    /// Row padding: top, right, left.
    pub const ROW_PAD: (f32, f32, f32) = (11.0, 12.0, 20.0);
    /// Row columns: checkbox, title (flex), agent, priority.
    pub const COL_CHECK: f32 = 22.0;
    /// Agent mark column.
    pub const COL_AGENT: f32 = 20.0;
    /// Priority column.
    pub const COL_PRIORITY: f32 = 24.0;
    /// The reserved due slot at the start of the metadata line, so list chips always start at the same x.
    pub const DUE_SLOT: f32 = 100.0;
    /// App bar and touch targets.
    pub const APP_BAR: f32 = 48.0;
    /// Minimum touch target.
    pub const TOUCH: f32 = 48.0;
    /// A drawer row's highlight. The row itself is [`TOUCH`] tall, so the target stays full size while
    /// the rows look compact.
    pub const NAV_ROW: f32 = 40.0;
    /// The room pill row.
    pub const ROOM_ROW: f32 = 48.0;
    /// Room pill height.
    pub const ROOM_PILL: f32 = 32.0;
    /// The single composer at the bottom.
    pub const COMPOSER: f32 = 48.0;
    /// The strip above the composer that holds the undo toast.
    pub const TOAST_STRIP: f32 = 56.0;
    /// Toast height.
    pub const TOAST: f32 = 44.0;
    /// Landscape strip on the cover screen.
    pub const LANDSCAPE_COVER: f32 = 36.0;
    /// Landscape strip on the inner screen.
    pub const LANDSCAPE_INNER: f32 = 40.0;
    /// Width of the list pane on the inner screen.
    pub const INNER_LIST: f32 = 450.0;

    /// Every metric, for the exports.
    pub const ALL: [(&str, f32); 19] = [
        ("row", ROW),
        ("col-check", COL_CHECK),
        ("col-agent", COL_AGENT),
        ("col-priority", COL_PRIORITY),
        ("due-slot", DUE_SLOT),
        ("app-bar", APP_BAR),
        ("touch", TOUCH),
        ("nav-row", NAV_ROW),
        ("room-row", ROOM_ROW),
        ("room-pill", ROOM_PILL),
        ("composer", COMPOSER),
        ("toast-strip", TOAST_STRIP),
        ("toast", TOAST),
        ("landscape-cover", LANDSCAPE_COVER),
        ("landscape-inner", LANDSCAPE_INNER),
        ("inner-list", INNER_LIST),
        ("row-pad-top", ROW_PAD.0),
        ("row-pad-right", ROW_PAD.1),
        ("row-pad-left", ROW_PAD.2),
    ];
}

/// Sizes of small components, shared by every platform.
pub mod component {
    /// Checkbox diameter in desktop rows. Phone rows use 22; compact lists use 18.
    pub const CHECKBOX: f32 = 20.0;
    /// Checkbox ring width.
    pub const CHECKBOX_RING: f32 = 1.5;
    /// The tick's stroke width, in the checkbox's 24-unit box.
    pub const CHECKBOX_TICK: f32 = 3.0;
    /// Agent letter mark diameter in rows.
    pub const AGENT_MARK: f32 = 20.0;
    /// Chip and overdue badge height.
    pub const CHIP: f32 = 20.0;
    /// The dot inside a chip.
    pub const CHIP_DOT: f32 = 6.0;
    /// Gap between the dot and the label.
    pub const CHIP_GAP: f32 = 6.0;
    /// Chip horizontal padding.
    pub const CHIP_PAD_X: f32 = 8.0;
    /// Button height.
    pub const BUTTON: f32 = 36.0;
    /// Small button and suggestion chip height.
    pub const BUTTON_SMALL: f32 = 28.0;
    /// Icon button (square).
    pub const ICON_BUTTON: f32 = 32.0;
    /// Icon size in toolbars and rows.
    pub const ICON: f32 = 18.0;
    /// Icon size inside chips and metadata.
    pub const ICON_SMALL: f32 = 14.0;
    /// List glyph size in the sidebar.
    pub const GLYPH: f32 = 16.0;

    /// Every metric, for the exports.
    pub const ALL: [(&str, f32); 14] = [
        ("checkbox", CHECKBOX),
        ("checkbox-ring", CHECKBOX_RING),
        ("checkbox-tick", CHECKBOX_TICK),
        ("agent-mark", AGENT_MARK),
        ("chip", CHIP),
        ("chip-dot", CHIP_DOT),
        ("chip-gap", CHIP_GAP),
        ("chip-pad-x", CHIP_PAD_X),
        ("button", BUTTON),
        ("button-small", BUTTON_SMALL),
        ("icon-button", ICON_BUTTON),
        ("icon", ICON),
        ("icon-small", ICON_SMALL),
        ("glyph", GLYPH),
    ];
}
