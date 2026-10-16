//! Flattening of path lists into folder trees for virtualized lists.

use std::collections::{HashMap, HashSet};

/// One row of a flattened path tree.
#[derive(Debug, Clone)]
pub enum TreeItem<T> {
    Folder {
        /// Stable key (prefix + folder path) used for the collapsed set.
        key: String,
        name: String,
        depth: usize,
        /// Number of leaves below the folder.
        count: usize,
        collapsed: bool,
    },
    Leaf {
        item: T,
        depth: usize,
    },
}

fn compare_paths(a: &str, b: &str) -> std::cmp::Ordering {
    let a: Vec<&str> = a.split('/').collect();
    let b: Vec<&str> = b.split('/').collect();
    for i in 0..a.len().min(b.len()) {
        if a[i] != b[i] {
            // At the same level, folders sort before files.
            let a_dir = i + 1 < a.len();
            let b_dir = i + 1 < b.len();
            return b_dir
                .cmp(&a_dir)
                .then_with(|| a[i].to_lowercase().cmp(&b[i].to_lowercase()))
                .then_with(|| a[i].cmp(b[i]));
        }
    }
    a.len().cmp(&b.len())
}

/// Turn `(path, payload)` pairs into rows. With `tree` off the result is the
/// flat, sorted list of leaves.
pub fn build_tree<T: Clone>(
    items: &[(String, T)],
    prefix: &str,
    collapsed: &HashSet<String>,
    tree: bool,
) -> Vec<TreeItem<T>> {
    let mut sorted: Vec<&(String, T)> = items.iter().collect();
    sorted.sort_by(|a, b| compare_paths(&a.0, &b.0));

    if !tree {
        sorted.sort_by_key(|entry| entry.0.to_lowercase());
        return sorted
            .into_iter()
            .map(|(_, item)| TreeItem::Leaf {
                item: item.clone(),
                depth: 0,
            })
            .collect();
    }

    let key_of = |dirs: &[&str]| format!("{}:{}", prefix, dirs.join("/"));

    let mut counts: HashMap<String, usize> = HashMap::new();
    for (path, _) in &sorted {
        let parts: Vec<&str> = path.split('/').collect();
        for depth in 1..parts.len() {
            *counts.entry(key_of(&parts[..depth])).or_default() += 1;
        }
    }

    let mut rows = Vec::new();
    let mut previous: Vec<&str> = Vec::new();
    for (path, item) in sorted {
        let parts: Vec<&str> = path.split('/').collect();
        let dirs = &parts[..parts.len() - 1];
        let common = dirs
            .iter()
            .zip(previous.iter())
            .take_while(|(a, b)| a == b)
            .count();

        let mut hidden = false;
        for depth in 0..dirs.len() {
            let key = key_of(&dirs[..=depth]);
            let is_collapsed = collapsed.contains(&key);
            if depth >= common {
                rows.push(TreeItem::Folder {
                    name: dirs[depth].to_string(),
                    depth,
                    count: counts.get(&key).copied().unwrap_or(0),
                    collapsed: is_collapsed,
                    key: key.clone(),
                });
            }
            if is_collapsed {
                hidden = true;
                break;
            }
        }
        // Remember how far the visible rows reached, so a sibling below a
        // collapsed folder does not emit that folder a second time.
        previous = dirs.to_vec();
        if !hidden {
            rows.push(TreeItem::Leaf {
                item: item.clone(),
                depth: dirs.len(),
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(paths: &[&str]) -> Vec<(String, usize)> {
        paths
            .iter()
            .enumerate()
            .map(|(i, p)| (p.to_string(), i))
            .collect()
    }

    fn shape(rows: &[TreeItem<usize>]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                TreeItem::Folder { name, depth, .. } => format!("{}d:{}", depth, name),
                TreeItem::Leaf { item, depth } => format!("{}f:{}", depth, item),
            })
            .collect()
    }

    #[test]
    fn folders_come_first_and_group_their_files() {
        let rows = build_tree(
            &items(&["b.txt", "src/a.rs", "src/ui/x.rs", "a.txt"]),
            "t",
            &HashSet::new(),
            true,
        );
        assert_eq!(
            shape(&rows),
            ["0d:src", "1d:ui", "2f:2", "1f:1", "0f:3", "0f:0"]
        );
    }

    #[test]
    fn collapsed_folder_hides_children_without_repeating() {
        let mut collapsed = HashSet::new();
        collapsed.insert("t:src".to_string());
        let rows = build_tree(
            &items(&["src/a.rs", "src/b.rs", "src/ui/x.rs", "z.txt"]),
            "t",
            &collapsed,
            true,
        );
        assert_eq!(shape(&rows), ["0d:src", "0f:3"]);
    }

    #[test]
    fn flat_mode_lists_sorted_leaves() {
        let rows = build_tree(&items(&["b/x", "a"]), "t", &HashSet::new(), false);
        assert_eq!(shape(&rows), ["0f:1", "0f:0"]);
    }
}
