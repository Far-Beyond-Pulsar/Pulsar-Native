use plugin_ai_macro::ai_tool;
use plugin_ai_tools::ToolRegistry;
use plugin_editor_api::{AiToolDefinition, PluginError};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::drawer::FileOperations;

fn workspace_root() -> Result<PathBuf, PluginError> {
    engine_state::get_project_path()
        .map(PathBuf::from)
        .ok_or_else(|| PluginError::Other {
            message: "No active project root".to_string(),
        })
}

fn resolve_workspace_path(path: &str, must_exist: bool) -> Result<PathBuf, PluginError> {
    let root = workspace_root()?;
    let joined = {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            root.join(p)
        }
    };

    let root_canonical = root.canonicalize().map_err(|err| PluginError::Other {
        message: format!("Failed to resolve project root: {err}"),
    })?;

    if must_exist {
        let canonical = joined.canonicalize().map_err(|err| PluginError::Other {
            message: format!("Path does not exist: {} ({err})", joined.display()),
        })?;
        if !canonical.starts_with(&root_canonical) {
            return Err(PluginError::Other {
                message: "Path escapes project root".to_string(),
            });
        }
        Ok(canonical)
    } else {
        let parent = joined
            .parent()
            .ok_or_else(|| PluginError::Other {
                message: format!("Invalid path: {}", joined.display()),
            })?
            .to_path_buf();
        let parent_canonical = parent.canonicalize().map_err(|err| PluginError::Other {
            message: format!("Parent path does not exist: {} ({err})", parent.display()),
        })?;
        if !parent_canonical.starts_with(&root_canonical) {
            return Err(PluginError::Other {
                message: "Path escapes project root".to_string(),
            });
        }
        Ok(joined)
    }
}

fn metadata_value(path: &Path) -> Value {
    match fs::metadata(path) {
        Ok(meta) => json!({
            "is_file": meta.is_file(),
            "is_dir": meta.is_dir(),
            "size_bytes": meta.len(),
            "readonly": meta.permissions().readonly(),
            "modified_unix_ms": meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis()),
        }),
        Err(_) => json!({}),
    }
}

fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

fn walk_tree(
    path: &Path,
    depth: usize,
    max_depth: usize,
    max_entries: usize,
    include_hidden: bool,
    emitted: &mut usize,
) -> Value {
    let mut children = Vec::new();
    if depth < max_depth {
        if let Ok(entries) = fs::read_dir(path) {
            let mut entries = entries.flatten().collect::<Vec<_>>();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                if *emitted >= max_entries {
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                if !include_hidden && is_hidden_name(&name) {
                    continue;
                }
                *emitted += 1;
                children.push(walk_tree(
                    &entry.path(),
                    depth + 1,
                    max_depth,
                    max_entries,
                    include_hidden,
                    emitted,
                ));
            }
        }
    }

    let display_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    json!({
        "name": display_name,
        "path": path.display().to_string(),
        "depth": depth,
        "children": children,
        "metadata": metadata_value(path),
    })
}

#[ai_tool(category = "file-manager")]
pub fn fm_list_dir(path: String, include_hidden: Option<bool>) -> Result<Value, PluginError> {
    let path = resolve_workspace_path(&path, true)?;
    if !path.is_dir() {
        return Err(PluginError::Other {
            message: format!("Not a directory: {}", path.display()),
        });
    }
    let include_hidden = include_hidden.unwrap_or(false);
    let mut entries = Vec::new();
    for entry in fs::read_dir(&path).map_err(|err| PluginError::Other {
        message: format!("Failed to list {}: {err}", path.display()),
    })? {
        let entry = entry.map_err(|err| PluginError::Other {
            message: format!("Failed reading dir entry: {err}"),
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        if !include_hidden && is_hidden_name(&name) {
            continue;
        }
        let p = entry.path();
        entries.push(json!({
            "name": name,
            "path": p.display().to_string(),
            "metadata": metadata_value(&p),
        }));
    }
    Ok(json!({
        "path": path.display().to_string(),
        "entries": entries,
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_tree_path(
    path: String,
    max_depth: Option<u32>,
    max_entries: Option<u32>,
    include_hidden: Option<bool>,
) -> Result<Value, PluginError> {
    let path = resolve_workspace_path(&path, true)?;
    let max_depth = max_depth.unwrap_or(4) as usize;
    let max_entries = max_entries.unwrap_or(1500) as usize;
    let include_hidden = include_hidden.unwrap_or(false);

    let mut emitted = 0usize;
    let tree = walk_tree(
        &path,
        0,
        max_depth,
        max_entries,
        include_hidden,
        &mut emitted,
    );

    Ok(json!({
        "path": path.display().to_string(),
        "max_depth": max_depth,
        "max_entries": max_entries,
        "emitted_entries": emitted,
        "truncated": emitted >= max_entries,
        "tree": tree,
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_path_info(path: String) -> Result<Value, PluginError> {
    let path = resolve_workspace_path(&path, true)?;
    Ok(json!({
        "path": path.display().to_string(),
        "metadata": metadata_value(&path),
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_list_files(
    path: String,
    recursive: Option<bool>,
    max_depth: Option<u32>,
    max_results: Option<u32>,
    include_hidden: Option<bool>,
    extensions: Option<Vec<String>>,
) -> Result<Value, PluginError> {
    let path = resolve_workspace_path(&path, true)?;
    let recursive = recursive.unwrap_or(true);
    let max_depth = max_depth.unwrap_or(if recursive { 64 } else { 1 }) as usize;
    let max_results = max_results.unwrap_or(2000) as usize;
    let include_hidden = include_hidden.unwrap_or(false);
    let normalized_exts = extensions
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .collect::<Vec<_>>();

    let mut stack = vec![(path.clone(), 0usize)];
    let mut files = Vec::new();
    while let Some((dir, depth)) = stack.pop() {
        if files.len() >= max_results || depth > max_depth {
            break;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if files.len() >= max_results {
                break;
            }
            let p = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if !include_hidden && is_hidden_name(&name) {
                continue;
            }
            if p.is_dir() {
                if recursive && depth < max_depth {
                    stack.push((p, depth + 1));
                }
                continue;
            }
            if !normalized_exts.is_empty() {
                let ext = p
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_default();
                if !normalized_exts.iter().any(|allowed| allowed == &ext) {
                    continue;
                }
            }
            files.push(json!({
                "name": name,
                "path": p.display().to_string(),
                "metadata": metadata_value(&p),
            }));
        }
    }

    Ok(json!({
        "path": path.display().to_string(),
        "count": files.len(),
        "truncated": files.len() >= max_results,
        "files": files,
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_new_folder(path: String, name: Option<String>) -> Result<Value, PluginError> {
    let base = resolve_workspace_path(&path, true)?;
    if !base.is_dir() {
        return Err(PluginError::Other {
            message: format!("Base path is not a directory: {}", base.display()),
        });
    }

    let created = if let Some(name) = name {
        let target = resolve_workspace_path(&format!("{}/{}", base.display(), name), false)?;
        fs::create_dir_all(&target).map_err(|err| PluginError::Other {
            message: format!("Failed to create directory {}: {err}", target.display()),
        })?;
        target
    } else {
        FileOperations::new_folder(&base).map_err(|err| PluginError::Other {
            message: format!("Failed to create folder: {err}"),
        })?
    };

    Ok(json!({
        "ok": true,
        "created_path": created.display().to_string(),
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_rename_item(path: String, new_name: String) -> Result<Value, PluginError> {
    let source = resolve_workspace_path(&path, true)?;
    let ops = FileOperations::new(Some(workspace_root()?));
    let renamed = ops
        .rename_item(&source, &new_name)
        .map_err(|err| PluginError::Other {
            message: format!("Rename failed: {err}"),
        })?;
    Ok(json!({
        "ok": true,
        "old_path": source.display().to_string(),
        "new_path": renamed.display().to_string(),
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_delete_item(path: String) -> Result<Value, PluginError> {
    let target = resolve_workspace_path(&path, true)?;
    let ops = FileOperations::new(Some(workspace_root()?));
    ops.delete_item(&target).map_err(|err| PluginError::Other {
        message: format!("Delete failed: {err}"),
    })?;
    Ok(json!({
        "ok": true,
        "deleted_path": target.display().to_string(),
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_duplicate_item(path: String) -> Result<Value, PluginError> {
    let source = resolve_workspace_path(&path, true)?;
    let duplicated = FileOperations::duplicate_item(&source).map_err(|err| PluginError::Other {
        message: format!("Duplicate failed: {err}"),
    })?;
    Ok(json!({
        "ok": true,
        "source_path": source.display().to_string(),
        "duplicated_path": duplicated.display().to_string(),
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_copy_items(
    source_paths: Vec<String>,
    target_folder: String,
) -> Result<Value, PluginError> {
    let sources = source_paths
        .iter()
        .map(|path| resolve_workspace_path(path, true))
        .collect::<Result<Vec<_>, _>>()?;
    let target = resolve_workspace_path(&target_folder, true)?;
    FileOperations::copy_items(&sources, &target).map_err(|err| PluginError::Other {
        message: format!("Copy failed: {err}"),
    })?;
    Ok(json!({
        "ok": true,
        "source_count": sources.len(),
        "target_folder": target.display().to_string(),
    }))
}

#[ai_tool(category = "file-manager")]
pub fn fm_move_items(
    source_paths: Vec<String>,
    target_folder: String,
) -> Result<Value, PluginError> {
    let sources = source_paths
        .iter()
        .map(|path| resolve_workspace_path(path, true))
        .collect::<Result<Vec<_>, _>>()?;
    let target = resolve_workspace_path(&target_folder, true)?;
    let ops = FileOperations::new(Some(workspace_root()?));
    ops.move_items(&sources, &target)
        .map_err(|err| PluginError::Other {
            message: format!("Move failed: {err}"),
        })?;
    Ok(json!({
        "ok": true,
        "source_count": sources.len(),
        "target_folder": target.display().to_string(),
    }))
}

fn build_registry() -> ToolRegistry {
    plugin_ai_tools::registry_from_inventory(module_path!())
}

fn registry() -> &'static ToolRegistry {
    static REGISTRY: OnceLock<ToolRegistry> = OnceLock::new();
    REGISTRY.get_or_init(build_registry)
}

pub fn ai_tools() -> Vec<AiToolDefinition> {
    registry().definitions()
}

pub fn capabilities_for_file(_file_path: &Path) -> Vec<String> {
    registry()
        .tool_names()
        .into_iter()
        .map(|name| name.to_string())
        .collect()
}

pub fn execute_ai_tool(
    file_path: &Path,
    tool_name: &str,
    mut tool_args: Value,
) -> Result<Value, PluginError> {
    if let Some(obj) = tool_args.as_object_mut() {
        obj.entry("path".to_string())
            .or_insert_with(|| Value::String(file_path.display().to_string()));
    }
    registry().execute(tool_name, tool_args)
}
