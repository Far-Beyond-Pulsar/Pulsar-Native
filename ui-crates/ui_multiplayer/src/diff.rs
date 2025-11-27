//! Line-by-line diff algorithm for file comparison
//!
//! Implements a Myers diff algorithm for efficient line-level diffing with O(ND) complexity.
//! Used for displaying file changes in the multiuser sync UI.

use std::cmp::min;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffOperation {
    Equal { line: String },
    Insert { line: String },
    Delete { line: String },
}

#[derive(Debug, Clone)]
pub struct LineDiff {
    pub operations: Vec<DiffOperation>,
}

impl LineDiff {
    pub fn new() -> Self {
        Self {
            operations: Vec::new(),
        }
    }

    /// Compute line-by-line diff between two texts using Myers algorithm
    pub fn compute(old_text: &str, new_text: &str) -> Self {
        let old_lines: Vec<&str> = old_text.lines().collect();
        let new_lines: Vec<&str> = new_text.lines().collect();

        let operations = myers_diff(&old_lines, &new_lines);

        Self { operations }
    }

    /// Get statistics about the diff
    pub fn stats(&self) -> DiffStats {
        let mut additions = 0;
        let mut deletions = 0;
        let mut unchanged = 0;

        for op in &self.operations {
            match op {
                DiffOperation::Insert { .. } => additions += 1,
                DiffOperation::Delete { .. } => deletions += 1,
                DiffOperation::Equal { .. } => unchanged += 1,
            }
        }

        DiffStats {
            additions,
            deletions,
            unchanged,
        }
    }

    /// Check if there are any changes
    pub fn has_changes(&self) -> bool {
        self.operations.iter().any(|op| matches!(op, DiffOperation::Insert { .. } | DiffOperation::Delete { .. }))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DiffStats {
    pub additions: usize,
    pub deletions: usize,
    pub unchanged: usize,
}

impl DiffStats {
    pub fn total_changes(&self) -> usize {
        self.additions + self.deletions
    }
}

/// Myers diff algorithm implementation
/// Returns a sequence of diff operations
fn myers_diff(old_lines: &[&str], new_lines: &[&str]) -> Vec<DiffOperation> {
    let n = old_lines.len();
    let m = new_lines.len();
    let max_d = n + m;

    // V contains the furthest reaching x for each k
    let mut v: HashMap<isize, isize> = HashMap::new();
    v.insert(1, 0);

    // Trace contains the V vectors for each d
    let mut trace: Vec<HashMap<isize, isize>> = Vec::new();

    // Find the shortest edit script
    for d in 0..=max_d {
        let mut current_v = v.clone();

        for k in (-(d as isize)..=(d as isize)).step_by(2) {
            let mut x = if k == -(d as isize) || (k != d as isize && v.get(&(k - 1)).unwrap_or(&-1) < v.get(&(k + 1)).unwrap_or(&-1)) {
                *v.get(&(k + 1)).unwrap_or(&0)
            } else {
                v.get(&(k - 1)).unwrap_or(&0) + 1
            };

            let mut y = x - k;

            while x < n as isize && y < m as isize && old_lines[x as usize] == new_lines[y as usize] {
                x += 1;
                y += 1;
            }

            current_v.insert(k, x);

            if x >= n as isize && y >= m as isize {
                trace.push(current_v);
                return backtrack(&trace, old_lines, new_lines);
            }
        }

        trace.push(current_v.clone());
        v = current_v;
    }

    // Fallback: If no path found, treat as complete replacement
    let mut ops = Vec::new();
    for line in old_lines {
        ops.push(DiffOperation::Delete { line: line.to_string() });
    }
    for line in new_lines {
        ops.push(DiffOperation::Insert { line: line.to_string() });
    }
    ops
}

/// Backtrack through the trace to construct the diff
fn backtrack(trace: &[HashMap<isize, isize>], old_lines: &[&str], new_lines: &[&str]) -> Vec<DiffOperation> {
    let mut x = old_lines.len() as isize;
    let mut y = new_lines.len() as isize;
    let mut operations = Vec::new();

    for d in (0..trace.len()).rev() {
        let v = &trace[d];
        let k = x - y;

        let prev_k = if k == -(d as isize) || (k != d as isize && v.get(&(k - 1)).unwrap_or(&-1) < v.get(&(k + 1)).unwrap_or(&-1)) {
            k + 1
        } else {
            k - 1
        };

        let prev_x = *v.get(&prev_k).unwrap_or(&0);
        let prev_y = prev_x - prev_k;

        while x > prev_x && y > prev_y {
            operations.push(DiffOperation::Equal {
                line: old_lines[(x - 1) as usize].to_string(),
            });
            x -= 1;
            y -= 1;
        }

        if d > 0 {
            if x == prev_x {
                operations.push(DiffOperation::Insert {
                    line: new_lines[(y - 1) as usize].to_string(),
                });
                y -= 1;
            } else {
                operations.push(DiffOperation::Delete {
                    line: old_lines[(x - 1) as usize].to_string(),
                });
                x -= 1;
            }
        }
    }

    operations.reverse();
    operations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_insertion() {
        let old = "line 1\nline 2";
        let new = "line 1\nline 1.5\nline 2";

        let diff = LineDiff::compute(old, new);
        let stats = diff.stats();

        assert_eq!(stats.additions, 1);
        assert_eq!(stats.deletions, 0);
        assert_eq!(stats.unchanged, 2);
    }

    #[test]
    fn test_simple_deletion() {
        let old = "line 1\nline 2\nline 3";
        let new = "line 1\nline 3";

        let diff = LineDiff::compute(old, new);
        let stats = diff.stats();

        assert_eq!(stats.additions, 0);
        assert_eq!(stats.deletions, 1);
        assert_eq!(stats.unchanged, 2);
    }

    #[test]
    fn test_modification() {
        let old = "line 1\nline 2\nline 3";
        let new = "line 1\nline 2 modified\nline 3";

        let diff = LineDiff::compute(old, new);
        let stats = diff.stats();

        assert_eq!(stats.additions, 1);
        assert_eq!(stats.deletions, 1);
        assert_eq!(stats.unchanged, 2);
    }

    #[test]
    fn test_no_changes() {
        let text = "line 1\nline 2\nline 3";

        let diff = LineDiff::compute(text, text);
        let stats = diff.stats();

        assert_eq!(stats.additions, 0);
        assert_eq!(stats.deletions, 0);
        assert!(!diff.has_changes());
    }
}
