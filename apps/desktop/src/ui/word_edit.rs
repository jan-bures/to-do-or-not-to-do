//! Word editing shared by the desktop text fields. Offsets are UTF-8 byte indices.

use gpui::{Action, KeyBinding};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Action, Clone, Copy, Debug, PartialEq)]
#[action(namespace = text_edit, no_json)]
pub(super) enum WordEdit {
    Left,
    Right,
    SelectLeft,
    SelectRight,
    DeleteLeft,
    DeleteRight,
}

pub(super) fn key_bindings(context: &'static str) -> [KeyBinding; 6] {
    use WordEdit::*;
    [
        ("ctrl-left", Left),
        ("ctrl-right", Right),
        ("ctrl-shift-left", SelectLeft),
        ("ctrl-shift-right", SelectRight),
        ("ctrl-backspace", DeleteLeft),
        ("ctrl-delete", DeleteRight),
    ]
    .map(|(key, action)| KeyBinding::new(key, action, Some(context)))
}

impl WordEdit {
    pub(super) fn target(self, text: &str, offset: usize) -> usize {
        // Windows moves to word starts in both directions. Keep punctuation and emoji as
        // stops too, and skip whitespace, including line breaks in multiline fields.
        let mut starts = text
            .split_word_bound_indices()
            .filter(|(_, segment)| !segment.chars().all(char::is_whitespace))
            .map(|(start, _)| start);
        match self {
            Self::Left | Self::SelectLeft | Self::DeleteLeft => {
                starts.rfind(|&start| start < offset).unwrap_or(0)
            }
            Self::Right | Self::SelectRight | Self::DeleteRight => {
                starts.find(|&start| start > offset).unwrap_or(text.len())
            }
        }
    }
}
