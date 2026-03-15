impl FileManagerDrawer {
    pub fn start_new_file(&mut self, cx: &mut Context<Self>) {
        if let Some(ref folder) = self.selected_folder {
            let new_path = folder.join("untitled.txt");
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
            let new_path = folder.join("New Folder");
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
        let items_raw: Vec<std::path::PathBuf> = if remote {
            match engine_fs::virtual_fs::list_dir(folder) {
                Ok(entries) => entries.iter().map(|e| {
                    // Reconstruct a path from the filename returned by the provider.
                    let name = e.name.trim_start_matches('/');
                    PathBuf::from(format!("{}/{}",
                        folder.to_string_lossy().trim_end_matches('/'), name))
                }).collect(),
                Err(e) => {
                    tracing::error!("Failed to list remote dir: {}", e);
                    return Vec::new();
                }
            }
        } else {
            let Ok(read) = std::fs::read_dir(folder) else {
                return Vec::new();
            };
            read.filter_map(|e| e.ok()).map(|e| e.path()).collect()
        };

        let mut items: Vec<FileItem> = items_raw
            .into_iter()
            .filter_map(|path| {
                // Filter hidden files
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
                // Apply search filter
                if !self.file_filter_query.is_empty() {
                    item.name
                        .to_lowercase()
                        .contains(&self.file_filter_query.to_lowercase())
                } else {
                    true
                }
            })
            .collect();

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
