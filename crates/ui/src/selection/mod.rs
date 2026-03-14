/// Unified selection abstraction for all collection components.
use std::collections::HashSet;

use crate::IndexPath;

/// The primary selection state type — covers single, multi, and range selection.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Selection<T: Clone + Eq + std::hash::Hash> {
    #[default]
    None,
    Single(T),
    Multi(HashSet<T>),
}

impl<T: Clone + Eq + std::hash::Hash> Selection<T> {
    pub fn select_single(item: T) -> Self {
        Selection::Single(item)
    }

    pub fn is_selected(&self, item: &T) -> bool {
        match self {
            Selection::None => false,
            Selection::Single(s) => s == item,
            Selection::Multi(set) => set.contains(item),
        }
    }

    pub fn selected_single(&self) -> Option<&T> {
        match self {
            Selection::Single(s) => Some(s),
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        *self = Selection::None;
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, Selection::None)
    }

    pub fn toggle(&mut self, item: T) {
        match self {
            Selection::None => *self = Selection::Single(item),
            Selection::Single(s) if *s == item => *self = Selection::None,
            Selection::Single(s) => {
                let mut set = HashSet::new();
                set.insert(s.clone());
                set.insert(item);
                *self = Selection::Multi(set);
            }
            Selection::Multi(set) => {
                if !set.remove(&item) {
                    set.insert(item);
                }
                if set.len() == 1 {
                    *self = Selection::Single(set.iter().next().unwrap().clone());
                }
            }
        }
    }
}

/// Simple `usize`-indexed selection (most common case for lists/tables).
pub type IndexSelection = Selection<usize>;

/// Selection keyed by [`IndexPath`] for sectioned/multi-dimensional lists.
///
/// `IndexPath` is a 3-field struct (section, row, column) and is not
/// directly substitutable for `usize`, so components using `Option<IndexPath>`
/// should migrate incrementally using this alias.
pub type IndexPathSelection = Selection<IndexPath>;

impl From<Option<IndexPath>> for IndexPathSelection {
    fn from(opt: Option<IndexPath>) -> Self {
        match opt {
            Some(ix) => Selection::Single(ix),
            None => Selection::None,
        }
    }
}

impl From<IndexPathSelection> for Option<IndexPath> {
    fn from(sel: IndexPathSelection) -> Self {
        sel.selected_single().copied()
    }
}
