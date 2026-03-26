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
            cx.notify();
        }
    }

    pub fn get_filtered_items(&self) -> Vec<FileItem> {
        let Some(ref folder) = self.selected_folder else {
            return Vec::new();
        };

        // List directory — either locally or remotely depending on the active provider.
        let remote = engine_fs::virtual_fs::is_remote() || engine_fs::is_cloud_path(folder);

        let mut items: Vec<FileItem> = if remote {
            // ── Remote path ──────────────────────────────────────────────────
            // Build FileItem objects directly from the FsEntry data returned by
            // list_dir.  This avoids N+1 blocking HTTP /stat calls that would
            // stall the UI thread for every visible file.
            //
            // Use string concatenation for path construction (never PathBuf::join)
            // so that Windows doesn't insert backslashes into cloud+pulsar:// URIs.
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
                        // Hidden-file filter.
                        if !self.show_hidden_files && name.starts_with('.') {
                            return None;
                        }
                        // Search filter.
                        if !self.file_filter_query.is_empty()
                            && !name.to_lowercase().contains(&self.file_filter_query.to_lowercase())
                        {
                            return None;
                        }

                        // Forward-slash path — safe on every OS.
                        let path = PathBuf::from(format!("{}/{}", folder_s, name));

                        // Extension-based file-type lookup (no HTTP call needed).
                        let file_type_def = if !e.is_dir {
                            self.registered_file_types.iter().find(|def| {
                                matches!(def.structure, plugin_editor_api::FileStructure::Standalone)
                                    && (name.ends_with(&format!(".{}", def.extension))
                                        || path
                                            .extension()
                                            .and_then(|x| x.to_str())
                                            == Some(def.extension.as_str()))
                            }).cloned()
                        } else {
                            None
                        };

                        let modified = e.modified.map(|secs| {
                            std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)
                        });

                        Some(FileItem {
                            path,
                            name: name.to_string(),
                            file_type_def,
                            is_folder: e.is_dir,
                            size: e.size,
                            modified,
                        })
                    })
                    .collect(),
                Err(e) => {
                    tracing::error!("Failed to list remote dir: {}", e);
                    return Vec::new();
                }
            }
        } else {
            // ── Local path ───────────────────────────────────────────────────
            let Ok(read) = std::fs::read_dir(folder) else {
                return Vec::new();
            };
            read.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter_map(|path| {
                    if !self.show_hidden_files {
                        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                            if name.starts_with('.') {
                                return None;
                            }
                        }
                    }
                    FileItem::from_path(&path, &self.registered_file_types)
                })
                .filter(|item| {
                    if !self.file_filter_query.is_empty() {
                        item.name
                            .to_lowercase()
                            .contains(&self.file_filter_query.to_lowercase())
                    } else {
                        true
                    }
                })
                .collect()
        };

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
}
