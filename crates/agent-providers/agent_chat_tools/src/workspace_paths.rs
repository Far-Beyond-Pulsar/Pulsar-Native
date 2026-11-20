//! Resolving a path against the workspace root without letting it escape.

use anyhow::{anyhow, Context};
use std::path::{Path, PathBuf};

pub(crate) fn resolve_workspace_path(root: &Path, rel_or_abs: &str) -> anyhow::Result<PathBuf> {
    let p = PathBuf::from(rel_or_abs);
    let joined = if p.is_absolute() { p } else { root.join(p) };
    let canonical = engine_fs::virtual_fs::canonicalize(&joined)
        .with_context(|| format!("Path does not exist: {}", joined.display()))?;

    let root_canonical = engine_fs::virtual_fs::canonicalize(root)
        .with_context(|| format!("Workspace root missing: {}", root.display()))?;

    if !canonical.starts_with(&root_canonical) {
        return Err(anyhow!("Path escapes workspace root"));
    }
    Ok(canonical)
}

pub(crate) fn resolve_workspace_path_soft(
    root: &Path,
    rel_or_abs: &str,
) -> anyhow::Result<PathBuf> {
    let p = PathBuf::from(rel_or_abs);
    let joined = if p.is_absolute() { p } else { root.join(&p) };

    if let Ok(canonical) = engine_fs::virtual_fs::canonicalize(&joined) {
        if let Ok(root_canonical) = engine_fs::virtual_fs::canonicalize(root) {
            if !canonical.starts_with(&root_canonical) {
                return Err(anyhow!("Path escapes workspace root"));
            }
        }
        return Ok(canonical);
    }

    let mut components = Vec::new();
    for part in joined.components() {
        use std::path::Component;
        match part {
            Component::ParentDir => {
                components.pop();
            }
            Component::CurDir => {}
            other => components.push(other),
        }
    }
    let normalized: PathBuf = components.iter().collect();

    let root_normalized: PathBuf = {
        let mut c = Vec::new();
        for part in root.components() {
            use std::path::Component;
            match part {
                Component::ParentDir => {
                    c.pop();
                }
                Component::CurDir => {}
                other => c.push(other),
            }
        }
        c.iter().collect()
    };

    if root_normalized.is_absolute() && !normalized.starts_with(&root_normalized) {
        return Err(anyhow!("Path escapes workspace root"));
    }
    Ok(normalized)
}
