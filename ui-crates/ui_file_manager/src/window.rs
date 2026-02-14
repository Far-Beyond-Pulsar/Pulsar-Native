//! File Manager Window - Standalone file browser and manager
//! Similar to the drawer but in a separate window

use gpui::*;
use ui::drawer_window_entity;
use ui_common::translate;

use crate::{FileManagerDrawer, FileSelected};

pub struct FileManagerWindow {
    file_manager: Entity<FileManagerDrawer>,
    // Note: Direct parent reference removed to improve decoupling.
    // Use event emitter pattern instead when parent communication is needed.
    // parent_app: Entity<PulsarApp>,
}

impl FileManagerWindow {
    pub fn new(
        file_manager: Entity<FileManagerDrawer>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Subscribe to file selected events and forward them to parent window
        cx.subscribe_in(&file_manager, window, Self::on_file_selected).detach();

        Self { 
            file_manager,
        }
    }

    pub fn file_manager(&self) -> &Entity<FileManagerDrawer> {
        &self.file_manager
    }

    fn on_file_selected(
        &mut self,
        _drawer: &Entity<FileManagerDrawer>,
        event: &FileSelected,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Emit event for parent to handle
        cx.emit(event.clone());
    }
}

impl EventEmitter<FileSelected> for FileManagerWindow {}

impl Render for FileManagerWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        drawer_window_entity("Window.Title.FileManager", self.file_manager.clone(), cx)
    }
}
