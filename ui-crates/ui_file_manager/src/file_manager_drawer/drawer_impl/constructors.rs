impl FileManagerDrawer {
    pub fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let resizable_state = ResizableState::new(cx);
        let rename_input_state = cx.new(|cx| InputState::new(window, cx));
        let folder_search_state = cx.new(|cx| InputState::new(window, cx));
        let file_filter_state = cx.new(|cx| InputState::new(window, cx));

        // Simple rename subscription - only handle Enter
        cx.subscribe(
            &rename_input_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::PressEnter { .. } = event {
                    drawer.commit_rename(cx);
                }
            },
        )
        .detach();

        // Subscribe to search inputs
        cx.subscribe(
            &folder_search_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change { .. } = event {
                    drawer.search_query = drawer.folder_search_state.read(cx).text().to_string();
                    cx.notify();
                }
            },
        )
        .detach();

        cx.subscribe(
            &file_filter_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change { .. } = event {
                    drawer.file_filter_query = drawer.file_filter_state.read(cx).text().to_string();
                    cx.notify();
                }
            },
        )
        .detach();

        let operations = FileOperations::new(project_path.clone());
        let fs_metadata = FsMetadataManager::new();

        Self {
            folder_tree: project_path.as_ref().and_then(|p| FolderNode::from_path(p)),
            project_path: project_path.clone(),
            selected_folder: project_path,
            selected_items: HashSet::new(),
            operations,
            fs_metadata,
            drag_state: DragState::None,
            breadcrumb_hover_timer: None,
            breadcrumb_hover_path: None,
            resizable_state,
            renaming_item: None,
            rename_input_state,
            view_mode: ViewMode::Grid,
            sort_by: SortBy::Name,
            sort_order: SortOrder::Ascending,
            search_query: String::new(),
            folder_search_state,
            file_filter_query: String::new(),
            file_filter_state,
            show_hidden_files: false,
            clipboard: None,
            registered_file_types: Vec::new(), // Will be populated from plugin manager
        }
    }

    pub fn new_in_window(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(project_path, window, cx)
    }

    /// Update registered file types from the plugin manager
    pub fn update_file_types(&mut self, file_types: Vec<plugin_editor_api::FileTypeDefinition>) {
        self.registered_file_types = file_types;
    }
}
