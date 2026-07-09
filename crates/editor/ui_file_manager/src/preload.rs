//! Pre-built folder-tree store.
//!
//! The loading-screen background thread calls [`store_preloaded_tree`] with the
//! result of [`crate::drawer::FolderNode::from_path`] so that
//! [`crate::FileManagerDrawer`] can take it in its constructor with zero disk I/O.
//!
//! The store is a write-once / take-once latch: after the first `take`, it
//! returns `None` forever so subsequent opens (e.g. project-switcher) fall
//! back to the synchronous path.

use std::sync::Mutex;

use crate::drawer::FolderNode;

static PRELOADED_TREE: Mutex<Option<FolderNode>> = Mutex::new(None);

/// Store a pre-built folder tree.  Called from the loading-screen thread.
pub fn store_preloaded_tree(tree: Option<FolderNode>) {
    if let Ok(mut g) = PRELOADED_TREE.lock() {
        *g = tree;
    }
}

/// Drain the pre-built folder tree.  Returns `None` if never populated or
/// already taken.  Called once from [`crate::FileManagerDrawer::new`].
pub fn take_preloaded_tree() -> Option<FolderNode> {
    PRELOADED_TREE.lock().ok()?.take()
}
