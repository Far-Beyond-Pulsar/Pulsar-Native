// Drag and drop handlers for file manager drawer

impl FileManagerDrawer {
    /// Handle dropping files onto a folder using GPUI's drag and drop API
    pub fn handle_drop_on_folder_new(&mut self, target_folder: &PathBuf, source_paths: &[PathBuf], cx: &mut Context<Self>) {
        let target_folder = target_folder.clone();
        let source_paths = source_paths.to_vec();

        // Don't allow dropping onto itself
        if source_paths.contains(&target_folder) {
            tracing::warn!("[FILE_MANAGER] ❌ Cannot drop folder onto itself");
            return;
        }

        // Don't allow dropping onto a child of the dragged item
        for source_path in &source_paths {
            if target_folder.starts_with(source_path) {
                tracing::warn!("[FILE_MANAGER] ❌ Cannot drop folder onto its own child");
                return;
            }
        }

        // Move the files
        match self.operations.move_items(&source_paths, &target_folder) {
            Ok(_) => {
                tracing::info!("[FILE_MANAGER] ✅ Moved {} item(s) to {:?}",
                    source_paths.len(), target_folder.file_name());

                // Clear selection after move
                self.selected_items.clear();

                // Refresh the folder tree
                if let Some(ref path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(path);
                }

                // Select the target folder to show where items were moved
                self.selected_folder = Some(target_folder);
            }
            Err(e) => {
                tracing::error!("[FILE_MANAGER] ❌ Failed to move items: {}", e);
            }
        }

        cx.notify();
    }

    /// Cancel the current drag operation (no-op with GPUI drag API)
    pub fn cancel_drag(&mut self, _cx: &mut Context<Self>) {
        // GPUI handles drag cancellation automatically
    }

    /// Start a timer to navigate to a breadcrumb path after 1 second of hovering
    pub fn start_breadcrumb_hover_timer(&mut self, path: &PathBuf, cx: &mut Context<Self>) {
        let path = path.clone();

        // If we're already hovering this path, don't restart the timer
        if self.breadcrumb_hover_path.as_ref() == Some(&path) {
            return;
        }

        // Cancel any existing timer
        self.breadcrumb_hover_timer = None;
        self.breadcrumb_hover_path = Some(path.clone());

        // Start a new 1-second timer
        let timer = cx.spawn(async move |drawer, mut cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;

            // Navigate to the folder
            let _ = cx.update(|cx| {
                drawer.update(cx, |drawer, cx| {
                    drawer.selected_folder = Some(path);
                    drawer.breadcrumb_hover_timer = None;
                    drawer.breadcrumb_hover_path = None;
                    tracing::debug!("[FILE_MANAGER] 🎯 Navigated to folder via breadcrumb hover");
                    cx.notify();
                })
            });
        });

        self.breadcrumb_hover_timer = Some(timer);
    }
}
