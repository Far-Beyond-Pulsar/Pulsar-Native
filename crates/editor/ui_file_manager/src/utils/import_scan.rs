//! Offering imports for source files the project already holds.
//!
//! When the drawer opens a project, and whenever an importable file in it is
//! added or changed, it scans (as a task in the Tasks window) for importable
//! sources — FBX, OBJ, … — with no import record, and for linked sources that
//! changed since they were imported. Unlinked sources go to the import
//! configurator, once per format; changed ones get a one-click reimport.
//! Each is offered once per session ([`asset_import::OfferedOnce`]), however
//! many drawers scan.

use std::sync::LazyLock;
use std::time::Duration;

use gpui::*;
use notify::Watcher as _;
use parking_lot::Mutex;
use ui::button::{Button, ButtonVariants as _};
use ui::notification::Notification;
use ui::ContextModal as _;

use crate::components::FileManagerDrawer;

/// What this session already offered, shared by every drawer.
static OFFERED: LazyLock<Mutex<asset_import::OfferedOnce>> = LazyLock::new(Default::default);

/// How long the watcher waits for a burst of changes (a copy, a save) to
/// settle before rescanning.
const SETTLE: Duration = Duration::from_millis(500);

/// Keeps a drawer's import watcher running; dropping it stops the watcher.
pub struct ImportWatch {
    _watcher: notify::RecommendedWatcher,
    _rescans: Task<()>,
}

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
            let (tx, rx) = smol::channel::bounded(1);
            asset_import::submit_scan(root.clone(), move |report| {
                let _ = tx.try_send(report);
            });
            let Ok(report) = rx.recv().await else {
                return;
            };
            let report = OFFERED.lock().take_new(&root, report);
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

    /// Rescan whenever an importable file in the project is added or
    /// changed, once the changes settle. `None` for cloud projects and when
    /// the platform watcher can't start.
    pub fn watch_for_imports(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<ImportWatch> {
        let root = self.project_path.clone()?;
        if engine_fs::is_cloud_path(&root) {
            return None;
        }
        let (tx, rx) = smol::channel::unbounded::<()>();
        let filter_root = root.clone();
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else {
                return;
            };
            let relevant = matches!(
                event.kind,
                notify::EventKind::Create(_) | notify::EventKind::Modify(_)
            ) && event
                .paths
                .iter()
                .any(|path| asset_import::affects_scan(&filter_root, path));
            if relevant {
                let _ = tx.try_send(());
            }
        })
        .inspect_err(|error| tracing::warn!("import watcher unavailable: {error}"))
        .ok()?;
        watcher
            .watch(&root, notify::RecursiveMode::Recursive)
            .inspect_err(|error| tracing::warn!("cannot watch {} for imports: {error}", root.display()))
            .ok()?;

        let rescans = cx.spawn_in(window, async move |this, cx| {
            while rx.recv().await.is_ok() {
                // Let the burst finish, then scan once for all of it.
                cx.background_executor().timer(SETTLE).await;
                while rx.try_recv().is_ok() {}
                if this
                    .update_in(cx, |drawer, window, cx| drawer.scan_for_imports(window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        Some(ImportWatch {
            _watcher: watcher,
            _rescans: rescans,
        })
    }
}
