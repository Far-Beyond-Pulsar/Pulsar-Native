impl FileManagerDrawer {
    pub fn handle_folder_select(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_folder = Some(path.clone());
        self.selected_items.clear();

        // Toggle expanded state in tree
        if let Some(ref mut tree) = self.folder_tree {
            tree.toggle_expanded(&path);
        }

        cx.notify();
    }

    pub fn handle_item_click(&mut self, item: &FileItem, modifiers: &Modifiers, cx: &mut Context<Self>) {
        // Single click just selects items
        if modifiers.control || modifiers.platform {
            if self.selected_items.contains(&item.path) {
                self.selected_items.remove(&item.path);
            } else {
                self.selected_items.insert(item.path.clone());
            }
        } else {
            self.selected_items.clear();
            self.selected_items.insert(item.path.clone());
        }

        cx.notify();
    }

    pub fn handle_item_double_click(&mut self, item: &FileItem, cx: &mut Context<Self>) {
        // Double click opens folders or files
        if item.is_folder {
            self.selected_folder = Some(item.path.clone());
        } else {
            cx.emit(FileSelected {
                path: item.path.clone(),
                file_type_def: item.file_type_def.clone(),
            });
        }
        cx.notify();
    }
}
