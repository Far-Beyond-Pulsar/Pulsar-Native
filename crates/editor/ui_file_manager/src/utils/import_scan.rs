//! Offering imports for source files the project already holds.
//!
//! When the drawer opens a project it scans (off the UI thread) for
//! importable sources — FBX, OBJ, … — with no import record, and for linked
//! sources that changed since they were imported. Unlinked sources go to the
//! import configurator, once per format; changed ones get a one-click
//! reimport.

use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::notification::Notification;
use ui::ContextModal as _;

use crate::components::FileManagerDrawer;

impl FileManagerDrawer {
    /// Scan the open project for importable files and out-of-date links.
    pub fn scan_for_imports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.project_path.clone() else {
            return;
        };
        if engine_fs::is_cloud_path(&root) {
            // Imports read and write local files.
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let scan_root = root.clone();
            let report = cx
                .background_executor()
                .spawn(async move { asset_import::scan_project(&scan_root) })
                .await;
            if report.is_empty() {
                return;
            }
            let _ = this.update_in(cx, |_this, window, cx| {
                if !report.out_of_date.is_empty() {
                    let sources: Vec<String> = report
                        .out_of_date
                        .iter()
                        .map(|record| record.source.clone())
                        .collect();
                    let root = root.clone();
                    window.push_notification(
                        Notification::warning(format!(
                            "{} linked source file(s) changed since they were imported",
                            sources.len()
                        ))
                        .autohide(false)
                        .action(move |_window, _cx| {
                            let (root, sources) = (root.clone(), sources.clone());
                            Button::new("reimport-changed")
                                .label("Reimport")
                                .primary()
                                .on_click(move |_, _, _| {
                                    for source in &sources {
                                        asset_import::submit_reimport(root.clone(), source);
                                    }
                                })
                        }),
                        cx,
                    );
                }
                if !report.missing.is_empty() {
                    tracing::warn!(
                        count = report.missing.len(),
                        "linked import sources are missing from the project"
                    );
                }
                if !report.unlinked.is_empty() {
                    crate::configurator::offer_import(root, report.unlinked, cx);
                }
            });
        })
        .detach();
    }
}
