//! Back / Forward history over opened files.

use std::path::PathBuf;

/// Most entries kept; the oldest are dropped past this.
const MAX_ENTRIES: usize = 100;

/// A browser-style history: visiting a new file after going back discards the
/// forward entries, and re-visiting the current file adds nothing.
#[derive(Default)]
pub struct NavigationHistory {
    entries: Vec<PathBuf>,
    /// Index of the entry currently shown.
    current: Option<usize>,
}

impl NavigationHistory {
    /// Record a visit to `path`.
    pub fn visit(&mut self, path: PathBuf) {
        if self.current.and_then(|ix| self.entries.get(ix)) == Some(&path) {
            return;
        }
        self.entries.truncate(self.current.map_or(0, |ix| ix + 1));
        self.entries.push(path);
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.current = Some(self.entries.len() - 1);
    }

    /// Step back, returning the entry to show.
    pub fn back(&mut self) -> Option<PathBuf> {
        let ix = self.current?.checked_sub(1)?;
        self.current = Some(ix);
        self.entries.get(ix).cloned()
    }

    /// Step forward, returning the entry to show.
    pub fn forward(&mut self) -> Option<PathBuf> {
        let ix = self.current? + 1;
        let path = self.entries.get(ix)?.clone();
        self.current = Some(ix);
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(name)
    }

    #[test]
    fn back_goes_to_the_previous_file_not_the_current_one() {
        let mut h = NavigationHistory::default();
        h.visit(p("a"));
        h.visit(p("b"));
        h.visit(p("c"));
        assert_eq!(h.back(), Some(p("b")));
        assert_eq!(h.back(), Some(p("a")));
        assert_eq!(h.back(), None, "nothing before the first entry");
        assert_eq!(h.forward(), Some(p("b")));
        assert_eq!(h.forward(), Some(p("c")));
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn visiting_after_going_back_drops_the_forward_entries() {
        let mut h = NavigationHistory::default();
        h.visit(p("a"));
        h.visit(p("b"));
        h.visit(p("c"));
        h.back();
        h.back();
        h.visit(p("d"));
        assert_eq!(h.forward(), None);
        assert_eq!(h.back(), Some(p("a")));
    }

    #[test]
    fn revisiting_the_current_file_adds_nothing() {
        let mut h = NavigationHistory::default();
        h.visit(p("a"));
        h.visit(p("a"));
        assert_eq!(h.back(), None);
    }

    #[test]
    fn empty_history_has_nowhere_to_go() {
        let mut h = NavigationHistory::default();
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), None);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = NavigationHistory::default();
        for i in 0..(MAX_ENTRIES + 20) {
            h.visit(p(&i.to_string()));
        }
        let mut steps = 0;
        while h.back().is_some() {
            steps += 1;
        }
        assert_eq!(steps, MAX_ENTRIES - 1);
    }
}
