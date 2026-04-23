use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::hash::Hash;

/// Observed-Remove Set (OR-Set) CRDT
/// Supports add and remove operations with eventual consistency
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ORSet<T: Clone + Eq + Hash> {
    /// Map from element to set of unique tags (actor_id, operation_id)
    elements: HashMap<T, HashSet<(String, u64)>>,
    /// Actor ID for this replica
    actor_id: String,
    /// Operation counter for this replica
    counter: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ORSetOp<T: Clone> {
    Add {
        element: T,
        tag: (String, u64),
    },
    Remove {
        element: T,
        tags: HashSet<(String, u64)>,
    },
}

impl<T: Clone + Eq + Hash + Debug> ORSet<T> {
    pub fn new(actor_id: String) -> Self {
        Self {
            elements: HashMap::new(),
            actor_id,
            counter: 0,
        }
    }

    /// Add an element to the set
    pub fn add(&mut self, element: T) -> ORSetOp<T> {
        self.counter += 1;
        let tag = (self.actor_id.clone(), self.counter);

        self.elements
            .entry(element.clone())
            .or_default()
            .insert(tag.clone());

        ORSetOp::Add { element, tag }
    }

    /// Remove an element from the set
    pub fn remove(&mut self, element: &T) -> Option<ORSetOp<T>> {
        if let Some(tags) = self.elements.get_mut(element) {
            // Only remove tags that this actor added
            let tags_to_remove: HashSet<_> = tags
                .iter()
                .filter(|(actor, _)| actor == &self.actor_id)
                .cloned()
                .collect();

            if tags_to_remove.is_empty() {
                return None;
            }

            // Remove those tags from the local set
            for tag in &tags_to_remove {
                tags.remove(tag);
            }

            // If no tags remain, remove the element entry
            if tags.is_empty() {
                self.elements.remove(element);
            }

            Some(ORSetOp::Remove {
                element: element.clone(),
                tags: tags_to_remove,
            })
        } else {
            None
        }
    }

    /// Check if an element is in the set
    pub fn contains(&self, element: &T) -> bool {
        self.elements
            .get(element)
            .is_some_and(|tags| !tags.is_empty())
    }

    /// Get all elements in the set
    pub fn elements(&self) -> Vec<&T> {
        self.elements
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .map(|(elem, _)| elem)
            .collect()
    }

    /// Get the size of the set
    pub fn len(&self) -> usize {
        self.elements
            .iter()
            .filter(|(_, tags)| !tags.is_empty())
            .count()
    }

    /// Check if the set is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Apply a remote operation
    pub fn apply(&mut self, op: ORSetOp<T>) {
        match op {
            ORSetOp::Add { element, tag } => {
                self.elements
                    .entry(element)
                    .or_default()
                    .insert(tag);
            }
            ORSetOp::Remove { element, tags } => {
                if let Some(element_tags) = self.elements.get_mut(&element) {
                    for tag in tags {
                        element_tags.remove(&tag);
                    }
                    if element_tags.is_empty() {
                        self.elements.remove(&element);
                    }
                }
            }
        }
    }

    /// Merge with another OR-Set
    pub fn merge(&mut self, other: &ORSet<T>) {
        for (element, other_tags) in &other.elements {
            let entry = self
                .elements
                .entry(element.clone())
                .or_default();
            for tag in other_tags {
                entry.insert(tag.clone());
            }
        }

        // Update counter to avoid conflicts
        self.counter = self.counter.max(other.counter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orset_add_and_contains() {
        let mut set = ORSet::new("actor1".to_string());
        set.add("hello");
        assert!(set.contains(&"hello"));
        assert!(!set.contains(&"world"));
    }

    #[test]
    fn test_orset_remove() {
        let mut set = ORSet::new("actor1".to_string());
        set.add("hello");
        assert!(set.contains(&"hello"));

        set.remove(&"hello");
        assert!(!set.contains(&"hello"));
    }

    #[test]
    fn test_orset_concurrent_add_remove() {
        // Simulate concurrent add and remove operations
        let mut set1 = ORSet::new("actor1".to_string());
        let mut set2 = ORSet::new("actor2".to_string());

        // Actor1 adds "x"
        let op1 = set1.add("x");

        // Actor2 also adds "x"
        let op2 = set2.add("x");

        // Apply operations
        set1.apply(op2.clone());
        set2.apply(op1.clone());

        // Both should have "x"
        assert!(set1.contains(&"x"));
        assert!(set2.contains(&"x"));

        // Actor1 removes "x" (only removes their own tag)
        let remove_op = set1.remove(&"x").unwrap();
        set2.apply(remove_op);

        // "x" should still exist because actor2's add is still present
        assert!(set2.contains(&"x"));
    }

    #[test]
    fn test_orset_merge() {
        let mut set1 = ORSet::new("actor1".to_string());
        let mut set2 = ORSet::new("actor2".to_string());

        set1.add("a");
        set1.add("b");

        set2.add("b");
        set2.add("c");

        set1.merge(&set2);

        assert!(set1.contains(&"a"));
        assert!(set1.contains(&"b"));
        assert!(set1.contains(&"c"));
        assert_eq!(set1.len(), 3);
    }

    #[test]
    fn test_orset_convergence() {
        // Test that replicas converge to the same state
        let mut set1 = ORSet::new("actor1".to_string());
        let mut set2 = ORSet::new("actor2".to_string());

        let op1_add_a = set1.add("a");
        let op1_add_b = set1.add("b");
        let op2_add_c = set2.add("c");
        let op2_add_d = set2.add("d");

        // Exchange operations
        set1.apply(op2_add_c.clone());
        set1.apply(op2_add_d.clone());
        set2.apply(op1_add_a.clone());
        set2.apply(op1_add_b.clone());

        // Both should have the same elements
        assert_eq!(set1.len(), set2.len());
        assert_eq!(set1.len(), 4);

        for elem in set1.elements() {
            assert!(set2.contains(elem));
        }
    }
}
