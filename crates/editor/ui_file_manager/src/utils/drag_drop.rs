use gpui::prelude::*;
use gpui::*;
use std::path::{Path, PathBuf};
use ui::notification::Notification;
use ui::ContextModal;

use crate::components::FileManagerDrawer;
use crate::utils::{operations::FileOperations, tree::FolderNode};

impl FileManagerDrawer {
    pub fn handle_drop_on_folder_new(
        &mut self,
        target: &Path,
        paths: &[PathBuf],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = target.to_path_buf();
        let s = paths.to_vec();
        if s.contains(&t) || s.iter().any(|p| t.starts_with(p)) {
            tracing::warn!("Invalid drop target");
            return;
        }
        match self.operations.move_items(&s, &t) {
            Ok(_) => {
                self.selected_items.clear();
                if let Some(ref p) = self.project_path {
                    self.folder_tree = FolderNode::from_path(p);
                }
                self.mark_directory_cache_dirty();

                // Import (bake) any model files that were just brought in
                // (issues #391 / #409): convert with the stored or default
                // options and write the `<source>.mesh` sidecar, so components
                // load the imported result instead of re-converting on load.
                for p in &s {
                    if let Some(name) = p.file_name() {
                        let dest = t.join(name);
                        let ext = dest.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if pulsar_rendering::mesh_cache::is_importable_model(ext) {
                            if let Err(e) =
                                pulsar_rendering::mesh_cache::import_mesh_asset_default(&dest)
                            {
                                tracing::warn!(
                                    "Model import failed for {}: {}",
                                    dest.display(),
                                    e
                                );
                            }
                        }
                    }
                }

                self.selected_folder = Some(t);
                self.hovered_drop_folder = None;
                self.show_drop_hint = false;
                for p in &s {
                    let n = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file")
                        .to_string();
                    w.push_notification(Notification::success(format!("Moved \"{}\"", n)), cx);
                }
            }
            Err(e) => {
                tracing::error!("Failed to move items: {}", e);
                self.hovered_drop_folder = None;
                self.show_drop_hint = false;
            }
        }
        cx.notify();
    }

    pub fn handle_external_drop_on_folder(
        &mut self,
        target: &Path,
        paths: &[PathBuf],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let t = target.to_path_buf();
        let s = paths.to_vec();
        if s.is_empty() {
            return;
        }
        let r = match self.operations.move_items(&s, &t) {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::warn!("External move failed: {}, fallback copy", e);
                FileOperations::copy_items(&s, &t)
            }
        };
        match r {
            Ok(_) => {
                self.selected_items.clear();
                if let Some(ref p) = self.project_path {
                    self.folder_tree = FolderNode::from_path(p);
                }
                self.mark_directory_cache_dirty();

                // Import (bake) any model files that were just brought in
                // (issues #391 / #409): convert with the stored or default
                // options and write the `<source>.mesh` sidecar, so components
                // load the imported result instead of re-converting on load.
                for p in &s {
                    if let Some(name) = p.file_name() {
                        let dest = t.join(name);
                        let ext = dest.extension().and_then(|e| e.to_str()).unwrap_or("");
                        if pulsar_rendering::mesh_cache::is_importable_model(ext) {
                            if let Err(e) =
                                pulsar_rendering::mesh_cache::import_mesh_asset_default(&dest)
                            {
                                tracing::warn!(
                                    "Model import failed for {}: {}",
                                    dest.display(),
                                    e
                                );
                            }
                        }
                    }
                }

                self.selected_folder = Some(t);
                self.hovered_drop_folder = None;
                self.show_drop_hint = false;
                for p in &s {
                    let n = p
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file")
                        .to_string();
                    w.push_notification(Notification::success(format!("Imported \"{}\"", n)), cx);
                }
            }
            Err(e) => {
                tracing::error!("Failed to import: {}", e);
                self.hovered_drop_folder = None;
                self.show_drop_hint = false;
            }
        }
        cx.notify();
    }

    pub fn start_breadcrumb_hover_timer(&mut self, path: &Path, cx: &mut Context<Self>) {
        let p = path.to_path_buf();
        if self.breadcrumb_hover_path.as_ref() == Some(&p) {
            return;
        }
        self.breadcrumb_hover_timer = None;
        self.breadcrumb_hover_path = Some(p.clone());
        self.breadcrumb_hover_timer = Some(cx.spawn(async move |d, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let _ = cx.update(|cx| {
                d.update(cx, |d, cx| {
                    if !cx.has_active_drag() {
                        return;
                    }
                    d.selected_folder = Some(p);
                    d.breadcrumb_hover_timer = None;
                    d.breadcrumb_hover_path = None;
                    cx.notify();
                })
            });
        }));
    }
}

pub fn cancel_drag(d: &mut FileManagerDrawer, cx: &mut gpui::Context<FileManagerDrawer>) {
    d.breadcrumb_hover_timer = None;
    d.breadcrumb_hover_path = None;
    if d.asset_drag_emitted {
        d.asset_drag_emitted = false;
        cx.emit(ui_types_common::DragEvent::AssetDragCancelled);
    }
}
