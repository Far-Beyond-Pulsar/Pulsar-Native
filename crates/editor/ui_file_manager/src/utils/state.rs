use std::collections::HashSet;
use std::path::Path;

use crate::components::FileManagerDrawer;
use crate::utils::cloud_join;
use crate::utils::git_integration::DeletedFileEntry;
use crate::utils::types::*;
use plugin_editor_api::FileStructure;

impl FileManagerDrawer {
    pub fn start_new_file(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let Some(ref f) = self.selected_folder else {
            return;
        };
        let remote = engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(f);
        let mut name = "untitled.txt".to_string();
        let mut suffix = 1;
        let mut np = cloud_join(f, &name);
        while (remote && engine_fs::virtual_fs::exists(&np).unwrap_or(false))
            || (!remote && np.exists())
        {
            name = format!("untitled ({suffix}).txt");
            suffix += 1;
            np = cloud_join(f, &name);
        }
        let r = if remote {
            engine_fs::virtual_fs::write_file(&np, b"")
        } else {
            std::fs::write(&np, "").map_err(Into::into)
        };
        if let Err(e) = r {
            tracing::error!("Failed to create file: {}", e);
            return;
        }
        crate::utils::start_rename(self, np, window, cx);
        self.mark_directory_cache_dirty();
    }

    pub fn start_new_folder(&mut self, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        let Some(ref f) = self.selected_folder else {
            return;
        };
        let remote = engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(f);
        let mut name = "New Folder".to_string();
        let mut suffix = 1;
        let mut np = cloud_join(f, &name);
        while (remote && engine_fs::virtual_fs::exists(&np).unwrap_or(false))
            || (!remote && np.exists())
        {
            name = format!("New Folder ({suffix})");
            suffix += 1;
            np = cloud_join(f, &name);
        }
        let r = if remote {
            engine_fs::virtual_fs::create_dir_all(&np)
        } else {
            std::fs::create_dir(&np).map_err(Into::into)
        };
        if let Err(e) = r {
            tracing::error!("Failed to create folder: {}", e);
            return;
        }
        crate::utils::start_rename(self, np, window, cx);
        self.mark_directory_cache_dirty();
    }

    pub fn get_filtered_items(&mut self) -> Vec<FileItem> {
        let Some(f) = self.selected_folder.clone() else {
            return Vec::new();
        };
        let mut items = self.cached_items_for_folder(&f);
        items.retain(|i| {
            if !self.show_hidden_files && i.name.starts_with('.') {
                return false;
            }
            if self.file_filter_query.is_empty() {
                return true;
            }
            i.name
                .to_lowercase()
                .contains(&self.file_filter_query.to_lowercase())
        });
        items.sort_by(|a, b| {
            let c = match self.sort_by {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Modified => a.modified.cmp(&b.modified),
                SortBy::Size => a.size.cmp(&b.size),
                SortBy::Type => a.display_name().cmp(b.display_name()),
            };
            match self.sort_order {
                SortOrder::Ascending => c,
                SortOrder::Descending => c.reverse(),
            }
        });
        items.sort_by_key(|i| !i.is_folder);

        // Append ghost (deleted) file items when deleted-files mode is active
        if self.show_deleted_files {
            let project = self.project_path.clone();
            for entry in &self.deleted_files {
                let full_path = project
                    .as_ref()
                    .map(|p| p.join(&entry.path))
                    .unwrap_or_else(|| entry.path.clone());
                let name = full_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                items.push(FileItem {
                    path: entry.path.clone(),
                    name,
                    file_type_def: None,
                    is_folder: false,
                    size: 0,
                    modified: None,
                    is_ghost: true,
                    restore_commit: Some(entry.commit_hash.clone()),
                });
            }
        }

        items
    }

    pub fn mark_directory_cache_dirty(&mut self) {
        self.directory_cache_dirty = true;
        self.tree_files.get_mut().clear();
        self.content_revision = self.content_revision.wrapping_add(1);
    }

    pub(crate) fn cached_items_for_folder(&mut self, f: &Path) -> Vec<FileItem> {
        let dirty = self
            .directory_cache
            .as_ref()
            .map(|(cf, _)| cf != f)
            .unwrap_or(true)
            || self.directory_cache_dirty;
        if !dirty {
            if let Some((_, items)) = &self.directory_cache {
                return items.clone();
            }
        }
        let items = self.read_items_for_folder(f);
        self.directory_cache = Some((f.to_path_buf(), items.clone()));
        self.directory_cache_dirty = false;
        items
    }

    pub(crate) fn read_items_for_folder(&self, f: &Path) -> Vec<FileItem> {
        let remote = engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(f);
        if remote {
            let fs = f.to_string_lossy().replace('\\', "/");
            let fs = fs.trim_end_matches('/');
            match engine_fs::virtual_fs::list_dir(f) {
                Ok(entries) => entries
                    .into_iter()
                    .filter_map(|e| {
                        let name = e.name.trim_start_matches('/');
                        if name.is_empty() {
                            return None;
                        }
                        let path = std::path::PathBuf::from(format!("{}/{}", fs, name));
                        let ft = if e.is_dir {
                            self.registered_file_types
                                .iter()
                                .find(|d| {
                                    matches!(d.structure, FileStructure::FolderBased { .. })
                                        && (name.ends_with(&format!(".{}", d.extension))
                                            || path.extension().and_then(|x| x.to_str())
                                                == Some(d.extension.as_str()))
                                })
                                .cloned()
                        } else {
                            self.registered_file_types
                                .iter()
                                .find(|d| {
                                    matches!(d.structure, FileStructure::Standalone)
                                        && (name.ends_with(&format!(".{}", d.extension))
                                            || path.extension().and_then(|x| x.to_str())
                                                == Some(d.extension.as_str()))
                                })
                                .cloned()
                        };
                        Some(FileItem {
                            path,
                            name: name.to_string(),
                            file_type_def: ft.clone(),
                            is_folder: e.is_dir && ft.is_none(),
                            size: e.size,
                            modified: e
                                .modified
                                .map(|s| std::time::UNIX_EPOCH + std::time::Duration::from_secs(s)),
                            is_ghost: false,
                            restore_commit: None,
                        })
                    })
                    .collect(),
                Err(e) => {
                    tracing::error!("Failed to list remote dir: {}", e);
                    Vec::new()
                }
            }
        } else {
            if let Some(project_root) = &self.project_path {
                if let Ok(Some(index)) = engine_fs::open_project_file_index(project_root) {
                    let relative_folder = f
                        .strip_prefix(project_root)
                        .unwrap_or_else(|_| Path::new(""))
                        .to_string_lossy()
                        .replace('\\', "/");
                    let entries = index.query(&engine_fs::FileIndexQuery {
                        direct_children_of: Some(relative_folder),
                        include_directories: true,
                        ..Default::default()
                    });
                    if let Ok(entries) = entries {
                        let marker_paths: HashSet<String> = self
                            .registered_file_types
                            .iter()
                            .filter_map(|definition| match &definition.structure {
                                FileStructure::FolderBased { marker_file, .. } => {
                                    Some(marker_file.as_str())
                                }
                                FileStructure::Standalone => None,
                            })
                            .flat_map(|marker| {
                                index
                                    .query(&engine_fs::FileIndexQuery {
                                        name_equals: Some(marker.to_owned()),
                                        ..Default::default()
                                    })
                                    .unwrap_or_default()
                                    .into_iter()
                                    .map(|entry| entry.path)
                                    .collect::<Vec<_>>()
                            })
                            .collect();
                        return entries
                            .into_iter()
                            .map(|entry| {
                                let path = project_root.join(&entry.path);
                                let file_type_def = if entry.is_dir {
                                    self.registered_file_types
                                        .iter()
                                        .find(|definition| {
                                            if let FileStructure::FolderBased {
                                                marker_file, ..
                                            } = &definition.structure
                                            {
                                                marker_paths.contains(&format!(
                                                    "{}/{}",
                                                    entry.path, marker_file
                                                )) || entry.name.ends_with(&format!(
                                                    ".{}",
                                                    definition.extension
                                                )) || Path::new(&entry.name)
                                                    .extension()
                                                    .and_then(|extension| extension.to_str())
                                                    == Some(definition.extension.as_str())
                                            } else {
                                                false
                                            }
                                        })
                                        .cloned()
                                } else {
                                    self.registered_file_types
                                        .iter()
                                        .find(|definition| {
                                            matches!(
                                                definition.structure,
                                                FileStructure::Standalone
                                            ) && (entry
                                                .name
                                                .ends_with(&format!(".{}", definition.extension))
                                                || entry.extension.as_deref()
                                                    == Some(definition.extension.as_str()))
                                        })
                                        .cloned()
                                };
                                FileItem {
                                    path,
                                    name: entry.name,
                                    file_type_def: file_type_def.clone(),
                                    is_folder: entry.is_dir && file_type_def.is_none(),
                                    size: entry.size,
                                    modified: entry.modified.map(|seconds| {
                                        std::time::UNIX_EPOCH
                                            + std::time::Duration::from_secs(
                                                u64::try_from(seconds).unwrap_or_default(),
                                            )
                                    }),
                                    is_ghost: false,
                                    restore_commit: None,
                                }
                            })
                            .collect();
                    }
                }
            }

            let Ok(read) = std::fs::read_dir(f) else {
                return Vec::new();
            };
            read.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter_map(|p| FileItem::from_path(&p, &self.registered_file_types))
                .collect()
        }
    }
}
