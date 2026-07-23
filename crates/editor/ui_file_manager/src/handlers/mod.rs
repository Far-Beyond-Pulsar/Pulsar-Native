use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;

use crate::components::FileManagerDrawer;
use crate::utils::{actions::*, tree::FolderNode, types::*};

pub fn handle_folder_select(
    d: &mut FileManagerDrawer,
    path: PathBuf,
    cx: &mut Context<FileManagerDrawer>,
) {
    d.selected_folder = Some(path.clone());
    d.selected_items.clear();
    d.selection_anchor = None;
    if let Some(ref mut t) = d.folder_tree {
        t.toggle_expanded(&path);
    }
    cx.notify();
}

pub fn handle_item_click(
    d: &mut FileManagerDrawer,
    item: &FileItem,
    mods: &Modifiers,
    cx: &mut Context<FileManagerDrawer>,
) {
    let additive = mods.control || mods.platform;

    if mods.shift {
        // Shift-click: extend the selection to a contiguous range between the
        // anchor and the clicked item, following the on-screen (filtered/sorted)
        // order. Without Ctrl held this replaces the selection; with Ctrl it
        // adds the range. The anchor itself is left unchanged.
        let items = d.get_filtered_items();
        let anchor = d.selection_anchor.clone().unwrap_or_else(|| item.path.clone());
        let anchor_idx = items.iter().position(|i| i.path == anchor);
        let click_idx = items.iter().position(|i| i.path == item.path);

        if let (Some(a), Some(c)) = (anchor_idx, click_idx) {
            if !additive {
                d.selected_items.clear();
            }
            let (lo, hi) = if a <= c { (a, c) } else { (c, a) };
            for it in &items[lo..=hi] {
                d.selected_items.insert(it.path.clone());
            }
        } else {
            // Anchor no longer visible (e.g. filtered out): fall back to single-select.
            d.selected_items.clear();
            d.selected_items.insert(item.path.clone());
            d.selection_anchor = Some(item.path.clone());
        }
    } else if additive {
        // Ctrl/Cmd-click: toggle this item and make it the new anchor.
        if d.selected_items.contains(&item.path) {
            d.selected_items.remove(&item.path);
        } else {
            d.selected_items.insert(item.path.clone());
        }
        d.selection_anchor = Some(item.path.clone());
    } else {
        // Plain click: single-select and set the anchor.
        d.selected_items.clear();
        d.selected_items.insert(item.path.clone());
        d.selection_anchor = Some(item.path.clone());
    }
    cx.notify();
}

/// Select every item currently visible in the active folder (Ctrl/Cmd+A).
pub fn handle_select_all(d: &mut FileManagerDrawer, cx: &mut Context<FileManagerDrawer>) {
    let items = d.get_filtered_items();
    d.selection_anchor = items.first().map(|i| i.path.clone());
    d.selected_items = items.into_iter().map(|i| i.path).collect();
    cx.notify();
}

pub fn handle_item_double_click(
    d: &mut FileManagerDrawer,
    item: &FileItem,
    cx: &mut Context<FileManagerDrawer>,
) {
    if item.is_folder {
        d.selected_folder = Some(item.path.clone());
    } else {
        cx.emit(FileSelected {
            path: item.path.clone(),
            file_type_def: item.file_type_def.clone(),
        });
    }
    cx.notify();
}

pub fn handle_create_asset(
    d: &mut FileManagerDrawer,
    action: &CreateAsset,
    cx: &mut Context<FileManagerDrawer>,
) {
    let Some(folder) = &d.selected_folder else {
        return;
    };
    let ft = d
        .registered_file_types
        .iter()
        .find(|x| x.id.as_str() == action.file_type_id)
        .cloned();
    let display = ft
        .as_ref()
        .map(|x| x.display_name.as_str())
        .unwrap_or(action.display_name.as_str());
    let ext = ft
        .as_ref()
        .map(|x| x.extension.as_str())
        .unwrap_or(action.extension.as_str());
    let mut fp =
        crate::utils::cloud_join(folder, &format!("New{}.{}", display.replace(" ", ""), ext));
    let mut c = 1;
    while (engine_fs::virtual_fs::is_remote()
        && engine_fs::virtual_fs::exists(&fp).unwrap_or(false))
        || (!engine_fs::virtual_fs::is_remote() && fp.exists())
    {
        fp = crate::utils::cloud_join(
            folder,
            &format!("New{}_{}.{}", display.replace(" ", ""), c, ext),
        );
        c += 1;
    }
    let write = |p: &std::path::Path, data: &[u8]| -> Result<(), Box<dyn std::error::Error>> {
        if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(p) {
            Ok(engine_fs::virtual_fs::write_file(p, data)?)
        } else {
            Ok(std::fs::write(p, data)?)
        }
    };
    let mkdir = |p: &std::path::Path| -> Result<(), Box<dyn std::error::Error>> {
        if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(p) {
            Ok(engine_fs::virtual_fs::create_dir_all(p)?)
        } else {
            Ok(std::fs::create_dir_all(p)?)
        }
    };
    let r = if let Some(def) = ft {
        match def.structure {
            plugin_editor_api::FileStructure::Standalone => {
                let content = if def.default_content.is_null() {
                    vec![]
                } else {
                    serde_json::to_string_pretty(&def.default_content)
                        .unwrap_or_default()
                        .into_bytes()
                };
                write(&fp, &content)
            }
            plugin_editor_api::FileStructure::FolderBased {
                marker_file,
                template_structure,
            } => mkdir(&fp).and_then(|_| {
                let mp = fp.join(&marker_file);
                let mc = if def.default_content.is_null() {
                    vec![]
                } else {
                    serde_json::to_string_pretty(&def.default_content)
                        .unwrap_or_default()
                        .into_bytes()
                };
                write(&mp, &mc).and_then(|_| {
                    for t in &template_structure {
                        match t {
                            plugin_editor_api::PathTemplate::File { path, content } => {
                                let tp = fp.join(path);
                                if let Some(pa) = tp.parent() {
                                    mkdir(pa)?;
                                }
                                write(&tp, content.as_bytes())?;
                            }
                            plugin_editor_api::PathTemplate::Folder { path } => {
                                mkdir(&fp.join(path))?;
                            }
                        }
                    }
                    Ok(())
                })
            }),
        }
    } else {
        let content = if action.default_content.is_null() {
            vec![]
        } else {
            action.default_content.to_string().into_bytes()
        };
        write(&fp, &content)
    };
    if let Err(e) = r {
        tracing::error!("create_asset: {}", e);
    } else {
        if let Some(ref p) = d.project_path {
            d.folder_tree = FolderNode::from_path(p);
        }
        d.mark_directory_cache_dirty();
        cx.notify();
    }
}

pub fn handle_new_folder(
    d: &mut FileManagerDrawer,
    action: &NewFolder,
    cx: &mut Context<FileManagerDrawer>,
) {
    let base = if !action.folder_path.is_empty() {
        Some(PathBuf::from(&action.folder_path))
    } else {
        d.selected_folder.clone()
    };
    let Some(folder) = base else {
        return;
    };
    let mut c = 1;
    let mut name = "NewFolder".to_string();
    let mut fp = crate::utils::cloud_join(&folder, &name);
    while (engine_fs::virtual_fs::is_remote()
        && engine_fs::virtual_fs::exists(&fp).unwrap_or(false))
        || (!engine_fs::virtual_fs::is_remote() && fp.exists())
    {
        name = format!("NewFolder_{}", c);
        fp = crate::utils::cloud_join(&folder, &name);
        c += 1;
    }
    d.selected_folder = Some(folder);
    if let Err(e) = if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(&fp) {
        engine_fs::virtual_fs::create_dir_all(&fp)
    } else {
        std::fs::create_dir(&fp).map_err(Into::into)
    } {
        tracing::error!("new_folder: {}", e);
    } else {
        d.renaming_item = Some(fp);
        d.mark_directory_cache_dirty();
        cx.notify();
    }
}

pub fn handle_delete_item(d: &mut FileManagerDrawer, cx: &mut Context<FileManagerDrawer>) {
    for item in d.selected_items.iter().cloned().collect::<Vec<PathBuf>>() {
        if let Err(e) = if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(&item) {
            engine_fs::virtual_fs::delete_path(&item)
        } else if item.is_dir() {
            std::fs::remove_dir_all(&item).map_err(Into::into)
        } else {
            std::fs::remove_file(&item).map_err(Into::into)
        } {
            tracing::error!("delete: {}", e);
        }
    }
    d.selected_items.clear();
    if let Some(ref p) = d.project_path {
        d.folder_tree = FolderNode::from_path(p);
    }
    d.mark_directory_cache_dirty();
    cx.notify();
}

pub fn handle_rename_item(
    d: &mut FileManagerDrawer,
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) {
    if let Some(item) = d.selected_items.iter().next().cloned() {
        crate::utils::start_rename(d, item, w, cx);
    }
}

pub fn handle_duplicate_item(d: &mut FileManagerDrawer, cx: &mut Context<FileManagerDrawer>) {
    for item in d.selected_items.iter().cloned().collect::<Vec<PathBuf>>() {
        if let (Some(parent), Some(name)) = (item.parent(), item.file_name()) {
            let ns = name.to_string_lossy();
            let mut c = 1;
            let mut nn = format!("{}_copy", ns);
            let mut np = crate::utils::cloud_join(parent, &nn);
            while (engine_fs::virtual_fs::is_remote()
                && engine_fs::virtual_fs::exists(&np).unwrap_or(false))
                || (!engine_fs::virtual_fs::is_remote() && np.exists())
            {
                nn = format!("{}_copy_{}", ns, c);
                np = crate::utils::cloud_join(parent, &nn);
                c += 1;
            }
            if let Err(e) = if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(&item)
            {
                engine_fs::virtual_fs::read_file(&item)
                    .and_then(|d| engine_fs::virtual_fs::write_file(&np, &d))
            } else if item.is_dir() {
                FileManagerDrawer::copy_dir_recursive(&item, &np).map_err(Into::into)
            } else {
                std::fs::copy(&item, &np).map(|_| ()).map_err(Into::into)
            } {
                tracing::error!("duplicate: {}", e);
            }
        }
    }
    if let Some(ref p) = d.project_path {
        d.folder_tree = FolderNode::from_path(p);
    }
    d.mark_directory_cache_dirty();
    cx.notify();
}

pub fn handle_copy(d: &mut FileManagerDrawer, _cx: &mut Context<FileManagerDrawer>) {
    d.clipboard = Some((d.selected_items.iter().cloned().collect(), false));
}

pub fn handle_cut(d: &mut FileManagerDrawer, _cx: &mut Context<FileManagerDrawer>) {
    d.clipboard = Some((d.selected_items.iter().cloned().collect(), true));
}

pub fn handle_paste(d: &mut FileManagerDrawer, cx: &mut Context<FileManagerDrawer>) {
    let Some((items, is_cut)) = d.clipboard.clone() else {
        return;
    };
    let Some(ref target) = d.selected_folder else {
        return;
    };
    for item in &items {
        if let Some(name) = item.file_name() {
            let tp = crate::utils::cloud_join(target, &name.to_string_lossy());
            if let Err(e) = if is_cut {
                if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(item) {
                    engine_fs::virtual_fs::rename(item, &tp)
                } else {
                    std::fs::rename(item, &tp).map_err(Into::into)
                }
            } else {
                if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(item) {
                    engine_fs::virtual_fs::read_file(item)
                        .and_then(|d| engine_fs::virtual_fs::write_file(&tp, &d))
                } else if item.is_dir() {
                    FileManagerDrawer::copy_dir_recursive(item, &tp).map_err(Into::into)
                } else {
                    std::fs::copy(item, &tp).map(|_| ()).map_err(Into::into)
                }
            } {
                tracing::error!("paste: {}", e);
            }
        }
    }
    if is_cut {
        d.clipboard = None;
    }
    if let Some(ref p) = d.project_path {
        d.folder_tree = FolderNode::from_path(p);
    }
    d.mark_directory_cache_dirty();
    cx.notify();
}

pub fn handle_open_in_file_manager(
    d: &mut FileManagerDrawer,
    _a: &OpenInFileManager,
    _cx: &mut Context<FileManagerDrawer>,
) {
    let p = d
        .selected_folder
        .clone()
        .or_else(|| d.selected_items.iter().next().cloned());
    if let Some(path) = p {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer")
                .arg(path.to_string_lossy().to_string())
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(path.to_string_lossy().to_string())
                .spawn();
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(path.to_string_lossy().to_string())
                .spawn();
        }
    }
}

pub fn handle_open_terminal_here(
    d: &mut FileManagerDrawer,
    _a: &OpenTerminalHere,
    _cx: &mut Context<FileManagerDrawer>,
) {
    let f = d.selected_folder.clone().or_else(|| {
        d.selected_items.iter().next().and_then(|p| {
            if p.is_dir() {
                Some(p.clone())
            } else {
                p.parent().map(|x| x.to_path_buf())
            }
        })
    });
    if let Some(folder) = f {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("cmd")
                .args(["/c", "start", "cmd"])
                .current_dir(folder)
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .args(["-a", "Terminal"])
                .arg(folder.to_string_lossy().to_string())
                .spawn();
        }
        #[cfg(target_os = "linux")]
        {
            for t in &["gnome-terminal", "konsole", "xterm"] {
                if std::process::Command::new(t)
                    .current_dir(&folder)
                    .spawn()
                    .is_ok()
                {
                    break;
                }
            }
        }
    }
}

pub fn handle_validate_asset(
    _: &mut FileManagerDrawer,
    _: &ValidateAsset,
    _: &mut Context<FileManagerDrawer>,
) {
    tracing::info!("validate not impl");
}
pub fn handle_toggle_favorite(
    _: &mut FileManagerDrawer,
    _: &ToggleFavorite,
    _: &mut Context<FileManagerDrawer>,
) {
    tracing::info!("favorite not impl");
}
pub fn handle_toggle_hidden(
    _: &mut FileManagerDrawer,
    _: &ToggleHidden,
    _: &mut Context<FileManagerDrawer>,
) {
    tracing::info!("hidden not impl");
}
pub fn handle_show_history(
    _: &mut FileManagerDrawer,
    _: &ShowHistory,
    _: &mut Context<FileManagerDrawer>,
) {
    tracing::info!("history not impl");
}
pub fn handle_check_multiuser_sync(
    _: &mut FileManagerDrawer,
    _: &CheckMultiuserSync,
    _: &mut Context<FileManagerDrawer>,
) {
    tracing::info!("multiuser not impl");
}

pub fn handle_toggle_gitignore(
    d: &mut FileManagerDrawer,
    _: &ToggleGitignore,
    cx: &mut Context<FileManagerDrawer>,
) {
    let Some(item) = d.selected_items.iter().next() else {
        return;
    };
    let Some(proj) = &d.project_path else {
        return;
    };
    let gp = proj.join(".gitignore");
    let content = std::fs::read_to_string(&gp).unwrap_or_default();
    if let Ok(rel) = item.strip_prefix(proj) {
        let p = rel.to_string_lossy().replace('\\', "/");
        if content.lines().any(|l| l.trim() == p) {
            let _ = std::fs::write(
                &gp,
                content
                    .lines()
                    .filter(|l| l.trim() != p)
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        } else {
            let _ = std::fs::write(
                &gp,
                if content.is_empty() {
                    p
                } else {
                    format!("{}\n{}", content.trim_end(), p)
                },
            );
        }
        cx.notify();
    }
}

pub fn handle_set_color_override(
    d: &mut FileManagerDrawer,
    action: &SetColorOverride,
    cx: &mut Context<FileManagerDrawer>,
) {
    let p = if action.item_path.is_empty() {
        d.selected_items.iter().next().cloned()
    } else {
        Some(PathBuf::from(&action.item_path))
    };
    if let Some(path) = p {
        let color = action.color.as_ref().map(|c| {
            let hex = ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32);
            gpui::rgb(hex).into()
        });
        if let Err(e) = d.fs_metadata.set_color_override(&path, color) {
            tracing::error!("set_color: {}", e);
        } else {
            cx.notify();
        }
    }
}

impl FileManagerDrawer {
    pub fn set_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.project_path = Some(path.clone());
        self.folder_tree = FolderNode::from_path(&path);
        self.selected_folder = Some(path.clone());
        self.fs_metadata.load_from_project_root(&path);
        self.mark_directory_cache_dirty();
        cx.notify();
    }
}
