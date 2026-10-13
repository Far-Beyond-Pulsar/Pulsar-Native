//! Saving and restoring the editor layout and open tabs, per project.
//!
//! **What is saved.** The dock framework's own `DockAreaState`: the split tree
//! with its sizes, every tab group with its active tab, and each side dock's
//! size / open state; plus the window geometry and the sidebar. A tab that edits
//! a file records that file (the `TabPanel` dump does this for any panel with a
//! `panel_file_path`). Paths inside the project are stored relative to it. The
//! record lives at `<project>/.pulsar/layout.json`, a map of window slots (see
//! [`format`]); this window reads and writes slot 0 and leaves the others alone.
//!
//! **When.** Any `DockEvent::LayoutChanged` (tab opened / closed / moved /
//! activated, split or dock resized, dock toggled) arms a 4 second timer; a
//! further change before it fires re-arms it, so a drag that resizes
//! continuously is written once, 4 s after the last movement. The layout is
//! also flushed when the window closes.
//!
//! **Restore** runs once at startup, after the plugin manager is up, in two
//! phases so a bad saved layout can never leave the live UI half rebuilt:
//!
//! 1. [`resolve`] walks the saved tree against a read-only snapshot of the live
//!    panels and produces a pure `Plan`. Live panels the app creates itself
//!    (level editor, agent chat, manual tool) are *claimed* by `panel_name`;
//!    panels that edit a file are recreated through the plugin manager exactly
//!    as `open_path` does; panels that declare a restore key are rebuilt by the
//!    restorer registered for its kind (`plugin_manager::panel_restorers`), see
//!    [`resolve`] for the full order. Files that no longer exist and groups left
//!    empty are dropped; a split left with one side collapses into it. Nothing
//!    is attached to the live UI.
//! 2. The plan is validated (the centre must hold a panel of the anchor group,
//!    i.e. the level editor, and each live panel is claimed at most once). A
//!    root that fails is discarded whole and its claims released; the live UI
//!    was never touched. Only then does [`commit`] install the plans: the anchor
//!    centre `TabPanel` is kept as that group (several panels and handlers hold
//!    that entity) and filled in place, side docks are set, and a live side-dock
//!    panel the saved layout does not mention is put back in its dock.
//!
//! **Dormant tabs.** A tab that names a panel (restore key or file) but cannot be
//! rebuilt because nothing is registered for it (its plugin is disabled or
//! missing) is not dropped: it is kept in `AppState::layout_dormant`, written
//! back under `dormant` on every save, and offered again on the next load
//! (see [`dormant`]). Disabling a plugin for one session therefore does not
//! erase its tabs. Ephemeral panels (`PanelState::ephemeral`) are skipped on
//! purpose and never kept.
//!
//! **Chrome and zoom.** The file manager drawer (docked, height) is saved in
//! `chrome` (see [`super::chrome`] for what is deliberately not saved). Those
//! changes emit no `DockEvent`, so they call `PulsarApp::chrome_changed`, which
//! arms the same debounce. The zoomed tab group is saved in `dock.zoomed`,
//! named by its active panel (not a runtime id); `DockArea::set_zoomed_in/out`
//! emit `LayoutChanged`. Commit applies it last, after the trees exist, and drops
//! it if no group matches.
//!
//! **Pop-outs.** Every live pop-out window opened from this window is saved
//! under `popouts` as `{ bounds, dock }`: its window geometry (same rule as the
//! main window, maximized pop-outs save the windowed size to return to) and the
//! whole tree of its own dock, since it may have been split or had tabs dropped
//! into it. `ui::dock::popout_events` (Opened / Changed / Closed) arm the same
//! debounce; the events fire inside window callbacks so they are handled on a
//! later turn, and events of pop-outs owned by another window are ignored. A
//! pop-out the user closed is no longer in `live_popouts` and so is absent from
//! the next save. On window close the final save runs in
//! `on_window_should_close`, while the pop-outs are still alive, and then sets
//! `layout_closing`: pop-outs closing along with the window cannot trigger a
//! save that would erase them.
//!
//! On restore each saved pop-out tree is one more root for the resolver (after
//! the main roots, so the main layout wins a contested panel). Pop-outs do not
//! claim live singleton panels; their tabs are rebuilt from restore keys and
//! files. Commit opens the windows last, after the main layout is installed, with
//! bounds checked against the connected displays (`window_geometry::fit_to_displays`:
//! a window whose display is gone opens windowed at the default size near the
//! primary display's corner). Closing a restored pop-out returns its tabs to the
//! centre anchor group of the main window. A pop-out that resolves to nothing is
//! dropped. Limitation: tabs inside a pop-out that nothing can rebuild (plugin
//! disabled) are not kept dormant, since the dormant list is per main root; they
//! are lost at the next save.
//!
//! `layout.json` is per-user (window geometry, open tabs), so new projects
//! gitignore it.
//!
//! Nothing is written until the restore has finished, so the default layout can
//! never overwrite a saved one.

mod commit;
mod dormant;
mod format;
mod resolve;
mod window_geometry;

use std::time::Duration;

use gpui::{AnyWindowHandle, AppContext as _, Context, Window, WindowBounds};
use ui::dock::{live_popouts, popout_events, DockEvent, PopoutEvent};

use super::PulsarApp;
use format::{layout_path, read_slot, write_slot, SavedPopout, SavedWindowLayout, PRIMARY_SLOT};
use window_geometry::SavedWindow;

pub(crate) use dormant::DormantLeaf;
pub(crate) use window_geometry::saved_window_bounds;

/// Quiet time after the last layout change before it is written.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(4);

// ── Save ─────────────────────────────────────────────────────────────────────

impl PulsarApp {
    /// Wire up layout persistence for this window. Only the primary project
    /// window persists; secondary windows share the project and would race it.
    pub(super) fn init_layout_persistence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.project_path.is_none() {
            return;
        }
        self.state.layout_persist = true;

        // Track the window's size and position; moving or resizing it saves the
        // layout like any other change (debounced).
        self.note_window_bounds(window.window_bounds());
        // A window that opened full screen never shows its windowed size, so
        // carry the saved one forward until the user leaves full screen.
        if self.state.window_restore_bounds.is_none() {
            if let Some(root) = self.state.project_path.as_deref() {
                self.state.window_restore_bounds = match saved_window_bounds(root) {
                    Some(
                        WindowBounds::Windowed(b)
                        | WindowBounds::Maximized(b)
                        | WindowBounds::Fullscreen(b),
                    ) => Some(b),
                    None => None,
                };
            }
        }
        cx.observe_window_bounds(window, |this, window, cx| {
            this.note_window_bounds(window.window_bounds());
            this.schedule_layout_save(cx);
        })
        .detach();

        let dock_area = self.state.dock_area.clone();
        cx.subscribe_in(
            &dock_area,
            window,
            |this, _, event: &DockEvent, _window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.schedule_layout_save(cx);
                }
            },
        )
        .detach();

        // Pop-outs opened from this window: opening, changing or closing one is
        // a layout change. Events fire inside the pop-out's window callbacks, so
        // they are handled on a later turn.
        let events = popout_events(cx);
        cx.subscribe(&events, |this, _, event: &PopoutEvent, cx| {
            this.on_popout_event(*event, cx);
        })
        .detach();

        // Flush a pending change when the window closes.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let bounds = window.window_bounds();
            _ = this.update(cx, |app, cx| {
                app.note_window_bounds(bounds);
                // Pop-outs are still alive here, so this captures them. Once
                // it is written they close with the window, and nothing may
                // save over it.
                app.save_layout_now(cx);
                app.state.layout_closing = true;
                app.state.layout_save_task = None;
            });
            true
        });

        // Restore once construction (plugin manager included) has finished.
        let this = cx.entity();
        window.defer(cx, move |window, cx| {
            this.update(cx, |app, cx| {
                app.restore_layout(window, cx);
                app.state.layout_ready = true;
            });
        });
    }

    /// Record the window's current geometry, remembering the last windowed
    /// bounds as the size to return to from maximized / full screen.
    fn note_window_bounds(&mut self, bounds: WindowBounds) {
        if let WindowBounds::Windowed(windowed) = bounds {
            self.state.window_restore_bounds = Some(windowed);
        }
        self.state.window_bounds = Some(bounds);
    }

    /// React to a pop-out event. Only pop-outs opened from this window count.
    fn on_popout_event(&mut self, event: PopoutEvent, cx: &mut Context<Self>) {
        match event {
            PopoutEvent::Opened { window } | PopoutEvent::Changed { window } => {
                cx.spawn(async move |this, cx| {
                    _ = this.update(cx, |app, cx| app.note_popout(window, cx));
                })
                .detach();
            }
            PopoutEvent::Closed { window } => {
                // The pop-out is already gone from `live_popouts`, so the next
                // save leaves it out.
                if self.state.layout_popouts.remove(&window).is_some() {
                    self.schedule_layout_save(cx);
                }
            }
        }
    }

    /// Remember a pop-out of this window (and the windowed size it returns to
    /// from maximized), then save.
    fn note_popout(&mut self, window: AnyWindowHandle, cx: &mut Context<Self>) {
        let Some(me) = self.state.layout_window else {
            return;
        };
        let ours = live_popouts(cx)
            .iter()
            .any(|p| p.window == window && p.parent == Some(me));
        if !ours {
            return;
        }
        let windowed = match cx.update_window(window, |_, w, _| w.window_bounds()) {
            Ok(WindowBounds::Windowed(b)) => Some(b),
            _ => None,
        };
        let entry = self.state.layout_popouts.entry(window).or_default();
        if windowed.is_some() {
            *entry = windowed;
        }
        self.schedule_layout_save(cx);
    }

    /// Every live pop-out opened from this window, as saved.
    fn capture_popouts(&self, cx: &mut Context<Self>) -> Vec<SavedPopout> {
        let Some(me) = self.state.layout_window else {
            return Vec::new();
        };
        let mut saved = Vec::new();
        for info in live_popouts(cx) {
            if info.parent != Some(me) {
                continue;
            }
            let Some(dock) = info.dock_area.upgrade() else {
                continue;
            };
            let remembered = self.state.layout_popouts.get(&info.window).copied().flatten();
            let bounds = match cx.update_window(info.window, |_, w, _| w.window_bounds()) {
                Ok(bounds) => bounds,
                // Mid-update (its own callback is running): fall back to the
                // last bounds seen rather than losing the pop-out.
                Err(_) => match remembered {
                    Some(b) => WindowBounds::Windowed(b),
                    None => continue,
                },
            };
            let tree = dock.read(cx).dump(cx).center;
            if tree.children.is_empty() {
                continue;
            }
            saved.push(SavedPopout {
                bounds: SavedWindow::capture(bounds, remembered),
                dock: tree,
            });
        }
        saved
    }

    /// (Re)arm the debounce timer. Dropping the previous task cancels it.
    pub(super) fn schedule_layout_save(&mut self, cx: &mut Context<Self>) {
        if !self.state.layout_persist || !self.state.layout_ready || self.state.layout_closing {
            return;
        }
        self.state.layout_save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DEBOUNCE).await;
            _ = this.update(cx, |app, cx| {
                app.state.layout_save_task = None;
                app.save_layout_now(cx);
            });
        }));
    }

    /// Write the current layout now, cancelling any pending timer.
    fn save_layout_now(&mut self, cx: &mut Context<Self>) {
        if !self.state.layout_persist || !self.state.layout_ready || self.state.layout_closing {
            return;
        }
        self.state.layout_save_task = None;
        let Some(root) = self.state.project_path.clone() else {
            return;
        };

        let mut layout = SavedWindowLayout {
            dock: self.state.dock_area.read(cx).dump(cx),
            window: self
                .state
                .window_bounds
                .map(|b| SavedWindow::capture(b, self.state.window_restore_bounds)),
            sidebar: self.state.nav_sidebar.read(cx).model.save(&root),
            chrome: self.state.chrome.save(),
            // Tabs no plugin could build this session ride along unchanged.
            popouts: self.capture_popouts(cx),
            dormant: dormant::prune(self.state.layout_dormant.clone(), |p| p.exists()),
            ..Default::default()
        };
        layout.relativize(&root);

        if let Err(error) = write_slot(&layout_path(&root), PRIMARY_SLOT, &layout) {
            tracing::warn!("could not save the editor layout: {error}");
        }
    }
}

// ── Restore ──────────────────────────────────────────────────────────────────

impl PulsarApp {
    fn restore_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.state.project_path.clone() else {
            return;
        };
        let Some(mut layout) = read_slot(&layout_path(&root), PRIMARY_SLOT) else {
            return;
        };
        layout.absolutize(&root);
        // Offer tabs shelved last time again; they resolve with everything else.
        layout.reinsert_dormant();
        // Chrome is plain flags; applying it cannot fail (heights are clamped).
        self.state.chrome.restore(&layout.chrome);
        let sidebar = std::mem::take(&mut layout.sidebar);
        self.state.nav_sidebar.update(cx, |nav, cx| {
            nav.model.restore(sidebar, &root);
            cx.notify();
        });

        let dock_area = self.state.dock_area.clone();
        let anchor = self.state.center_tabs.clone();

        // Phase 1: resolve against a read-only snapshot; nothing live changes.
        let snapshot = resolve::LiveSnapshot::capture(&dock_area, &anchor, cx);
        let resolved = resolve::resolve_layout(&layout, &snapshot, &root, &dock_area, window, cx);
        // Whatever is still unbuildable is shelved again (and so saved again).
        self.state.layout_dormant = resolved.dormant.clone();

        // Phase 2: attach.
        let popouts = commit::commit(
            resolved,
            &dock_area,
            &anchor,
            &snapshot.anchor_panels,
            window,
            cx,
        );
        // Seed the windowed size a restored maximized pop-out returns to.
        for (handle, restore) in popouts {
            self.state.layout_popouts.insert(handle, restore);
        }

        self.refresh_open_editor_snapshot(cx);
        cx.notify();
    }
}
