impl FileManagerDrawer {
    pub fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(old_path) = self.renaming_item.take() else {
            return;
        };

        let new_name = self.rename_input_state.read(cx).text().to_string().trim().to_string();

        // Validate
        if new_name.is_empty() {
            cx.notify();
            return;
        }

        let old_name = old_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if new_name == old_name {
            cx.notify();
            return;
        }

        // Check for invalid characters
        if new_name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            tracing::error!("Invalid filename: {}", new_name);
            cx.notify();
            return;
        }

        // Perform rename
        match self.operations.rename_item(&old_path, &new_name) {
            Ok(new_path) => {
                // Update metadata
                if let Err(e) = self.fs_metadata.rename_file(&old_path, &new_path) {
                    tracing::error!("Failed to update metadata for rename: {}", e);
                }

                // Update selections
                if self.selected_folder.as_ref() == Some(&old_path) {
                    self.selected_folder = Some(new_path.clone());
                }
                if self.selected_items.remove(&old_path) {
                    self.selected_items.insert(new_path);
                }

                // Refresh tree
                if let Some(ref project_path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(project_path);
                }
            }
            Err(e) => {
                tracing::error!("Rename failed: {}", e);
            }
        }

        cx.notify();
    }

    pub fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Get the current name
        let current_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        // Set renaming state
        self.renaming_item = Some(path);

        // Clear and set the input text
        self.rename_input_state.update(cx, |state, cx| {
            // First clear everything
            let text_len = state.text().len();
            if text_len > 0 {
                state.replace_text_in_range(Some(0..text_len), "", window, cx);
            }
            // Then insert the current name
            state.replace_text_in_range(Some(0..0), &current_name, window, cx);
        });

        cx.notify();
    }

    pub fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming_item = None;
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(ref project_path) = self.project_path {
            self.folder_tree = FolderNode::from_path(project_path);
        }
        cx.notify();
    }
}
