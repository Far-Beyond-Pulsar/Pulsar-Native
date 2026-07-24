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
        // Model files are converted to engine-native `.mesh` assets in the target
        // folder — the source model itself is NOT brought into the project. Other
        // files (textures, etc.) are copied/moved in as-is.
        let (models, others): (Vec<PathBuf>, Vec<PathBuf>) = s.iter().cloned().partition(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(pulsar_rendering::mesh_cache::is_importable_model)
                .unwrap_or(false)
        });

        let mut any_ok = false;

        // Bring non-model files in unchanged.
        if !others.is_empty() {
            let r = match self.operations.move_items(&others, &t) {
                Ok(_) => Ok(()),
                Err(e) => {
                    tracing::warn!("External move failed: {}, fallback copy", e);
                    FileOperations::copy_items(&others, &t)
                }
            };
            match r {
                Ok(_) => {
                    any_ok = true;
                    for p in &others {
                        let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("file");
                        w.push_notification(
                            Notification::success(format!("Imported \"{}\"", n)),
                            cx,
                        );
                    }
                }
                Err(e) => tracing::error!("Failed to import files: {}", e),
            }
        }

        // For model files, show the import configurator modal; on confirm it
        // converts each to a native `.mesh` asset with the chosen options (the
        // source model is not brought into the project). Falls back to a default
        // import if the format advertises no options schema.
        //
        // NOTE: The modal MUST be opened asynchronously (on the next frame)
        // rather than synchronously during the drop handler. Opening a modal
        // re-enters the event loop, which on macOS prevents the
        // `performDragOperation` callback from returning and releasing the
        // `NSDraggingSession`. This would silently break all subsequent drops.
        if !models.is_empty() {
            let ext = models
                .first()
                .and_then(|p| p.extension())
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_string();
            if let Some(schema) = pulsar_rendering::mesh_cache::options_schema(&ext) {
                use ui_common::PulsarWindowExt as _;
                let params = crate::configurator::ImportConfiguratorParams {
                    sources: models.clone(),
                    target: t.clone(),
                    schema,
                };
                crate::configurator::ImportConfigurator::open(params, cx);
                any_ok = true;
            } else {
                for src in &models {
                    match pulsar_rendering::mesh_cache::import_model_to_native_default(src, &t) {
                        Ok(_) => any_ok = true,
                        Err(e) => {
                            tracing::error!("Model import failed for {}: {}", src.display(), e)
                        }
                    }
                }
            }
        }

        if any_ok {
            self.selected_items.clear();
            if let Some(ref p) = self.project_path {
                self.folder_tree = FolderNode::from_path(p);
            }
            self.mark_directory_cache_dirty();
            self.selected_folder = Some(t);
        }
        self.hovered_drop_folder = None;
        self.show_drop_hint = false;
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
