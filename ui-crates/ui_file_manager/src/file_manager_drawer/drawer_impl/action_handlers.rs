impl FileManagerDrawer {
    pub fn handle_create_asset(&mut self, action: &CreateAsset, cx: &mut Context<Self>) {
        if let Some(folder) = &self.selected_folder {
            // Create file name with extension
            let file_name = format!("New{}.{}", action.display_name.replace(" ", ""), action.extension);
            let file_path = folder.join(&file_name);

            // Check if file already exists and generate unique name
            let mut counter = 1;
            let mut final_path = file_path.clone();
            while final_path.exists() {
                let file_name = format!("New{}_{}.{}", action.display_name.replace(" ", ""), counter, action.extension);
                final_path = folder.join(&file_name);
                counter += 1;
            }

            // Create the file with default content from the file type definition
            let content = if action.default_content.is_null() {
                // For SQLite databases, create an empty database file
                if action.extension == "db" || action.extension == "sqlite" || action.extension == "sqlite3" {
                    // SQLite databases need proper initialization, which will be handled by the editor
                    // For now, create an empty file that the editor will initialize
                    vec![]
                } else {
                    // For other files, use empty content
                    vec![]
                }
            } else {
                // Use the default content from the file type definition
                action.default_content.to_string().into_bytes()
            };

            if let Err(e) = std::fs::write(&final_path, content) {
                tracing::error!("Failed to create file {:?}: {}", final_path, e);
            } else {
                // Refresh the folder tree
                if let Some(ref path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(path);
                }
                cx.notify();
            }
        }
    }

    pub fn handle_new_folder(&mut self, _action: &NewFolder, cx: &mut Context<Self>) {
        if let Some(folder) = &self.selected_folder {
            // Create folder with unique name
            let mut counter = 1;
            let mut folder_name = "NewFolder".to_string();
            let mut folder_path = folder.join(&folder_name);

            while folder_path.exists() {
                folder_name = format!("NewFolder_{}", counter);
                folder_path = folder.join(&folder_name);
                counter += 1;
            }

            // Create the folder
            if let Err(e) = std::fs::create_dir(&folder_path) {
                tracing::error!("Failed to create folder {:?}: {}", folder_path, e);
            } else {
                // Refresh the folder tree
                if let Some(ref path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(path);
                }
                cx.notify();
            }
        }
    }

    pub fn handle_delete_item(&mut self, cx: &mut Context<Self>) {
        let items_to_delete: Vec<PathBuf> = self.selected_items.iter().cloned().collect();

        for item in items_to_delete {
            if item.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&item) {
                    tracing::error!("Failed to delete folder {:?}: {}", item, e);
                }
            } else {
                if let Err(e) = std::fs::remove_file(&item) {
                    tracing::error!("Failed to delete file {:?}: {}", item, e);
                }
            }
        }

        self.selected_items.clear();

        // Refresh the folder tree
        if let Some(ref path) = self.project_path {
            self.folder_tree = FolderNode::from_path(path);
        }

        cx.notify();
    }

    pub fn handle_rename_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_items.iter().next().cloned() {
            self.start_rename(item, window, cx);
        }
    }

    pub fn handle_duplicate_item(&mut self, cx: &mut Context<Self>) {
        let items_to_duplicate: Vec<PathBuf> = self.selected_items.iter().cloned().collect();

        for item in items_to_duplicate {
            if let Some(parent) = item.parent() {
                if let Some(name) = item.file_name() {
                    let name_str = name.to_string_lossy();

                    // Generate unique name
                    let mut counter = 1;
                    let mut new_name = format!("{}_copy", name_str);
                    let mut new_path = parent.join(&new_name);

                    while new_path.exists() {
                        new_name = format!("{}_copy_{}", name_str, counter);
                        new_path = parent.join(&new_name);
                        counter += 1;
                    }

                    // Copy the item
                    if item.is_dir() {
                        if let Err(e) = Self::copy_dir_recursive(&item, &new_path) {
                            tracing::error!("Failed to duplicate folder {:?}: {}", item, e);
                        }
                    } else {
                        if let Err(e) = std::fs::copy(&item, &new_path) {
                            tracing::error!("Failed to duplicate file {:?}: {}", item, e);
                        }
                    }
                }
            }
        }

        // Refresh the folder tree
        if let Some(ref path) = self.project_path {
            self.folder_tree = FolderNode::from_path(path);
        }

        cx.notify();
    }

    pub fn handle_copy(&mut self, _cx: &mut Context<Self>) {
        let items: Vec<PathBuf> = self.selected_items.iter().cloned().collect();
        self.clipboard = Some((items, false));
    }

    pub fn handle_cut(&mut self, _cx: &mut Context<Self>) {
        let items: Vec<PathBuf> = self.selected_items.iter().cloned().collect();
        self.clipboard = Some((items, true));
    }

    pub fn handle_paste(&mut self, cx: &mut Context<Self>) {
        if let Some((items, is_cut)) = &self.clipboard {
            if let Some(target_folder) = &self.selected_folder {
                for item in items.iter() {
                    if let Some(name) = item.file_name() {
                        let target_path = target_folder.join(name);

                        if *is_cut {
                            // Move operation
                            if let Err(e) = std::fs::rename(item, &target_path) {
                                tracing::error!("Failed to move {:?} to {:?}: {}", item, target_path, e);
                            }
                        } else {
                            // Copy operation
                            if item.is_dir() {
                                if let Err(e) = Self::copy_dir_recursive(item, &target_path) {
                                    tracing::error!("Failed to copy folder {:?}: {}", item, e);
                                }
                            } else {
                                if let Err(e) = std::fs::copy(item, &target_path) {
                                    tracing::error!("Failed to copy file {:?}: {}", item, e);
                                }
                            }
                        }
                    }
                }

                // Clear clipboard if it was a cut operation
                if *is_cut {
                    self.clipboard = None;
                }

                // Refresh the folder tree
                if let Some(ref path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(path);
                }

                cx.notify();
            }
        }
    }

    fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;

        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }

        Ok(())
    }

    pub fn handle_open_in_file_manager(&mut self, _action: &OpenInFileManager, _cx: &mut Context<Self>) {
        // Get the path to open - either the selected folder or first selected item
        let path_to_open = if let Some(folder) = &self.selected_folder {
            Some(folder.clone())
        } else {
            self.selected_items.iter().next().cloned()
        };

        if let Some(path) = path_to_open {
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("explorer")
                    .arg(path.to_string_lossy().to_string())
                    .spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open")
                    .arg(path.to_string_lossy().to_string())
                    .spawn();
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("xdg-open")
                    .arg(path.to_string_lossy().to_string())
                    .spawn();
            }
        }
    }

    pub fn handle_open_terminal_here(&mut self, _action: &OpenTerminalHere, _cx: &mut Context<Self>) {
        let folder = self.selected_folder.clone().or_else(|| {
            self.selected_items.iter().next().and_then(|p| {
                if p.is_dir() {
                    Some(p.clone())
                } else {
                    p.parent().map(|p| p.to_path_buf())
                }
            })
        });

        if let Some(folder) = folder {
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("cmd")
                    .args(&["/c", "start", "cmd"])
                    .current_dir(folder)
                    .spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open")
                    .args(&["-a", "Terminal"])
                    .arg(folder.to_string_lossy().to_string())
                    .spawn();
            }
            #[cfg(target_os = "linux")]
            {
                // Try common terminal emulators
                for term in &["gnome-terminal", "konsole", "xterm"] {
                    if std::process::Command::new(term)
                        .current_dir(&folder)
                        .spawn()
                        .is_ok()
                    {
                        break;
                    }
                }
            }
        }
    }

    pub fn handle_validate_asset(&mut self, _action: &ValidateAsset, _cx: &mut Context<Self>) {
        // TODO: Implement asset validation
        // This would check if the asset file is valid according to its type
        tracing::info!("Validate asset action triggered - not yet implemented");
    }

    pub fn handle_toggle_favorite(&mut self, _action: &ToggleFavorite, _cx: &mut Context<Self>) {
        // TODO: Implement favorite toggling
        // This would mark/unmark files as favorites (stored in project settings)
        tracing::info!("Toggle favorite action triggered - not yet implemented");
    }

    pub fn handle_toggle_gitignore(&mut self, _action: &ToggleGitignore, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_items.iter().next() {
            if let Some(project_path) = &self.project_path {
                let gitignore_path = project_path.join(".gitignore");

                // Read existing .gitignore or create empty string
                let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();

                // Get relative path from project root
                if let Ok(relative_path) = item.strip_prefix(project_path) {
                    let pattern = relative_path.to_string_lossy().replace('\\', "/");

                    if content.lines().any(|line| line.trim() == pattern) {
                        // Remove from .gitignore
                        let new_content: String = content
                            .lines()
                            .filter(|line| line.trim() != pattern)
                            .collect::<Vec<_>>()
                            .join("\n");
                        let _ = std::fs::write(&gitignore_path, new_content);
                        tracing::info!("Removed {} from .gitignore", pattern);
                    } else {
                        // Add to .gitignore
                        let new_content = if content.is_empty() {
                            pattern
                        } else {
                            format!("{}\n{}", content.trim_end(), pattern)
                        };
                        let _ = std::fs::write(&gitignore_path, new_content);
                        tracing::info!("Added pattern to .gitignore");
                    }


                    cx.notify();
                }
            }
        }
    }

    pub fn handle_toggle_hidden(&mut self, _action: &ToggleHidden, _cx: &mut Context<Self>) {
        // TODO: Implement hidden file toggling
        // This would mark files as hidden (on Windows, set hidden attribute; on Unix, rename with dot prefix)
        tracing::info!("Toggle hidden action triggered - not yet implemented");
    }

    pub fn handle_show_history(&mut self, _action: &ShowHistory, _cx: &mut Context<Self>) {
        // TODO: Implement file history viewer
        // This would show git history or file modification history
        tracing::info!("Show history action triggered - not yet implemented");
    }

    pub fn handle_check_multiuser_sync(&mut self, _action: &CheckMultiuserSync, _cx: &mut Context<Self>) {
        // TODO: Implement multiuser sync check
        // This would check if all connected peers have this file synced
        tracing::info!("Check multiuser sync action triggered - not yet implemented");
    }

    pub fn handle_set_color_override(&mut self, action: &SetColorOverride, cx: &mut Context<Self>) {
        let item_path = if action.item_path.is_empty() {
            // Use first selected item
            self.selected_items.iter().next().cloned()
        } else {
            Some(PathBuf::from(&action.item_path))
        };

        if let Some(path) = item_path {
            let color = action.color.as_ref().map(|c| {
                let hex = ((c.r as u32) << 16) | ((c.g as u32) << 8) | (c.b as u32);
                gpui::rgb(hex).into()
            });

            if let Err(e) = self.fs_metadata.set_color_override(&path, color) {
                tracing::error!("Failed to set color override: {}", e);
            } else {
                cx.notify(); // Refresh UI
            }
        }
    }

    pub fn set_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        tracing::debug!("[FILE_MANAGER] set_project_path called with: {:?}", path);
        tracing::debug!("[FILE_MANAGER] Path exists: {}", path.exists());
        tracing::debug!("[FILE_MANAGER] Path is_dir: {}", path.is_dir());

        self.project_path = Some(path.clone());
        self.folder_tree = FolderNode::from_path(&path);
        self.selected_folder = Some(path.clone());

        tracing::debug!("[FILE_MANAGER] folder_tree is_some: {}", self.folder_tree.is_some());
        tracing::debug!("[FILE_MANAGER] selected_folder: {:?}", self.selected_folder);

        cx.notify();
    }
}
