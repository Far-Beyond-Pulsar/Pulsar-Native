//! Class identity: a GUID per class directory, kept in `class.json`.
//!
//! The GUID is generated the first time a class is saved (by the Blueprint
//! editor) or scanned (by [`crate::ClassRegistry`]) and never changes after
//! that, so renaming or moving a class keeps every level reference valid.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Class metadata file name inside a class directory.
pub const CLASS_META_FILE: &str = "class.json";

/// A class's stable GUID. Serialized as a plain string.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClassId(pub String);

impl ClassId {
    /// A fresh random id.
    pub fn new_random() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `true` for the empty id an unresolved or not-yet-migrated instance
    /// may carry.
    pub fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }
}

impl std::fmt::Display for ClassId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for ClassId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for ClassId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Contents of `class.json`. Unknown keys are preserved on rewrite.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ClassMeta {
    #[serde(default)]
    pub class_id: ClassId,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl ClassMeta {
    /// Read `<dir>/class.json`. `None` when missing or unreadable.
    pub fn read(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join(CLASS_META_FILE)).ok()?;
        match serde_json::from_str::<Self>(&text) {
            Ok(meta) => Some(meta),
            Err(error) => {
                tracing::warn!(dir = %dir.display(), "Unreadable class.json: {error}");
                None
            }
        }
    }

    /// Write `<dir>/class.json`.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(dir.join(CLASS_META_FILE), text)
    }
}

/// The class id stored in `dir`, if any.
pub fn read_class_id(dir: &Path) -> Option<ClassId> {
    ClassMeta::read(dir)
        .map(|meta| meta.class_id)
        .filter(|id| !id.is_empty())
}

/// The class id stored in `dir`, generating and writing one when missing.
///
/// If the directory is read-only (a packaged game), the id falls back to a
/// deterministic `name:<dir name>` form so lookups stay stable across runs.
pub fn ensure_class_id(dir: &Path) -> ClassId {
    if let Some(id) = read_class_id(dir) {
        return id;
    }
    let mut meta = ClassMeta::read(dir).unwrap_or_default();
    meta.class_id = ClassId::new_random();
    match meta.write(dir) {
        Ok(()) => {
            tracing::info!(dir = %dir.display(), id = %meta.class_id, "Assigned class id");
            meta.class_id
        }
        Err(error) => {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            tracing::warn!(
                dir = %dir.display(),
                "Could not write class.json ({error}); using a name-derived class id"
            );
            fallback_class_id(name)
        }
    }
}

/// Deterministic id used when `class.json` cannot be written.
pub fn fallback_class_id(class_name: &str) -> ClassId {
    ClassId(format!("name:{class_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_generates_once_and_keeps_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = ensure_class_id(dir.path());
        assert!(!first.is_empty());
        assert_eq!(ensure_class_id(dir.path()), first, "stable across calls");
        assert_eq!(read_class_id(dir.path()), Some(first));
    }

    #[test]
    fn unknown_keys_survive_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(CLASS_META_FILE), r#"{"note":"keep"}"#).unwrap();
        let id = ensure_class_id(dir.path());
        let text = std::fs::read_to_string(dir.path().join(CLASS_META_FILE)).unwrap();
        assert!(text.contains("keep"));
        assert!(text.contains(id.as_str()));
    }
}
