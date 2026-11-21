//! Public row, query and dependency types of the project file index.

use std::path::PathBuf;

/// One typed reference from an asset to another, as recorded by the owning
/// format's extractor. Kinds are `AssetKind::KIND` strings; `role` is `hard`
/// (needed to load/evaluate) or `hint` (editor-only convenience, ignored by
/// cycle checks); `via` names the authored field; `expect` is the format's
/// compatibility rule for the edge (opaque to the index).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetDep {
    pub source_kind: String,
    pub source_id: String,
    pub target_kind: String,
    pub target_id: String,
    pub role: String,
    pub via: String,
    pub expect: String,
}

/// A recorded edge together with where the referencing asset lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetDependent {
    pub path: PathBuf,
    pub dep: AssetDep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedFile {
    /// Forward-slash-separated path relative to the project root.
    pub path: String,
    pub name: String,
    pub extension: Option<String>,
    pub file_type_id: Option<String>,
    pub is_dir: bool,
    pub size: u64,
    pub created: Option<i64>,
    pub modified: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileIndexSort {
    Path,
    Name,
    Extension,
    FileType,
    Size,
    Created,
    Modified,
}

#[derive(Debug, Clone, Default)]
pub struct FileIndexQuery {
    pub file_type_id: Option<String>,
    pub extension: Option<String>,
    /// Inclusive lower bound, Unix seconds.
    pub modified_after: Option<i64>,
    /// Inclusive upper bound, Unix seconds.
    pub modified_before: Option<i64>,
    pub name_contains: Option<String>,
    /// Match this folder and all descendants. Empty means the project root.
    pub folder: Option<String>,
    /// Restrict results to direct children of this folder (for file listings).
    pub direct_children_of: Option<String>,
    /// Exact file name match; useful for marker files.
    pub name_equals: Option<String>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub include_directories: bool,
    pub sort: FileIndexSort,
    pub descending: bool,
    pub limit: Option<usize>,
}

impl Default for FileIndexSort {
    fn default() -> Self {
        Self::Path
    }
}
