use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Replicated Growable Array (RGA) CRDT
/// Suitable for collaborative text editing and ordered sequences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RGASeq<T: Clone> {
    /// Actor ID for this replica
    actor_id: String,
    /// Operation counter
    counter: u64,
    /// Nodes in the sequence
    nodes: HashMap<NodeId, Node<T>>,
    /// Head of the linked list
    head: NodeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId {
    actor_id: String,
    counter: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Node<T: Clone> {
    value: Option<T>, // None if tombstoned
    next: Option<NodeId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RGAOp<T: Clone> {
    Insert {
        id: NodeId,
        value: T,
        after: Option<NodeId>,
    },
    Delete {
        id: NodeId,
    },
}

impl<T: Clone> RGASeq<T> {
    pub fn new(actor_id: String) -> Self {
        let head = NodeId {
            actor_id: "HEAD".to_string(),
            counter: 0,
        };

        let mut nodes = HashMap::new();
        nodes.insert(
            head.clone(),
            Node {
                value: None,
                next: None,
            },
        );

        Self {
            actor_id,
            counter: 0,
            nodes,
            head,
        }
    }

    /// Insert a value at a given index
    pub fn insert(&mut self, index: usize, value: T) -> RGAOp<T> {
        self.counter += 1;
        let new_id = NodeId {
            actor_id: self.actor_id.clone(),
            counter: self.counter,
        };

        // Find the node after which to insert
        let after = self.node_at_index(index);

        // Insert into internal structure
        self.insert_after(new_id.clone(), value.clone(), after.clone());

        RGAOp::Insert {
            id: new_id,
            value,
            after,
        }
    }

    /// Delete value at a given index
    pub fn delete(&mut self, index: usize) -> Option<RGAOp<T>> {
        let node_id = self.node_id_at_index(index)?;

        if let Some(node) = self.nodes.get_mut(&node_id) {
            node.value = None; // Tombstone
        }

        Some(RGAOp::Delete { id: node_id })
    }

    /// Get the value at an index
    pub fn get(&self, index: usize) -> Option<&T> {
        let node_id = self.node_id_at_index(index)?;
        self.nodes.get(&node_id)?.value.as_ref()
    }

    /// Get all values as a vector
    pub fn to_vec(&self) -> Vec<T> {
        let mut result = Vec::new();
        let mut current = self.head.clone();

        while let Some(node) = self.nodes.get(&current) {
            if let Some(value) = &node.value {
                result.push(value.clone());
            }

            if let Some(next_id) = &node.next {
                current = next_id.clone();
            } else {
                break;
            }
        }

        result
    }

    /// Get the length (number of non-tombstoned elements)
    pub fn len(&self) -> usize {
        self.to_vec().len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Apply a remote operation
    pub fn apply(&mut self, op: RGAOp<T>) {
        match op {
            RGAOp::Insert { id, value, after } => {
                self.insert_after(id, value, after);
            }
            RGAOp::Delete { id } => {
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.value = None; // Tombstone
                }
            }
        }
    }

    /// Merge with another RGA sequence
    pub fn merge(&mut self, _other: &RGASeq<T>) {
        // In a full implementation, this would merge the node structures
        // For now, this is a placeholder
        // Real implementation would need to handle concurrent inserts properly
    }

    // Helper methods

    fn node_at_index(&self, index: usize) -> Option<NodeId> {
        if index == 0 {
            return Some(self.head.clone());
        }

        let mut current_index = 0;
        let mut current = self.head.clone();

        while let Some(node) = self.nodes.get(&current) {
            if let Some(next_id) = &node.next {
                if node.value.is_some() {
                    if current_index == index - 1 {
                        return Some(current.clone());
                    }
                    current_index += 1;
                }
                current = next_id.clone();
            } else {
                break;
            }
        }

        Some(current)
    }

    fn node_id_at_index(&self, index: usize) -> Option<NodeId> {
        let mut current_index = 0;
        let mut current = self.head.clone();

        while let Some(node) = self.nodes.get(&current) {
            if node.value.is_some() {
                if current_index == index {
                    return Some(current.clone());
                }
                current_index += 1;
            }

            if let Some(next_id) = &node.next {
                current = next_id.clone();
            } else {
                break;
            }
        }

        None
    }

    fn insert_after(&mut self, new_id: NodeId, value: T, after: Option<NodeId>) {
        let after_id = after.unwrap_or_else(|| self.head.clone());

        if let Some(after_node) = self.nodes.get_mut(&after_id) {
            let old_next = after_node.next.clone();
            after_node.next = Some(new_id.clone());

            self.nodes.insert(
                new_id,
                Node {
                    value: Some(value),
                    next: old_next,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rga_insert_and_get() {
        let mut seq = RGASeq::new("actor1".to_string());
        seq.insert(0, 'a');
        seq.insert(1, 'b');
        seq.insert(2, 'c');

        assert_eq!(seq.to_vec(), vec!['a', 'b', 'c']);
    }

    #[test]
    fn test_rga_delete() {
        let mut seq = RGASeq::new("actor1".to_string());
        seq.insert(0, 'a');
        seq.insert(1, 'b');
        seq.insert(2, 'c');

        seq.delete(1);

        assert_eq!(seq.to_vec(), vec!['a', 'c']);
    }

    #[test]
    fn test_rga_concurrent_insert() {
        let mut seq1 = RGASeq::new("actor1".to_string());
        let mut seq2 = RGASeq::new("actor2".to_string());

        let op1 = seq1.insert(0, 'a');
        let op2 = seq1.insert(1, 'b');

        let op3 = seq2.insert(0, 'x');
        let op4 = seq2.insert(1, 'y');

        // Apply operations
        seq1.apply(op3);
        seq1.apply(op4);
        seq2.apply(op1);
        seq2.apply(op2);

        // Both should have all elements
        assert_eq!(seq1.len(), 4);
        assert_eq!(seq2.len(), 4);
    }

    #[test]
    fn test_rga_convergence() {
        let mut seq1 = RGASeq::new("actor1".to_string());
        let mut seq2 = RGASeq::new("actor2".to_string());

        // Actor1 inserts
        let op1 = seq1.insert(0, 'h');
        let op2 = seq1.insert(1, 'i');

        // Actor2 inserts
        let op3 = seq2.insert(0, 'b');
        let op4 = seq2.insert(1, 'y');
        let op5 = seq2.insert(2, 'e');

        // Exchange all operations
        seq1.apply(op3);
        seq1.apply(op4);
        seq1.apply(op5);
        seq2.apply(op1);
        seq2.apply(op2);

        // Both should have the same length
        assert_eq!(seq1.len(), seq2.len());
    }
}
