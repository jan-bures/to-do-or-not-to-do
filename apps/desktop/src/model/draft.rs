//! An editor's unsaved text. The title and notes editors save when editing ends; a save that fails (the
//! local cache couldn't be written) keeps the draft, so the text stays in the editor and the next save
//! tries again. Text arriving from the store never replaces a draft.

/// Whether an editor holds text the store doesn't have yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    dirty: bool,
}

impl Draft {
    /// The owner typed.
    pub fn edited(&mut self) {
        self.dirty = true;
    }

    /// The owner threw the draft away (Escape), or another task's text was loaded.
    pub fn discard(&mut self) {
        self.dirty = false;
    }

    #[cfg(test)]
    pub fn is_dirty(self) -> bool {
        self.dirty
    }

    /// Whether text from the store may replace the editor's: only when nothing is unsaved.
    pub fn accepts_incoming(self) -> bool {
        !self.dirty
    }

    /// Saves the draft, if there is one, with `save`, which returns whether the store took it. The
    /// draft is cleared only then. Returns whether nothing is left unsaved.
    pub fn commit(&mut self, save: impl FnOnce() -> bool) -> bool {
        if self.dirty && save() {
            self.dirty = false;
        }
        !self.dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures;
    use crate::model::commands;
    use crate::store::Store;

    #[test]
    fn a_failed_save_keeps_the_draft_for_a_retry() {
        let fixture = fixtures::by_name("my-day").expect("fixture");
        let store = Store::fixture(&fixture);
        let id = *store.view().tasks.keys().next().expect("a task");
        let save = |store: &Store, text: &str| {
            commands::retitle(&store.view(), id, text).is_none_or(|cmd| store.submit(cmd).is_ok())
        };

        let mut draft = Draft::default();
        assert!(draft.commit(|| unreachable!("nothing to save")));
        draft.edited();
        assert!(!draft.accepts_incoming());

        // The cache can't be written: the draft stays, and so does the text.
        store.refuse_submits(true);
        assert!(!draft.commit(|| save(&store, "Renamed while the disk was full")));
        assert!(draft.is_dirty() && !draft.accepts_incoming());
        assert_ne!(
            store.view().tasks[&id].title,
            "Renamed while the disk was full"
        );

        // The next attempt saves it.
        store.refuse_submits(false);
        assert!(draft.commit(|| save(&store, "Renamed while the disk was full")));
        assert!(!draft.is_dirty() && draft.accepts_incoming());
        assert_eq!(
            store.view().tasks[&id].title,
            "Renamed while the disk was full"
        );
    }

    #[test]
    fn discarding_drops_the_draft() {
        let mut draft = Draft::default();
        draft.edited();
        draft.discard();
        assert!(draft.accepts_incoming());
        assert!(draft.commit(|| unreachable!("nothing to save")));
    }
}
