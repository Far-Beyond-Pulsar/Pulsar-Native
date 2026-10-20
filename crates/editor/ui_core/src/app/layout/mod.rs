//! Saving and restoring the editor layout and open tabs, per project.
//!
//! **What is saved.** The dock framework's own `DockAreaState`: the split tree
//! with its sizes, every tab group with its active tab, and each side dock's
//! size / open state; plus the window geometry and the sidebar. A tab that edits
//! a file records that file (the `TabPanel` dump does this for any panel with a
//! `panel_file_path`). Paths inside the project are stored relative to it. The
//! record lives at `<project>/.pulsar/layout.json`, a map of window slots (see
//! [`format`]); this window reads and writes its own slot and leaves the others alone.
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
//! **Owned panels.** An editor with a dock of its own (the Blueprint editor's graph
//! canvas, the level editor's hierarchy...) owns inner panels the user can pop
//! out. They carry an owner marker (`PanelState::with_owner`; a panel declares it
//! with `Panel::owner_file`) instead of a file, are looked up among the live
//! panels of the owner editor on restore and moved out of its dock instead of
//! being created; see [`owned`].
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
//! under `popouts` as `{ id, bounds, dock, zoomed }`: its window geometry (same rule as the
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
//! dropped. The zoomed group of a pop-out is saved in its `zoomed` and applied
//! once its window is open (same identity rule as the main window). A pop-out
//! restored on a display that is gone opens windowed near the primary display;
//! if it was maximized it comes back windowed, not maximized (accepted).
//!
//! Tabs inside a pop-out that nothing can rebuild (plugin disabled) are kept
//! dormant like the main window's, under the root `DormantRoot::Popout(id)`.
//! `id` is the stable `SavedPopout::id` (positions shift when one closes). A
//! pop-out whose tabs are *all* dormant opens no window; it stays in the file as
//! an empty pop-out (a "ghost") carrying its id and bounds so its tabs have
//! somewhere to return to, and goes once no dormant tab refers to it. Closing a
//! pop-out drops its dormant tabs; a save drops dormant tabs of pop-outs it does
//! not contain.
//!
//! **Windows and slots.** Every editor window of a project owns one slot of
//! `layout.json` (see [`format`], [`store`]). The slot is claimed when the
//! window is built: the lowest ordinal not held by a live window of the
//! project, so the primary window is slot 0. Each window saves and restores
//! only its slot; a window whose slot has no record keeps its defaults (and a
//! centre without a level-editor tab is valid when the window has none: the
//! resolver requires the anchor group only if the window has one). Writes are
//! read-modify-write under one in-process lock, so windows never clobber each
//! other; two engine *processes* on one project are last-writer-wins per slot.
//!
//! *Closing.* When a window is gone its slot is released
//! ([`store::LayoutStore::release_slot`]). A secondary window closed on its own
//! while others of the project remain drops its slot. Closing the primary
//! window, the last window, or any window once the app is quitting
//! (`on_app_quit`) keeps every slot. The close itself still goes through
//! `on_window_should_close` (final save, then `layout_closing` so pop-outs that
//! close with the window cannot overwrite it); the slot is released afterwards,
//! on entity release, so a close that is vetoed never loses a slot.
//!
//! *Startup.* After the primary window (slot 0) has restored, it reopens the
//! other recorded slots once per process ([`PulsarApp::reopen_recorded_windows`]):
//! each is opened through `PulsarRoot::open_additional` (the normal project
//! window path without the one-window-per-kind focus dedup) with its slot
//! reserved, so it restores its own slot and opens with that slot's window
//! geometry (`saved_window_bounds`, validated against the connected displays
//! when the window is created). A slot whose window cannot be created stays
//! recorded and is skipped; reopened windows are non-primary so they cannot
//! reopen anything themselves.
//!
//! Note: nothing in the editor creates a second window for the *same* project
//! yet (`PulsarRoot::open` focuses the existing editor window), so slots other
//! than 0 appear only once such a feature calls `open_additional`.
//!
//! `layout.json` is per-user (window geometry, open tabs), so new projects
//! gitignore it.
//!
//! Nothing is written until the restore has finished, so the default layout can
//! never overwrite a saved one.

mod commit;
mod dormant;
mod format;
mod owned;
mod resolve;
mod store;
mod window_geometry;

use std::time::Duration;

use gpui::{AnyWindowHandle, AppContext as _, Context, Window, WindowBounds};
use ui::dock::{live_popouts, popout_events, DockEvent, PopoutEvent};

use super::PulsarApp;
use format::SavedWindowLayout;
use store::{LayoutStore, MAX_REOPENED, PRIMARY};
use window_geometry::SavedWindow;

pub(crate) use dormant::DormantLeaf;
pub(crate) use format::SavedPopout;
pub(crate) use window_geometry::{saved_window_bounds, saved_window_bounds_for};

/// What a window remembers about one of its pop-outs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PopoutNote {
    /// The pop-out's id in the saved layout (see [`SavedPopout::id`]).
    pub id: u32,
    /// The last windowed bounds, which a maximized pop-out returns to.
    pub restore: Option<gpui::Bounds<gpui::Pixels>>,
}

/// Quiet time after the last layout change before it is written.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(4);

// ── Save ─────────────────────────────────────────────────────────────────────

impl PulsarApp {
    /// Wire up layout persistence for this window: claim its slot in the
    /// project's layout file and save / restore that slot. Every project window
    /// persists, each into its own slot (see [`store`]).
    pub(super) fn init_layout_persistence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.state.project_path.clone() else {
            return;
        };
        self.state.layout_persist = true;
        // Pop-outs are matched to the window that opened them by this handle.
        self.state.layout_window = Some(window.window_handle());
        let slot = LayoutStore::global().claim_slot(&root);
        self.state.layout_slot = Some(slot);

        // The app quitting is learnt before any window is torn down, so windows
        // that close as part of it keep their slots.
        cx.on_app_quit(|app, cx| {
            LayoutStore::global().mark_quitting();
            // Last chance to write while the windows are still open.
            app.flush_layout_for_close(cx);
            async {}
        })
        .detach();
        // The window is gone (closed, or removed without a close request): give
        // the slot back; a window closed on its own drops it from the file.
        cx.on_release({
            let root = root.clone();
            move |app, _| {
                app.state.layout_closing = true;
                app.state.layout_save_task = None;
                LayoutStore::global().release_slot(&root, slot);
            }
        })
        .detach();

        // Track the window's size and position; moving or resizing it saves the
        // layout like any other change (debounced).
        self.note_window_bounds(window.window_bounds());
        // A window that opened full screen never shows its windowed size, so
        // carry the saved one forward until the user leaves full screen.
        if self.state.window_restore_bounds.is_none() {
            self.state.window_restore_bounds = match saved_window_bounds_for(&root, slot) {
                Some(
                    WindowBounds::Windowed(b)
                    | WindowBounds::Maximized(b)
                    | WindowBounds::Fullscreen(b),
                ) => Some(b),
                None => None,
            };
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
        window.on_window_should_close(cx, {
            let this = this.clone();
            move |window, cx| {
                let bounds = window.window_bounds();
                _ = this.update(cx, |app, cx| {
                    app.note_window_bounds(bounds);
                    app.flush_layout_for_close(cx);
                });
                true
            }
        });
        // The title-bar close button removes the window without a close request,
        // so it runs this hook instead.
        ui_common::close_hooks::on_before_close(
            window,
            move |window, cx| {
                let bounds = window.window_bounds();
                _ = this.update(cx, |app, cx| {
                    app.note_window_bounds(bounds);
                    app.flush_layout_for_close(cx);
                });
            },
            cx,
        );

        // Restore once construction (plugin manager included) has finished.
        let this = cx.entity();
        // Let the application paint its initial frame before resolving saved
        // tabs. Editor providers may still need to construct UI entities on
        // this thread, but that work must not prevent the user from seeing the
        // editor window at all.
        window.on_next_frame(move |window, cx| {
            this.update(cx, |app, cx| {
                app.restore_layout(window, cx);
                app.state.layout_ready = true;
                cx.notify();
                // Only the primary window brings the others back.
                if app.state.layout_slot == Some(PRIMARY) {
                    app.reopen_recorded_windows(cx);
                }
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
                if let Some(note) = self.state.layout_popouts.remove(&window) {
                    // Its dormant tabs go with it, so a new pop-out reusing the
                    // id does not inherit them.
                    self.state
                        .layout_dormant
                        .retain(|leaf| leaf.root != dormant::DormantRoot::Popout(note.id));
                    self.state.layout_ghosts.retain(|g| g.id != note.id);
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
        let note = self.popout_note(window);
        if windowed.is_some() {
            note.restore = windowed;
        }
        self.schedule_layout_save(cx);
    }

    /// The note for pop-out `window`, giving it the lowest unused id if new.
    fn popout_note(&mut self, window: AnyWindowHandle) -> &mut PopoutNote {
        if !self.state.layout_popouts.contains_key(&window) {
            let taken: Vec<u32> = self
                .state
                .layout_popouts
                .values()
                .map(|note| note.id)
                .chain(self.state.layout_ghosts.iter().map(|g| g.id))
                .chain(
                    self.state
                        .layout_dormant
                        .iter()
                        .filter_map(|leaf| match leaf.root {
                            dormant::DormantRoot::Popout(id) => Some(id),
                            _ => None,
                        }),
                )
                .collect();
            let id = (0..).find(|id| !taken.contains(id)).unwrap_or(u32::MAX);
            self.state
                .layout_popouts
                .insert(window, PopoutNote { id, restore: None });
        }
        self.state
            .layout_popouts
            .get_mut(&window)
            .expect("inserted above")
    }

    /// Every live pop-out opened from this window, as saved, each with the
    /// entity id of its dock area (to match its owned panels, see [`owned`]).
    fn capture_popouts(&mut self, cx: &mut Context<Self>) -> Vec<(SavedPopout, gpui::EntityId)> {
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
            let note = *self.popout_note(info.window);
            let remembered = note.restore;
            let bounds = match cx.update_window(info.window, |_, w, _| w.window_bounds()) {
                Ok(bounds) => bounds,
                // Mid-update (its own callback is running): fall back to the
                // last bounds seen rather than losing the pop-out.
                Err(_) => match remembered {
                    Some(b) => WindowBounds::Windowed(b),
                    None => continue,
                },
            };
            let dump = dock.read(cx).dump(cx);
            let tree = dump.center;
            if tree.children.is_empty() {
                continue;
            }
            saved.push((
                SavedPopout {
                    id: note.id,
                    bounds: SavedWindow::capture(bounds, remembered),
                    dock: tree,
                    zoomed: dump.zoomed,
                },
                dock.entity_id(),
            ));
        }
        saved
    }

    /// Write the layout one last time, while the window and its pop-outs are
    /// still alive, then stop saving: pop-outs closing along with the window
    /// must not save over it.
    fn flush_layout_for_close(&mut self, cx: &mut Context<Self>) {
        if self.state.layout_closing {
            return;
        }
        self.save_layout_now(cx);
        self.state.layout_closing = true;
        self.state.layout_save_task = None;
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

        let captured = self.capture_popouts(cx);
        let popout_areas: Vec<(gpui::EntityId, u32)> =
            captured.iter().map(|(p, area)| (*area, p.id)).collect();
        let popouts: Vec<SavedPopout> = captured.into_iter().map(|(p, _)| p).collect();
        // Tabs no plugin could build this session ride along unchanged, kept
        // only for pop-outs that are part of this save.
        let mut dormant = dormant::prune(self.state.layout_dormant.clone(), |p| p.exists());
        let ghosts: Vec<SavedPopout> = self
            .state
            .layout_ghosts
            .iter()
            .filter(|g| {
                dormant
                    .iter()
                    .any(|leaf| leaf.root == dormant::DormantRoot::Popout(g.id))
            })
            .cloned()
            .collect();
        let saved_ids: Vec<u32> = popouts.iter().chain(&ghosts).map(|p| p.id).collect();
        dormant::drop_orphaned(&mut dormant, &saved_ids);
        let mut popouts = popouts;
        popouts.extend(ghosts);

        let mut layout = SavedWindowLayout {
            dock: self.state.dock_area.read(cx).dump(cx),
            window: self
                .state
                .window_bounds
                .map(|b| SavedWindow::capture(b, self.state.window_restore_bounds)),
            sidebar: self.state.nav_sidebar.read(cx).model.save(&root),
            chrome: {
                let mut chrome = self.state.chrome.save();
                chrome.file_manager_folder = self
                    .state
                    .chrome
                    .file_manager_folder
                    .as_deref()
                    .and_then(|folder| folder.strip_prefix(&root).ok())
                    .filter(|relative| !relative.as_os_str().is_empty())
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"));
                chrome
            },
            popouts,
            dormant,
            ..Default::default()
        };
        // Owned panels (an editor's inner panels) get the rank that lets a restore
        // pick the same one again.
        let ranks = owned::ranks_by_root(
            &owned::live_owned(cx),
            self.state.dock_area.entity_id(),
            &popout_areas,
        );
        layout.assign_owner_nth(&ranks);
        layout.relativize(&root);

        let Some(slot) = self.state.layout_slot else {
            return;
        };
        tracing::info!(
            slot,
            popouts = layout.popouts.len(),
            left_open = layout.dock.left_dock.as_ref().map(|d| d.is_open()),
            "layout: saving"
        );
        if let Err(error) = LayoutStore::global().save_slot(&root, slot, &layout) {
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
        let Some(slot) = self.state.layout_slot else {
            return;
        };
        // A window with no recorded slot (a new secondary) keeps its defaults.
        let Some(mut layout) = LayoutStore::global().read_slot(&root, slot) else {
            return;
        };
        layout.absolutize(&root);
        // Offer tabs shelved last time again; they resolve with everything else.
        layout.reinsert_dormant();
        // Chrome is plain flags; applying it cannot fail (heights are clamped).
        self.state.chrome.restore(&layout.chrome);
        // The drawer reopens in the folder the user left it in, if it is
        // still there.
        if let Some(folder) = layout
            .chrome
            .file_manager_folder
            .as_deref()
            .map(|relative| root.join(relative))
            .filter(|folder| folder.is_dir())
        {
            self.state
                .file_manager_drawer
                .update(cx, |drawer, cx| drawer.show_folder(folder, cx));
        }
        let sidebar = std::mem::take(&mut layout.sidebar);
        self.state.nav_sidebar.update(cx, |nav, cx| {
            nav.model.restore(sidebar, &root);
            cx.notify();
        });

        let dock_area = self.state.dock_area.clone();
        let anchor = self.state.center_tabs.clone();

        tracing::info!(
            slot,
            popouts = layout.popouts.len(),
            left_open = layout.dock.left_dock.as_ref().map(|d| d.is_open()),
            "layout: restoring"
        );
        // Phase 1: resolve against a read-only snapshot; nothing live changes.
        let snapshot = resolve::LiveSnapshot::capture(&dock_area, &anchor, cx);
        let resolved = resolve::resolve_layout(&layout, &snapshot, &root, &dock_area, window, cx);
        // Whatever is still unbuildable is shelved again (and so saved again).
        self.state.layout_dormant = resolved.dormant.clone();
        self.state.layout_ghosts = resolved.ghost_popouts.clone();

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
        for (handle, id, restore) in popouts {
            self.state
                .layout_popouts
                .insert(handle, PopoutNote { id, restore });
        }

        self.refresh_open_editor_snapshot(cx);
        cx.notify();
    }
}

// ── Other windows ────────────────────────────────────────────────────────────

impl PulsarApp {
    /// Reopen the other windows recorded for this project, each through the
    /// ordinary project-window path so it restores its own slot (and its own
    /// geometry, via `saved_window_bounds`). Runs once per process and project,
    /// from the primary window only; a reopened window has a non-primary slot
    /// so it can never reopen anything itself. A slot whose window fails to
    /// open stays recorded and is simply skipped.
    fn reopen_recorded_windows(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.state.project_path.clone() else {
            return;
        };
        let store = LayoutStore::global();
        if !store.begin_reopen(&root) {
            return;
        }
        let slots: Vec<u32> = store
            .slots(&root)
            .into_iter()
            .filter(|slot| *slot != PRIMARY && !store.is_live(&root, *slot))
            .take(MAX_REOPENED)
            .collect();
        if slots.is_empty() {
            return;
        }
        // Not inside this window's update: opening a window runs a whole
        // editor's construction.
        cx.defer(move |cx| {
            let store = LayoutStore::global();
            for slot in slots {
                if store.is_live(&root, slot) {
                    continue;
                }
                store.reserve_slot(&root, slot);
                crate::PulsarRoot::open_additional(root.clone(), cx);
                // Construction is synchronous: either the window claimed the
                // reservation or it never came up.
                if !store.is_live(&root, slot) {
                    tracing::warn!("layout: could not reopen window slot {slot}");
                }
                store.clear_reservations(&root);
            }
        });
    }
}
