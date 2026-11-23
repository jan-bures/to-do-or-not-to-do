//! Shared room-name search. Callers choose how to lay out the matching rooms.

use todo_core::query::fold;

/// A case- and accent-insensitive substring, independent of the room's kind.
#[derive(Debug)]
pub struct Filter(String);

impl Filter {
    pub fn new(query: &str) -> Self {
        Self(fold(query.trim()))
    }

    pub fn matches(&self, name: &str) -> bool {
        self.0.is_empty() || fold(name).contains(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Filter;

    #[test]
    fn matches_trimmed_substrings_with_the_shared_fold() {
        let filter = Filter::new("  ZLUTE  ");
        assert!(filter.matches("Ty žluté složky"));
        assert!(!filter.matches("Žlutá"));
        assert!(Filter::new("DAY").matches("My Day"));
        assert!(Filter::new(" sh ").matches("Trash"));
        assert!(Filter::new("acti").matches("Activity"));
        assert!(Filter::new(" ").matches("Anything"));
        assert!(!Filter::new("yellow folders").matches("yellow project folders"));
    }
}
