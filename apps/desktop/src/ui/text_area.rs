//! A multi-line plain-text editor for a task's notes: wraps to its width, Enter starts a new line, Escape
//! (or leaving it) ends editing. Same input-handler plumbing as `TextInput` (IME, selection, clipboard),
//! laid out with GPUI's wrapped text.

use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, Pixels, Point, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window,
    WrappedLine, actions, div, fill, point, prelude::*, px, relative, size,
};
use unicode_segmentation::UnicodeSegmentation;

use super::input::{InputColors, InputEvent};
use super::word_edit::{self, WordEdit};

actions!(
    text_area,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        Newline,
        SoftNewline,
        Paste,
        Cut,
        Copy,
        Done,
    ]
);

/// Key bindings for the `TextArea` context.
pub fn key_bindings() -> Vec<gpui::KeyBinding> {
    use gpui::KeyBinding;
    let ctx = Some("TextArea");
    let mut bindings = vec![
        KeyBinding::new("backspace", Backspace, ctx),
        KeyBinding::new("delete", Delete, ctx),
        KeyBinding::new("left", Left, ctx),
        KeyBinding::new("right", Right, ctx),
        KeyBinding::new("up", Up, ctx),
        KeyBinding::new("down", Down, ctx),
        KeyBinding::new("shift-left", SelectLeft, ctx),
        KeyBinding::new("shift-right", SelectRight, ctx),
        KeyBinding::new("shift-up", SelectUp, ctx),
        KeyBinding::new("shift-down", SelectDown, ctx),
        KeyBinding::new("ctrl-a", SelectAll, ctx),
        KeyBinding::new("ctrl-v", Paste, ctx),
        KeyBinding::new("ctrl-c", Copy, ctx),
        KeyBinding::new("ctrl-x", Cut, ctx),
        KeyBinding::new("home", Home, ctx),
        KeyBinding::new("end", End, ctx),
        KeyBinding::new("enter", Newline, ctx),
        KeyBinding::new("shift-enter", SoftNewline, ctx),
        KeyBinding::new("escape", Done, ctx),
        KeyBinding::new("ctrl-enter", Done, ctx),
    ];
    bindings.extend(word_edit::key_bindings("TextArea"));
    bindings
}

/// One paragraph as laid out: its shaped text, where it starts in the content, and its top.
struct Paragraph {
    line: WrappedLine,
    start: usize,
    top: Pixels,
}

/// The last layout, for mouse hits, caret moves and the IME.
struct Layout {
    paragraphs: Vec<Paragraph>,
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    /// Whether it shows the content (not the placeholder).
    content: SharedString,
}

impl Layout {
    fn paragraph_of(&self, offset: usize) -> Option<&Paragraph> {
        self.paragraphs
            .iter()
            .rev()
            .find(|p| p.start <= offset)
            .or(self.paragraphs.first())
    }

    /// Where `offset` sits, relative to the bounds' origin.
    fn position(&self, offset: usize) -> Point<Pixels> {
        let Some(p) = self.paragraph_of(offset) else {
            return Point::default();
        };
        let local = offset.saturating_sub(p.start).min(p.line.len());
        let at = p
            .line
            .position_for_index(local, self.line_height)
            .unwrap_or_default();
        point(at.x, p.top + at.y)
    }

    /// The offset closest to `pos`, relative to the bounds' origin.
    fn index_at(&self, pos: Point<Pixels>) -> usize {
        let Some(first) = self.paragraphs.first() else {
            return 0;
        };
        if pos.y < Pixels::ZERO {
            return first.start;
        }
        let p = self
            .paragraphs
            .iter()
            .rev()
            .find(|p| p.top <= pos.y)
            .unwrap_or(first);
        let rows = p.line.wrap_boundaries().len() + 1;
        let max_y = self.line_height * (rows as f32) - px(1.);
        let local = point(pos.x.max(Pixels::ZERO), (pos.y - p.top).min(max_y));
        let i = match p.line.closest_index_for_position(local, self.line_height) {
            Ok(i) | Err(i) => i,
        };
        p.start + i.min(p.line.len())
    }
}

#[allow(missing_debug_implementations)] // Holds GPUI layout state that isn't Debug.
pub struct TextArea {
    pub focus_handle: FocusHandle,
    pub content: SharedString,
    pub placeholder: SharedString,
    pub colors: InputColors,
    /// One paragraph only (a title that wraps): Enter submits and pasted line breaks become spaces.
    pub single: bool,
    /// Chat sends on Enter but still accepts newlines via Shift+Enter.
    pub chat: bool,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    layout: Option<Layout>,
    is_selecting: bool,
}

impl EventEmitter<InputEvent> for TextArea {}

impl TextArea {
    pub fn new(
        placeholder: impl Into<SharedString>,
        colors: InputColors,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: "".into(),
            placeholder: placeholder.into(),
            colors,
            single: false,
            chat: false,
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            layout: None,
            is_selecting: false,
        }
    }

    /// Replaces the content (a different task was selected, or the notes changed elsewhere) and puts the
    /// caret at its end.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        self.content = if self.single { one_line(text) } else { text };
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn reset(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.content = "".into();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.layout = None;
        self.is_selecting = false;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn done(&mut self, _: &Done, _w: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        if self.chat {
            cx.emit(InputEvent::Submit);
        } else {
            self.soft_newline(&SoftNewline, window, cx);
        }
    }

    fn soft_newline(&mut self, _: &SoftNewline, window: &mut Window, cx: &mut Context<Self>) {
        if self.single {
            cx.emit(InputEvent::Submit);
        } else {
            self.replace_text_in_range(None, "\n", window, cx);
        }
    }

    /// Selects everything (to type over it).
    pub fn select_everything(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn left(&mut self, _: &Left, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), window, cx);
        } else {
            self.move_to(self.selected_range.start, window, cx)
        }
    }

    fn right(&mut self, _: &Right, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), window, cx);
        } else {
            self.move_to(self.selected_range.end, window, cx)
        }
    }

    /// The offset one visual row above (`rows` < 0) or below the caret.
    fn vertical(&self, rows: f32) -> usize {
        let Some(layout) = &self.layout else {
            return self.cursor_offset();
        };
        if layout.content != self.content {
            return self.cursor_offset();
        }
        let at = layout.position(self.cursor_offset());
        let y = at.y + layout.line_height * rows + layout.line_height / 2.;
        if y < Pixels::ZERO {
            return 0;
        }
        let total = layout
            .paragraphs
            .last()
            .map(|p| p.top + layout.line_height * (p.line.wrap_boundaries().len() + 1) as f32)
            .unwrap_or_default();
        if y >= total {
            return self.content.len();
        }
        layout.index_at(point(at.x, y))
    }

    fn up(&mut self, _: &Up, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(-1.);
        self.move_to(to, window, cx);
    }

    fn down(&mut self, _: &Down, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(1.);
        self.move_to(to, window, cx);
    }

    fn select_up(&mut self, _: &SelectUp, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(-1.);
        self.select_to(to, window, cx);
    }

    fn select_down(&mut self, _: &SelectDown, window: &mut Window, cx: &mut Context<Self>) {
        let to = self.vertical(1.);
        self.select_to(to, window, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn select_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn edit_word(&mut self, edit: &WordEdit, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let target = edit.target(&self.content, cursor);
        match edit {
            WordEdit::Left | WordEdit::Right => self.move_to(target, window, cx),
            WordEdit::SelectLeft | WordEdit::SelectRight => self.select_to(target, window, cx),
            WordEdit::DeleteLeft | WordEdit::DeleteRight => self.delete_to(target, window, cx),
        }
    }

    fn select_all(&mut self, _: &SelectAll, window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, window, cx);
        self.select_to(self.content.len(), window, cx)
    }

    /// Home and End go to the start and end of the paragraph.
    fn home(&mut self, _: &Home, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.cursor_offset();
        let start = self.content[..at].rfind('\n').map_or(0, |i| i + 1);
        self.move_to(start, window, cx);
    }

    fn end(&mut self, _: &End, window: &mut Window, cx: &mut Context<Self>) {
        let at = self.cursor_offset();
        let end = self.content[at..]
            .find('\n')
            .map_or(self.content.len(), |i| at + i);
        self.move_to(end, window, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(self.previous_boundary(self.cursor_offset()), window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_to(self.next_boundary(self.cursor_offset()), window, cx);
    }

    fn delete_to(&mut self, target: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            if self.cursor_offset() == target {
                return;
            }
            self.select_to(target, window, cx);
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn index_for_mouse(&self, position: Point<Pixels>) -> usize {
        match &self.layout {
            Some(l) if l.content == self.content && !self.content.is_empty() => {
                l.index_at(position - l.bounds.origin)
            }
            _ => 0,
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        let at = self.index_for_mouse(event.position);
        if event.modifiers.shift {
            self.select_to(at, window, cx);
        } else {
            self.move_to(at, window, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_selecting {
            let at = self.index_for_mouse(event.position);
            self.select_to(at, window, cx);
        }
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let text = text.replace("\r\n", "\n");
            let text = if self.single {
                text.replace('\n', " ")
            } else {
                text
            };
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn move_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, window: &Window, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextArea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        window.notify_text_selection_changed(&self.focus_handle);
        cx.notify();
    }

    fn text_length_utf16(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.content.encode_utf16().count())
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        window.notify_text_input_changed(&self.focus_handle);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let single_text;
        let new_text = if self.single {
            single_text = crate::model::labels::one_line(new_text);
            single_text.as_str()
        } else {
            new_text
        };
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let single_text;
        let new_text = if self.single {
            single_text = crate::model::labels::one_line(new_text);
            single_text.as_str()
        } else {
            new_text
        };
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        let start16 = self.offset_to_utf16(range.start);
        let len16 = new_text.encode_utf16().count();
        self.selected_range = new_selected_range_utf16
            .map(|s| {
                self.range_from_utf16(&(start16 + s.start.min(len16)..start16 + s.end.min(len16)))
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        window.notify_text_input_changed(&self.focus_handle);
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        if layout.content != self.content {
            return None;
        }
        let range = self.range_from_utf16(&range_utf16);
        let start = layout.position(range.start);
        let end = layout.position(range.end);
        let origin = layout.bounds.origin;
        Some(Bounds::from_corners(
            origin + start,
            origin + point(end.x.max(start.x), start.y + layout.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        if layout.content != self.content {
            return None;
        }
        let i = layout.index_at(point - layout.bounds.origin);
        Some(self.offset_to_utf16(i))
    }
}

struct AreaElement {
    area: Entity<TextArea>,
}

struct Prepainted {
    paragraphs: Vec<Paragraph>,
    quads: Vec<PaintQuad>,
    cursor: Option<PaintQuad>,
    line_height: Pixels,
}

/// The text to lay out and its runs (the placeholder when empty; the IME's marked text underlined).
fn display(area: &TextArea, window: &Window) -> (SharedString, Vec<TextRun>, Pixels) {
    let style = window.text_style();
    let (text, color) = if area.content.is_empty() {
        (area.placeholder.clone(), area.colors.placeholder)
    } else {
        (area.content.clone(), area.colors.text)
    };
    let run = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = match area
        .marked_range
        .as_ref()
        .filter(|_| !area.content.is_empty())
    {
        Some(m) => vec![
            TextRun {
                len: m.start,
                ..run.clone()
            },
            TextRun {
                len: m.end - m.start,
                underline: Some(UnderlineStyle {
                    color: Some(run.color),
                    thickness: px(1.0),
                    wavy: false,
                }),
                ..run.clone()
            },
            TextRun {
                len: text.len() - m.end,
                ..run
            },
        ]
        .into_iter()
        .filter(|r| r.len > 0)
        .collect(),
        None => vec![run],
    };
    let font_size = style.font_size.to_pixels(window.rem_size());
    (text, runs, font_size)
}

fn rows(lines: &[WrappedLine]) -> usize {
    lines
        .iter()
        .map(|l| l.wrap_boundaries().len() + 1)
        .sum::<usize>()
        .max(1)
}

impl Element for AreaElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepainted;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let (text, runs, font_size) = display(self.area.read(cx), window);
        let line_height = window.line_height();
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, _cx| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let lines = window
                .text_system()
                .shape_text(text.clone(), font_size, &runs, width, None)
                .unwrap_or_default();
            size(width.unwrap_or_default(), line_height * rows(&lines) as f32)
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let area = self.area.read(cx);
        let (text, runs, font_size) = display(area, window);
        let line_height = window.line_height();
        let lines = window
            .text_system()
            .shape_text(text, font_size, &runs, Some(bounds.size.width), None)
            .unwrap_or_default();
        let mut paragraphs = Vec::with_capacity(lines.len());
        let (mut start, mut top) = (0, Pixels::ZERO);
        for line in lines {
            let len = line.len();
            let height = line_height * (line.wrap_boundaries().len() + 1) as f32;
            paragraphs.push(Paragraph { line, start, top });
            start += len + 1;
            top += height;
        }
        let layout = Layout {
            paragraphs,
            bounds,
            line_height,
            content: area.content.clone(),
        };
        let colors = area.colors;
        let sel = area.selected_range.clone();
        let mut quads = Vec::new();
        let cursor = if sel.is_empty() || area.content.is_empty() {
            let at = layout.position(area.cursor_offset());
            let max_x = (bounds.size.width - px(1.5)).max(Pixels::ZERO);
            Some(fill(
                Bounds::new(
                    bounds.origin + point(at.x.min(max_x), at.y),
                    size(px(1.5), line_height),
                ),
                colors.caret,
            ))
        } else {
            let (a, b) = (layout.position(sel.start), layout.position(sel.end));
            let o = bounds.origin;
            let w = bounds.size.width;
            let ax = a.x.min(w);
            let bx = b.x.min(w);
            if a.y == b.y {
                quads.push(fill(
                    Bounds::from_corners(o + point(ax, a.y), o + point(bx, b.y + line_height)),
                    colors.selection,
                ));
            } else {
                quads.push(fill(
                    Bounds::from_corners(o + point(ax, a.y), o + point(w, a.y + line_height)),
                    colors.selection,
                ));
                if b.y > a.y + line_height {
                    quads.push(fill(
                        Bounds::from_corners(
                            o + point(px(0.), a.y + line_height),
                            o + point(w, b.y),
                        ),
                        colors.selection,
                    ));
                }
                quads.push(fill(
                    Bounds::from_corners(o + point(px(0.), b.y), o + point(bx, b.y + line_height)),
                    colors.selection,
                ));
            }
            None
        };
        let Layout { paragraphs, .. } = layout;
        Prepainted {
            paragraphs,
            quads,
            cursor,
            line_height,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.area.read(cx).focus_handle.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.area.clone()),
            cx,
        );
        let focused = focus.is_focused(window);
        if focused {
            for q in prepaint.quads.drain(..) {
                window.paint_quad(q);
            }
        }
        for p in &prepaint.paragraphs {
            let origin = point(bounds.origin.x, bounds.origin.y + p.top);
            if let Err(e) = p.line.paint(
                origin,
                prepaint.line_height,
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            ) {
                tracing::debug!("painting notes failed: {e:#}");
            }
        }
        if focused && let Some(c) = prepaint.cursor.take() {
            window.paint_quad(c);
        }
        let paragraphs = std::mem::take(&mut prepaint.paragraphs);
        let line_height = prepaint.line_height;
        self.area.update(cx, |area, _| {
            area.layout = Some(Layout {
                paragraphs,
                bounds,
                line_height,
                content: area.content.clone(),
            });
        });
    }
}

impl Render for TextArea {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .min_w_0()
            .key_context("TextArea")
            .track_focus(&self.focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::done))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::soft_newline))
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::edit_word))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(AreaElement { area: cx.entity() })
    }
}

impl Focusable for TextArea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn one_line(text: SharedString) -> SharedString {
    if text
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
    {
        crate::model::labels::one_line(&text).into()
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_shortcuts_work_across_paragraphs() {
        let mut app = gpui::TestApp::new();
        app.update(|cx| cx.bind_keys(key_bindings()));
        let mut window = app.open_window(|window, cx| {
            use crate::theme::{self, Theme};
            let colors = crate::ui::shell::quick_add_colors(&theme::palette(
                Theme::Light,
                theme::hues::MY_DAY,
            ));
            let mut area = TextArea::new("", colors, cx);
            area.set_text("One word\n\nTwo words", cx);
            window.focus(&area.focus_handle, cx);
            area
        });
        window.draw();
        window.simulate_keystrokes("ctrl-left ctrl-left ctrl-shift-left");
        window.update(|area, _, _| {
            assert_eq!(&area.content[area.selected_range.clone()], "word\n\n");
            assert!(area.selection_reversed);
        });
        window.simulate_keystroke("ctrl-delete");
        window.update(|area, _, _| assert_eq!(area.content.as_ref(), "One Two words"));
        window.simulate_keystrokes("ctrl-right ctrl-backspace");
        window.update(|area, _, _| assert_eq!(area.content.as_ref(), "One words"));
    }

    #[test]
    fn single_paragraph_area_wraps_long_text_and_keeps_one_line_on_set_and_replace() {
        struct NarrowHost {
            area: Entity<TextArea>,
        }
        impl Render for NarrowHost {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().w(px(220.)).child(self.area.clone())
            }
        }
        let mut app = gpui::TestApp::new();
        app.update(|cx| cx.bind_keys(key_bindings()));
        let mut area_entity = None;
        let mut window = app.open_window(|window, cx| {
            use crate::theme::{self, Theme};
            let colors = crate::ui::shell::quick_add_colors(&theme::palette(
                Theme::Light,
                theme::hues::MY_DAY,
            ));
            let area = cx.new(|cx| {
                let mut area = TextArea::new("Add step", colors, cx);
                area.single = true;
                area.set_text(
                    "Need to look at when exactly session starts and ends\nacross every client",
                    cx,
                );
                area.select_everything(cx);
                area
            });
            let focus = area.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
            area_entity = Some(area.clone());
            NarrowHost { area }
        });
        window.draw();
        let area = area_entity.expect("area");
        window.update(|_, _, cx| {
            let area = area.read(cx);
            assert_eq!(
                area.content.as_ref(),
                "Need to look at when exactly session starts and ends across every client"
            );
            let layout = area.layout.as_ref().expect("layout");
            assert_eq!(layout.paragraphs.len(), 1);
            assert!(
                !layout.paragraphs[0].line.wrap_boundaries().is_empty(),
                "long single-paragraph text must wrap inside a narrow container"
            );
            assert!(layout.bounds.size.width <= px(220.));
            assert!(layout.bounds.size.height > layout.line_height);
        });
    }
}
