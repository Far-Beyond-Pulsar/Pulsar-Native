pub mod actions;
mod drag_drop;
pub mod fs_metadata;
pub mod helpers;
pub mod operations;
mod rename;
mod state;
mod thumbnails;
pub mod tree;
pub mod types;

pub use actions::*;
pub use drag_drop::*;
pub use helpers::*;
pub use rename::*;
pub use state::*;
pub use thumbnails::*;
pub use tree::FolderNode;
pub use types::*;

use std::path::{Path, PathBuf};

pub fn cloud_join(base: &Path, component: &str) -> PathBuf {
    if engine_fs::is_cloud_path(base) {
        let s = base.to_string_lossy().replace('\\', "/");
        PathBuf::from(format!("{}/{}", s.trim_end_matches('/'), component))
    } else {
        base.join(component)
    }
}
