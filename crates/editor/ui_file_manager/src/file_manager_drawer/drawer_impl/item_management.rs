impl FileManagerDrawer {
    pub fn start_new_file(&mut self, cx: &mut Context<Self>) {
        if let Some(ref folder) = self.selected_folder {
            let new_path = cloud_join(folder, "untitled.txt");
            let result = if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(folder) {
                engine_fs::virtual_fs::write_file(&new_path, b"")
            } else {
                std::fs::write(&new_path, "").map_err(Into::into)
            };
            if let Err(e) = result {
                tracing::error!("Failed to create file: {}", e);
                return;
            }
            self.renaming_item = Some(new_path);
            self.mark_directory_cache_dirty();
            cx.notify();
        }
    }

    pub fn start_new_folder(&mut self, cx: &mut Context<Self>) {
        if let Some(ref folder) = self.selected_folder {
            let new_path = cloud_join(folder, "New Folder");
            let result = if engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(folder) {
                engine_fs::virtual_fs::create_dir_all(&new_path)
            } else {
                std::fs::create_dir(&new_path).map_err(Into::into)
            };
            if let Err(e) = result {
                tracing::error!("Failed to create folder: {}", e);
                return;
            }
            self.renaming_item = Some(new_path);
            self.mark_directory_cache_dirty();
            cx.notify();
        }
    }

    pub fn get_filtered_items(&mut self) -> Vec<FileItem> {
        let Some(folder) = self.selected_folder.clone() else {
            return Vec::new();
        };

        let mut items = self.cached_items_for_folder(&folder);

        items.retain(|item| {
            if !self.show_hidden_files && item.name.starts_with('.') {
                return false;
            }
            if !self.file_filter_query.is_empty() {
                item.name
                    .to_lowercase()
                    .contains(&self.file_filter_query.to_lowercase())
            } else {
                true
            }
        });

        // Sort items
        items.sort_by(|a, b| {
            let cmp = match self.sort_by {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Modified => a.modified.cmp(&b.modified),
                SortBy::Size => a.size.cmp(&b.size),
                SortBy::Type => a.display_name().cmp(b.display_name()),
            };

            match self.sort_order {
                SortOrder::Ascending => cmp,
                SortOrder::Descending => cmp.reverse(),
            }
        });

        // Folders first
        items.sort_by_key(|item| !item.is_folder);

        items
    }

    pub fn mark_directory_cache_dirty(&mut self) {
        self.directory_cache_dirty = true;
    }

    fn cached_items_for_folder(&mut self, folder: &Path) -> Vec<FileItem> {
        let needs_refresh = self
            .directory_cache
            .as_ref()
            .map(|(cached_folder, _)| cached_folder != folder)
            .unwrap_or(true)
            || self.directory_cache_dirty;

        if !needs_refresh {
            if let Some((_, items)) = &self.directory_cache {
                return items.clone();
            }
        }

        let items = self.read_items_for_folder(folder);
        self.directory_cache = Some((folder.to_path_buf(), items.clone()));
        self.directory_cache_dirty = false;
        items
    }

    fn read_items_for_folder(&self, folder: &Path) -> Vec<FileItem> {
        let remote = engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(folder);

        let items: Vec<FileItem> = if remote {
            let folder_s = folder.to_string_lossy().replace('\\', "/");
            let folder_s = folder_s.trim_end_matches('/');

            match engine_fs::virtual_fs::list_dir(folder) {
                Ok(entries) => entries
                    .into_iter()
                    .filter_map(|e| {
                        let name = e.name.trim_start_matches('/');
                        if name.is_empty() {
                            return None;
                        }

                        let path = PathBuf::from(format!("{}/{}", folder_s, name));
                        let file_type_def = if e.is_dir {
                            self.registered_file_types
                                .iter()
                                .find(|def| {
                                    matches!(
                                        def.structure,
                                        plugin_editor_api::FileStructure::FolderBased { .. }
                                    ) && (name.ends_with(&format!(".{}", def.extension))
                                        || path.extension().and_then(|x| x.to_str())
                                            == Some(def.extension.as_str()))
                                })
                                .cloned()
                        } else {
                            self.registered_file_types
                                .iter()
                                .find(|def| {
                                    matches!(
                                        def.structure,
                                        plugin_editor_api::FileStructure::Standalone
                                    ) && (name.ends_with(&format!(".{}", def.extension))
                                        || path.extension().and_then(|x| x.to_str())
                                            == Some(def.extension.as_str()))
                                })
                                .cloned()
                        };

                        let modified = e.modified.map(|secs| {
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)
                        });
                        let is_folder = e.is_dir && file_type_def.is_none();

                        Some(FileItem {
                            path,
                            name: name.to_string(),
                            file_type_def,
                            is_folder,
                            size: e.size,
                            modified,
                        })
                    })
                    .collect(),
                Err(e) => {
                    tracing::error!("Failed to list remote dir: {}", e);
                    Vec::new()
                }
            }
        } else {
            let Ok(read) = std::fs::read_dir(folder) else {
                return Vec::new();
            };
            read.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter_map(|path| FileItem::from_path(&path, &self.registered_file_types))
                .collect()
        };

        items
    }
}
