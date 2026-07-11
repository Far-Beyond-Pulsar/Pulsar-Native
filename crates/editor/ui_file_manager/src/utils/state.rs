use std::path::Path;

use crate::components::FileManagerDrawer;
use crate::utils::cloud_join;
use crate::utils::types::*;
use plugin_editor_api::FileStructure;

impl FileManagerDrawer {
    pub fn start_new_file(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(ref f) = self.selected_folder else {
            return;
        };
        let np = cloud_join(f, "untitled.txt");
        let r = if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(f) {
            engine_fs::virtual_fs::write_file(&np, b"")
        } else {
            std::fs::write(&np, "").map_err(Into::into)
        };
        if let Err(e) = r {
            tracing::error!("Failed to create file: {}", e);
            return;
        }
        self.renaming_item = Some(np);
        self.mark_directory_cache_dirty();
        cx.notify();
    }

    pub fn start_new_folder(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(ref f) = self.selected_folder else {
            return;
        };
        let np = cloud_join(f, "New Folder");
        let r = if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(f) {
            engine_fs::virtual_fs::create_dir_all(&np)
        } else {
            std::fs::create_dir(&np).map_err(Into::into)
        };
        if let Err(e) = r {
            tracing::error!("Failed to create folder: {}", e);
            return;
        }
        self.renaming_item = Some(np);
        self.mark_directory_cache_dirty();
        cx.notify();
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
        items
    }

    pub fn mark_directory_cache_dirty(&mut self) {
        self.directory_cache_dirty = true;
    }

    fn cached_items_for_folder(&mut self, f: &Path) -> Vec<FileItem> {
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

    fn read_items_for_folder(&self, f: &Path) -> Vec<FileItem> {
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
                        })
                    })
                    .collect(),
                Err(e) => {
                    tracing::error!("Failed to list remote dir: {}", e);
                    Vec::new()
                }
            }
        } else {
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
