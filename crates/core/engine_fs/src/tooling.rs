use anyhow::{anyhow, Context, Result};
use parking_lot::RwLock;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tool_registry::{PluginToolRegistry, ToolContext, ToolRegistry};
use tool_registry_macros::tool;

use crate::virtual_fs;

const TOOLING_STATE_KEY: &str = "engine_fs_tooling_state";
const DEFAULT_MAX_ENTRIES: usize = 200;
const MAX_ALLOWED_ENTRIES: usize = 1000;
const DEFAULT_TREE_DEPTH: usize = 3;
const MAX_ALLOWED_DEPTH: usize = 8;

static GLOBAL_STATE: OnceLock<Arc<RwLock<ToolingState>>> = OnceLock::new();

#[derive(Clone)]
pub struct ToolingState {
    workspace_root: PathBuf,
    cwd: PathBuf,
}

impl ToolingState {
    pub fn new(workspace_root: PathBuf) -> Self {
        Self {
            cwd: workspace_root.clone(),
            workspace_root,
        }
    }
}

#[derive(Clone)]
struct FilterOptions {
    search: Option<String>,
    include_hidden: bool,
    files_only: bool,
    directories_only: bool,
}

#[derive(Clone)]
struct ListedEntry {
    name: String,
    path: String,
    is_dir: bool,
    size: u64,
    modified: Option<u64>,
    depth: usize,
}

pub fn register_tools(registry: &mut ToolRegistry) {
    registry.merge_plugin(&PluginToolRegistry::from_namespace(module_path!()));
}

pub fn new_tooling_state(workspace_root: PathBuf) -> ToolingState {
    ToolingState::new(workspace_root)
}

pub fn insert_tooling_state(ctx: &mut ToolContext, workspace_root: PathBuf) {
    let state = GLOBAL_STATE
        .get_or_init(|| Arc::new(RwLock::new(ToolingState::new(workspace_root.clone()))));
    {
        let mut guard = state.write();
        if guard.workspace_root != workspace_root {
            guard.workspace_root = workspace_root.clone();
            guard.cwd = workspace_root.clone();
        }
    }
    ctx.insert_extra(TOOLING_STATE_KEY, ToolingState::new(workspace_root));
}

pub fn current_working_dir(ctx: &ToolContext) -> Option<PathBuf> {
    let _ = ctx;
    GLOBAL_STATE.get().map(|state| state.read().cwd.clone())
}

fn workspace_root() -> Result<PathBuf> {
    GLOBAL_STATE
        .get()
        .map(|state| state.read().workspace_root.clone())
        .ok_or_else(|| anyhow!("Workspace root unavailable in engine_fs tooling state"))
}

fn current_dir() -> Result<PathBuf> {
    GLOBAL_STATE
        .get()
        .map(|state| state.read().cwd.clone())
        .ok_or_else(|| anyhow!("Current working directory unavailable in engine_fs tooling state"))
}

fn set_current_dir(path: PathBuf) -> Result<()> {
    let state = GLOBAL_STATE
        .get()
        .ok_or_else(|| anyhow!("Filesystem tooling state unavailable"))?;
    state.write().cwd = path;
    Ok(())
}

fn normalize_local_path(path: &Path) -> PathBuf {
    let mut parts = Vec::new();
    let mut prefix = None;
    let mut has_root = false;

    for component in path.components() {
        match component {
            Component::Prefix(value) => prefix = Some(value.as_os_str().to_owned()),
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = parts.last() {
                    if last != ".." {
                        parts.pop();
                        continue;
                    }
                }
                if !has_root {
                    parts.push("..".into());
                }
            }
            Component::Normal(value) => parts.push(value.to_owned()),
        }
    }

    let mut normalized = if let Some(prefix) = prefix {
        PathBuf::from(prefix)
    } else {
        PathBuf::new()
    };

    if has_root {
        normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR));
    }

    for part in parts {
        normalized.push(part);
    }
    normalized
}

fn normalize_cloud_path(raw: &str) -> String {
    let raw = raw.replace('\\', "/");
    let (scheme, rest) = raw
        .split_once("://")
        .map(|(scheme, rest)| (format!("{}://", scheme), rest.to_string()))
        .unwrap_or_default();

    let mut normalized_parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                normalized_parts.pop();
            }
            value => normalized_parts.push(value),
        }
    }

    if normalized_parts.is_empty() {
        scheme.trim_end_matches('/').to_string()
    } else {
        format!("{}{}", scheme, normalized_parts.join("/"))
    }
}

fn ensure_within_root(path: &Path, root: &Path) -> Result<()> {
    if virtual_fs::is_cloud_path(root) || virtual_fs::is_cloud_path(path) {
        let path = normalize_cloud_path(&virtual_fs::normalize_path(path));
        let root = normalize_cloud_path(&virtual_fs::normalize_path(root));
        if path == root || path.starts_with(&(root.clone() + "/")) {
            return Ok(());
        }
        return Err(anyhow!("Path escapes workspace root"));
    }

    let normalized_path = normalize_local_path(path);
    let normalized_root = normalize_local_path(root);
    if normalized_path.starts_with(&normalized_root) {
        Ok(())
    } else {
        Err(anyhow!("Path escapes workspace root"))
    }
}

fn resolve_path(path: Option<&str>) -> Result<PathBuf> {
    let root = workspace_root()?;
    let cwd = current_dir()?;
    let raw = path.unwrap_or(".");
    let candidate = if raw.is_empty() || raw == "." {
        cwd
    } else {
        let provided = PathBuf::from(raw);
        if provided.is_absolute() || virtual_fs::is_cloud_path(&provided) {
            provided
        } else if virtual_fs::is_cloud_path(&cwd) {
            PathBuf::from(virtual_fs::cloud_join(
                &virtual_fs::normalize_path(&cwd),
                raw,
            ))
        } else {
            cwd.join(provided)
        }
    };

    ensure_within_root(&candidate, &root)?;
    Ok(candidate)
}

fn rel_display(base: &Path, path: &Path) -> String {
    if virtual_fs::is_cloud_path(base) || virtual_fs::is_cloud_path(path) {
        let path_str = normalize_cloud_path(&virtual_fs::normalize_path(path));
        let base_str = normalize_cloud_path(&virtual_fs::normalize_path(base));
        if path_str == base_str {
            ".".to_string()
        } else if let Some(rest) = path_str.strip_prefix(&(base_str + "/")) {
            rest.to_string()
        } else {
            path_str
        }
    } else {
        path.strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

fn entry_matches(name: &str, path: &str, is_dir: bool, options: &FilterOptions) -> bool {
    if !options.include_hidden && name.starts_with('.') {
        return false;
    }
    if options.files_only && is_dir {
        return false;
    }
    if options.directories_only && !is_dir {
        return false;
    }
    if let Some(search) = &options.search {
        let search = search.to_lowercase();
        return name.to_lowercase().contains(&search) || path.to_lowercase().contains(&search);
    }
    true
}

fn collect_entries(
    root: &Path,
    recursive: bool,
    max_depth: usize,
    max_entries: usize,
    filters: &FilterOptions,
) -> Result<(Vec<ListedEntry>, bool)> {
    let mut entries = Vec::new();
    let mut truncated = false;

    if recursive {
        for manifest in virtual_fs::manifest(root)? {
            let depth = manifest
                .path
                .split('/')
                .filter(|part| !part.is_empty())
                .count();
            if depth > max_depth {
                continue;
            }

            let full_path = if virtual_fs::is_cloud_path(root) {
                PathBuf::from(virtual_fs::cloud_join(
                    &virtual_fs::normalize_path(root),
                    &manifest.path,
                ))
            } else {
                root.join(&manifest.path)
            };
            let name = Path::new(&manifest.path)
                .file_name()
                .map(|value| value.to_string_lossy().to_string())
                .unwrap_or_else(|| manifest.path.clone());
            let rel_path = rel_display(root, &full_path);
            if !entry_matches(&name, &rel_path, manifest.is_dir, filters) {
                continue;
            }

            if entries.len() >= max_entries {
                truncated = true;
                break;
            }
            entries.push(ListedEntry {
                name,
                path: rel_path,
                is_dir: manifest.is_dir,
                size: manifest.size,
                modified: manifest.modified,
                depth,
            });
        }
    } else {
        for entry in virtual_fs::list_dir(root)? {
            let full_path = if virtual_fs::is_cloud_path(root) {
                PathBuf::from(virtual_fs::cloud_join(
                    &virtual_fs::normalize_path(root),
                    &entry.name,
                ))
            } else {
                root.join(&entry.name)
            };
            let rel_path = rel_display(root, &full_path);
            if !entry_matches(&entry.name, &rel_path, entry.is_dir, filters) {
                continue;
            }
            if entries.len() >= max_entries {
                truncated = true;
                break;
            }
            entries.push(ListedEntry {
                name: entry.name,
                path: rel_path,
                is_dir: entry.is_dir,
                size: entry.size,
                modified: entry.modified,
                depth: 1,
            });
        }
    }

    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((entries, truncated))
}

fn parse_max_entries(value: Option<i64>) -> usize {
    value
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_MAX_ENTRIES)
        .clamp(1, MAX_ALLOWED_ENTRIES)
}

fn parse_max_depth(value: Option<i64>) -> usize {
    value
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_TREE_DEPTH)
        .clamp(1, MAX_ALLOWED_DEPTH)
}

#[tool(category = "filesystem")]
/// Change the AI filesystem working directory used by list_files, tree, and other relative-path tools.
pub fn cd(path: String) -> anyhow::Result<Value> {
    let target = resolve_path(Some(&path))?;
    let metadata = virtual_fs::metadata(&target)
        .with_context(|| format!("Path does not exist: {}", target.display()))?;
    if !metadata.is_dir {
        return Err(anyhow!("cd target is not a directory"));
    }
    set_current_dir(target.clone())?;

    let workspace_root = workspace_root()?;
    Ok(json!({
        "ok": true,
        "cwd": target.display().to_string(),
        "workspace_root": workspace_root.display().to_string(),
    }))
}

#[tool(category = "filesystem")]
/// List files and directories from the current AI working directory or a specified path.
/// Supports optional recursion and case-insensitive search filtering.
pub fn list_files(
    path: Option<String>,
    recursive: Option<bool>,
    search: Option<String>,
    include_hidden: Option<bool>,
    files_only: Option<bool>,
    directories_only: Option<bool>,
    max_entries: Option<i64>,
) -> anyhow::Result<Value> {
    let base = resolve_path(path.as_deref())?;
    let metadata = virtual_fs::metadata(&base)
        .with_context(|| format!("Path does not exist: {}", base.display()))?;
    if !metadata.is_dir {
        return Err(anyhow!("list_files target is not a directory"));
    }

    let recursive = recursive.unwrap_or(false);
    let max_entries = parse_max_entries(max_entries);
    let filters = FilterOptions {
        search,
        include_hidden: include_hidden.unwrap_or(false),
        files_only: files_only.unwrap_or(false),
        directories_only: directories_only.unwrap_or(false),
    };
    let max_depth = if recursive { MAX_ALLOWED_DEPTH } else { 1 };
    let (entries, truncated) = collect_entries(&base, recursive, max_depth, max_entries, &filters)?;

    Ok(json!({
        "ok": true,
        "cwd": current_dir()?.display().to_string(),
        "path": base.display().to_string(),
        "recursive": recursive,
        "count": entries.len(),
        "truncated": truncated,
        "entries": entries.into_iter().map(|entry| json!({
            "name": entry.name,
            "path": entry.path,
            "is_dir": entry.is_dir,
            "size": entry.size,
            "modified": entry.modified,
        })).collect::<Vec<_>>(),
    }))
}

#[tool(category = "filesystem")]
/// Return a bounded directory tree from the current AI working directory or a specified path.
/// Includes max depth, entry limit, and search filtering controls.
pub fn tree(
    path: Option<String>,
    max_depth: Option<i64>,
    max_entries: Option<i64>,
    search: Option<String>,
    include_hidden: Option<bool>,
    files_only: Option<bool>,
    directories_only: Option<bool>,
) -> anyhow::Result<Value> {
    let base = resolve_path(path.as_deref())?;
    let metadata = virtual_fs::metadata(&base)
        .with_context(|| format!("Path does not exist: {}", base.display()))?;
    if !metadata.is_dir {
        return Err(anyhow!("tree target is not a directory"));
    }

    let max_depth = parse_max_depth(max_depth);
    let max_entries = parse_max_entries(max_entries);
    let filters = FilterOptions {
        search,
        include_hidden: include_hidden.unwrap_or(false),
        files_only: files_only.unwrap_or(false),
        directories_only: directories_only.unwrap_or(false),
    };
    let (entries, truncated) = collect_entries(&base, true, max_depth, max_entries, &filters)?;

    let lines = entries
        .iter()
        .map(|entry| {
            let indent = "  ".repeat(entry.depth.saturating_sub(1));
            format!(
                "{}{}{}",
                indent,
                if entry.is_dir { "- " } else { "* " },
                entry.path
            )
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": true,
        "cwd": current_dir()?.display().to_string(),
        "path": base.display().to_string(),
        "max_depth": max_depth,
        "count": entries.len(),
        "truncated": truncated,
        "tree": lines.join("\n"),
        "entries": entries.into_iter().map(|entry| json!({
            "name": entry.name,
            "path": entry.path,
            "is_dir": entry.is_dir,
            "depth": entry.depth,
            "size": entry.size,
            "modified": entry.modified,
        })).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_cloud_segments() {
        assert_eq!(
            normalize_cloud_path("cloud+pulsar://host/project/dir/../file.txt"),
            "cloud+pulsar://host/project/file.txt"
        );
    }

    #[test]
    fn local_normalization_removes_dot_segments() {
        let normalized = normalize_local_path(Path::new("/tmp/project/a/../b/./file.txt"));
        assert!(normalized.ends_with("tmp/project/b/file.txt"));
    }
}
