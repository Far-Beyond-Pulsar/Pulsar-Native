impl FileManagerDrawer {
    pub fn start_new_file(&mut self, cx: &mut Context<Self>) {
        if let Some(ref folder) = self.selected_folder {
            let new_path = folder.join("untitled.txt");
            if let Err(e) = std::fs::write(&new_path, "") {
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
            if let Err(e) = std::fs::create_dir(&new_path) {
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

        let Ok(entries) = std::fs::read_dir(folder) else {
            return Vec::new();
        };

        let mut items: Vec<FileItem> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();

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
