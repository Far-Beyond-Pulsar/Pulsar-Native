//! Saving and restoring the dock layout and open tabs, per project.
//!
//! **What is saved.** The dock framework's own [`DockAreaState`]: the split
//! tree with its sizes, every tab group with its active tab, and each side
//! dock's size / open state. A tab that edits a file records that file (the
//! `TabPanel` dump does this for any panel with a `panel_file_path`). Paths
//! inside the project are stored relative to it. The record lives at
//! `<project>/.pulsar/layout.json`.
//!
//! **When.** Any `DockEvent::LayoutChanged` (tab opened / closed / moved /
//! activated, split or dock resized, dock toggled) arms a 4 second timer; a
//! further change before it fires re-arms it, so a drag that resizes
//! continuously is written once, 4 s after the last movement. The layout is
//! also flushed when the window closes.
//!
//! **Restore.** Runs once at startup, after the plugin manager is up. The saved
//! tree is rebuilt around what already exists rather than replacing it:
//!
//! - Live panels the app creates itself (level editor, agent chat, manual tool)
//!   are reused, found by `panel_name`.
//! - Panels that edit a file are recreated through the plugin manager, exactly
//!   as `open_path` does. Files that no longer exist, and groups left empty,
//!   are dropped; a split left with one side collapses into it.
//! - The centre `TabPanel` that holds the level editor is kept as that group,
//!   because several panels and handlers hold that entity.
//! - A live side-dock panel the saved layout does not mention (for example one
//!   added by an update) is put back in its dock rather than lost.
//!
//! Nothing is written until the restore has finished, so the default layout can
//! never overwrite a saved one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{Bounds, Context, Entity, EntityId, Pixels, Window, WindowBounds};
use serde::{Deserialize, Serialize};
use ui::dock::{
    DockAreaState, DockEvent, DockItem, DockPlacement, DockState, PanelInfo, PanelState, PanelView,
    TabPanel,
};

use super::PulsarApp;
use super::nav_sidebar::model::SavedSidebar;

/// Bump when the format changes incompatibly; older files are then ignored.
const LAYOUT_VERSION: u32 = 1;
const LAYOUT_FILE: &str = "layout.json";
/// Quiet time after the last layout change before it is written.
const SAVE_DEBOUNCE: Duration = Duration::from_secs(4);

#[derive(Serialize, Deserialize)]
struct SavedLayout {
    version: u32,
    dock: DockAreaState,
    /// Window size, position and maximized / fullscreen state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    window: Option<SavedWindow>,
    /// Pinned tabs, collapsed groups and expanded folders of the left sidebar.
    #[serde(default, skip_serializing_if = "SavedSidebar::is_empty")]
    sidebar: SavedSidebar,
}

/// How the window was shown. The bounds saved alongside are the *restore*
/// bounds, so a maximized window comes back maximized and un-maximizes to its
/// previous size.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
enum SavedWindowState {
    Windowed,
    Maximized,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct SavedWindow {
    state: SavedWindowState,
    bounds: Bounds<Pixels>,
}

impl SavedWindow {
    /// Capture `bounds`. For a maximized or fullscreen window the platform
    /// reports the screen-sized bounds, so `restore` (the last windowed
    /// bounds) is saved instead; leaving full screen then returns to the size
    /// the window had before.
    fn capture(bounds: WindowBounds, restore: Option<Bounds<Pixels>>) -> Self {
        let (state, bounds) = match bounds {
            WindowBounds::Windowed(b) => (SavedWindowState::Windowed, b),
            WindowBounds::Maximized(b) => (SavedWindowState::Maximized, restore.unwrap_or(b)),
            WindowBounds::Fullscreen(b) => (SavedWindowState::Fullscreen, restore.unwrap_or(b)),
        };
        Self { state, bounds }
    }

    /// The bounds to open a window with. A windowed window needs a plausible
    /// saved size, else `None` (use the default). A maximized or fullscreen
    /// window does not care about its size, so it is reopened that way even if
    /// the saved size is unusable; the bounds then only say which display and
    /// what size to leave it at.
    fn to_window_bounds(self) -> Option<WindowBounds> {
        let size = self.bounds.size;
        let usable = |v: Pixels| f32::from(v).is_finite() && f32::from(v) >= MIN_WINDOW_SIDE;
        let sized = usable(size.width) && usable(size.height);
        match self.state {
            SavedWindowState::Windowed => sized.then_some(WindowBounds::Windowed(self.bounds)),
            SavedWindowState::Maximized => {
                Some(WindowBounds::Maximized(self.restore_bounds(sized)))
            }
            SavedWindowState::Fullscreen => {
                Some(WindowBounds::Fullscreen(self.restore_bounds(sized)))
            }
        }
    }

    fn restore_bounds(self, sized: bool) -> Bounds<Pixels> {
        if sized {
            self.bounds
        } else {
            Bounds {
                origin: gpui::point(gpui::px(50.), gpui::px(50.)),
                size: gpui::size(gpui::px(1600.), gpui::px(900.)),
            }
        }
    }
}

/// Smallest saved window side we trust, in logical pixels.
const MIN_WINDOW_SIDE: f32 = 320.0;

/// The window geometry saved for `project_root`, to open its window with.
/// Whether it is still on a connected display is checked when the window is
/// opened.
pub(crate) fn saved_window_bounds(project_root: &Path) -> Option<WindowBounds> {
    read_layout(&layout_path(project_root))?
        .window?
        .to_window_bounds()
}

fn layout_path(project_root: &Path) -> PathBuf {
    project_root.join(".pulsar").join(LAYOUT_FILE)
}

/// Rewrite recorded file paths inside the project as project-relative.
fn relativize(state: &mut PanelState, root: &Path) {
    if let Some(value) = state.file_mut() {
        if let Some(path) = value.as_str().map(PathBuf::from) {
            if let Ok(relative) = path.strip_prefix(root) {
                *value = relative.to_string_lossy().replace('\\', "/").into();
            }
        }
    }
    for child in &mut state.children {
        relativize(child, root);
    }
}

/// Inverse of [`relativize`]: make recorded paths absolute under `root`.
fn absolutize(state: &mut PanelState, root: &Path) {
    if let Some(value) = state.file_mut() {
        if let Some(path) = value.as_str().map(PathBuf::from) {
            if path.is_relative() {
                *value = root.join(path).to_string_lossy().into_owned().into();
            }
        }
    }
    for child in &mut state.children {
        absolutize(child, root);
    }
}

fn map_dock_panels(dock: &mut Option<DockState>, f: impl Fn(&mut PanelState)) {
    if let Some(dock) = dock {
        // `DockState` exposes its tree read-only; round-trip through serde to
        // rewrite it, which also keeps this independent of its field layout.
        if let Ok(mut value) = serde_json::to_value(&*dock) {
            if let Some(panel) = value.get_mut("panel") {
                if let Ok(mut state) = serde_json::from_value::<PanelState>(panel.take()) {
                    f(&mut state);
                    if let Ok(v) = serde_json::to_value(&state) {
                        *panel = v;
                    }
                }
            }
            if let Ok(updated) = serde_json::from_value::<DockState>(value) {
                *dock = updated;
            }
        }
    }
}

fn transform_layout(layout: &mut SavedLayout, f: impl Fn(&mut PanelState) + Copy) {
    f(&mut layout.dock.center);
    map_dock_panels(&mut layout.dock.left_dock, f);
    map_dock_panels(&mut layout.dock.right_dock, f);
    map_dock_panels(&mut layout.dock.bottom_dock, f);
}

fn write_layout(path: &Path, layout: &SavedLayout) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(layout)?;
    // Write beside the target and rename, so a crash mid-write cannot leave a
    // truncated layout behind.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)
}

fn read_layout(path: &Path) -> Option<SavedLayout> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str::<SavedLayout>(&text) {
        Ok(layout) if layout.version == LAYOUT_VERSION => Some(layout),
        Ok(layout) => {
            tracing::info!(
                found = layout.version,
                expected = LAYOUT_VERSION,
                "ignoring saved layout of another version"
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                "ignoring unreadable saved layout {}: {error}",
                path.display()
            );
            None
        }
    }
}

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

        // Flush a pending change when the window closes.
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            let bounds = window.window_bounds();
            _ = this.update(cx, |app, cx| {
                app.note_window_bounds(bounds);
                app.save_layout_now(cx);
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

    /// (Re)arm the debounce timer. Dropping the previous task cancels it.
    pub(super) fn schedule_layout_save(&mut self, cx: &mut Context<Self>) {
        if !self.state.layout_persist || !self.state.layout_ready {
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
        if !self.state.layout_persist || !self.state.layout_ready {
            return;
        }
        self.state.layout_save_task = None;
        let Some(root) = self.state.project_path.clone() else {
            return;
        };

        let mut layout = SavedLayout {
            version: LAYOUT_VERSION,
            dock: self.state.dock_area.read(cx).dump(cx),
            window: self
                .state
                .window_bounds
                .map(|b| SavedWindow::capture(b, self.state.window_restore_bounds)),
            sidebar: self.state.nav_sidebar.read(cx).model.save(&root),
        };
        transform_layout(&mut layout, |state| relativize(state, &root));

        if let Err(error) = write_layout(&layout_path(&root), &layout) {
            tracing::warn!("could not save the editor layout: {error}");
        }
    }
}

// ── Restore ──────────────────────────────────────────────────────────────────

/// A live panel available for reuse, and where it lives now.
struct LivePanel {
    panel: Arc<dyn PanelView>,
    placement: DockPlacement,
}

struct Restorer {
    project_root: PathBuf,
    live: Vec<LivePanel>,
    /// Entity ids of the panels now in the anchor tab group.
    anchor_panels: HashSet<EntityId>,
    /// The centre tab group to keep. Taken by the first group that holds one of
    /// its panels.
    anchor: Option<Entity<TabPanel>>,
    dock_area: gpui::WeakEntity<ui::dock::DockArea>,
}

impl Restorer {
    /// The panel a saved tab stands for, reusing a live one where possible.
    fn resolve_leaf(
        &mut self,
        leaf: &PanelState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Option<Arc<dyn PanelView>> {
        if let Some(ix) = self
            .live
            .iter()
            .position(|live| live.panel.panel_name(cx) == leaf.panel_name)
        {
            return Some(self.live.remove(ix).panel);
        }

        let path = PathBuf::from(leaf.file()?);
        if !path.exists() {
            tracing::info!("layout: skipping {} (no longer exists)", path.display());
            return None;
        }
        let manager = plugin_manager::global()?;
        let mut manager = manager.write();
        manager.set_project_root(Some(self.project_root.clone()));
        super::refresh_plugin_editor_settings(&mut manager);
        match manager.create_editor_for_file(&path, window, cx) {
            Ok(panel) => Some(panel),
            Err(error) => {
                tracing::warn!("layout: could not reopen {}: {error}", path.display());
                None
            }
        }
    }

    fn build_item(
        &mut self,
        state: &PanelState,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> Option<DockItem> {
        match &state.info {
            PanelInfo::Stack { sizes, .. } => {
                let axis = state.info.axis()?;
                let mut items = Vec::new();
                let mut kept_sizes: Vec<Option<Pixels>> = Vec::new();
                let mut dropped = false;
                for (ix, child) in state.children.iter().enumerate() {
                    match self.build_item(child, window, cx) {
                        Some(item) => {
                            items.push(item);
                            kept_sizes.push(sizes.get(ix).copied());
                        }
                        None => dropped = true,
                    }
                }
                match items.len() {
                    0 => None,
                    1 => items.pop(),
                    n => {
                        // Saved sizes no longer add up once a side is gone.
                        let sizes = if dropped { vec![None; n] } else { kept_sizes };
                        Some(DockItem::split_with_sizes(
                            axis,
                            items,
                            sizes,
                            &self.dock_area,
                            window,
                            cx,
                        ))
                    }
                }
            }
            PanelInfo::Tabs { active_index } => {
                let mut panels: Vec<Arc<dyn PanelView>> = Vec::new();
                let mut active = None;
                for (ix, child) in state.children.iter().enumerate() {
                    if let Some(panel) = self.resolve_leaf(child, window, cx) {
                        if ix == *active_index {
                            active = Some(panels.len());
                        }
                        panels.push(panel);
                    }
                }
                if panels.is_empty() {
                    return None;
                }
                let active = active.unwrap_or(0);

                let holds_anchor_panel = panels
                    .iter()
                    .any(|p| self.anchor_panels.contains(&p.view().entity_id()));
                if holds_anchor_panel {
                    if let Some(anchor) = self.anchor.take() {
                        return Some(Self::fill_anchor(anchor, &panels, active, window, cx));
                    }
                }
                Some(DockItem::tabs(
                    panels,
                    Some(active),
                    &self.dock_area,
                    window,
                    cx,
                ))
            }
            PanelInfo::Tiles { metas } => {
                // Free-floating tab groups, each with its saved bounds and
                // stacking order. A group that cannot be rebuilt takes its
                // bounds with it.
                let mut items = Vec::new();
                let mut kept = Vec::new();
                for (ix, child) in state.children.iter().enumerate() {
                    let Some(meta) = metas.get(ix).copied() else {
                        continue;
                    };
                    if let Some(item @ DockItem::Tabs { .. }) = self.build_item(child, window, cx) {
                        items.push(item);
                        kept.push(meta);
                    }
                }
                if items.is_empty() {
                    return None;
                }
                Some(DockItem::tiles(items, kept, &self.dock_area, window, cx))
            }
            // A bare panel outside any tab group is not part of this app's layouts.
            PanelInfo::Panel(_) => None,
        }
    }

    /// Put `panels` into the existing `anchor` tab group in saved order. The
    /// panels already in it are skipped by `insert_panel_at`, so inserting each
    /// at its saved index leaves the group in exactly the saved order.
    fn fill_anchor(
        anchor: Entity<TabPanel>,
        panels: &[Arc<dyn PanelView>],
        active: usize,
        window: &mut Window,
        cx: &mut gpui::App,
    ) -> DockItem {
        let items = anchor.update(cx, |tabs, cx| {
            for (ix, panel) in panels.iter().enumerate() {
                let at = ix.min(tabs.all_panels().len());
                tabs.insert_panel_at(panel.clone(), at, window, cx);
            }
            tabs.set_active_tab(active, window, cx);
            tabs.all_panels()
        });
        let active_ix = anchor
            .read(cx)
            .active_tab_index()
            .unwrap_or(active.min(items.len().saturating_sub(1)));
        DockItem::Tabs {
            items,
            active_ix,
            view: anchor,
        }
    }
}

impl PulsarApp {
    fn restore_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.state.project_path.clone() else {
            return;
        };
        let Some(mut layout) = read_layout(&layout_path(&root)) else {
            return;
        };
        transform_layout(&mut layout, |state| absolutize(state, &root));
        let sidebar = std::mem::take(&mut layout.sidebar);
        self.state.nav_sidebar.update(cx, |nav, cx| {
            nav.model.restore(sidebar, &root);
            cx.notify();
        });

        let dock_area = self.state.dock_area.clone();
        let weak_dock = dock_area.downgrade();

        // Everything the app already created, with where it sits.
        let live: Vec<LivePanel> = dock_area
            .read(cx)
            .tab_panels(cx)
            .into_iter()
            .flat_map(|(placement, tabs)| {
                tabs.read(cx)
                    .all_panels()
                    .into_iter()
                    .map(move |panel| LivePanel { panel, placement })
            })
            .collect();
        let anchor_panels: HashSet<EntityId> = self
            .state
            .center_tabs
            .read(cx)
            .all_panels()
            .iter()
            .map(|p| p.view().entity_id())
            .collect();

        let mut restorer = Restorer {
            project_root: root,
            live,
            anchor_panels,
            anchor: Some(self.state.center_tabs.clone()),
            dock_area: weak_dock,
        };

        // Centre. If the saved tree never mentions the anchor group's panels
        // (it always should: the level editor cannot be closed) leave the
        // centre as it is rather than orphan the entity everything holds.
        let center = restorer.build_item(&layout.dock.center, window, cx);
        match center {
            Some(center) if restorer.anchor.is_none() => {
                dock_area.update(cx, |area, cx| area.set_center(center, window, cx));
            }
            _ => tracing::warn!("layout: saved centre has no editor tab; keeping the default"),
        }

        // Side docks, only those this app has.
        let docks = [
            (DockPlacement::Left, layout.dock.left_dock.as_ref()),
            (DockPlacement::Right, layout.dock.right_dock.as_ref()),
            (DockPlacement::Bottom, layout.dock.bottom_dock.as_ref()),
        ];
        for (placement, saved) in docks {
            let Some(saved) = saved else { continue };
            if !dock_area.read(cx).has_dock(placement) {
                continue;
            }
            let Some(item) = restorer.build_item(saved.panel(), window, cx) else {
                continue;
            };
            let (size, open) = (Some(saved.size()), saved.is_open());
            dock_area.update(cx, |area, cx| match placement {
                DockPlacement::Left => area.set_left_dock(item, size, open, window, cx),
                DockPlacement::Right => area.set_right_dock(item, size, open, window, cx),
                DockPlacement::Bottom => area.set_bottom_dock(item, size, open, window, cx),
                DockPlacement::Center => {}
            });
        }

        // A side-dock panel the saved layout did not mention goes back where
        // it was instead of disappearing.
        for LivePanel { panel, placement } in std::mem::take(&mut restorer.live) {
            if placement != DockPlacement::Center {
                dock_area.update(cx, |area, cx| {
                    area.add_panel(panel, placement, None, window, cx)
                });
            }
        }

        self.refresh_open_editor_snapshot(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_group(files: &[&str]) -> PanelState {
        let mut group = PanelState {
            panel_name: "TabPanel".into(),
            info: PanelInfo::tabs(0),
            ..Default::default()
        };
        for file in files {
            group.add_child(
                PanelState {
                    panel_name: "Editor".into(),
                    ..Default::default()
                }
                .with_file(*file),
            );
        }
        group
    }

    #[test]
    fn project_paths_round_trip_through_relative_form() {
        let root = Path::new("/proj");
        let mut state = tab_group(&["/proj/scripts/a.rs", "/elsewhere/b.rs"]);

        relativize(&mut state, root);
        assert_eq!(state.children[0].file(), Some("scripts/a.rs"));
        // Outside the project stays absolute.
        assert_eq!(state.children[1].file(), Some("/elsewhere/b.rs"));

        absolutize(&mut state, root);
        assert_eq!(
            Path::new(state.children[0].file().unwrap()),
            root.join("scripts/a.rs")
        );
        assert_eq!(state.children[1].file(), Some("/elsewhere/b.rs"));
    }

    #[test]
    fn layout_survives_a_write_and_read() {
        let dir = std::env::temp_dir().join(format!("pulsar-layout-{}", std::process::id()));
        let path = layout_path(&dir);
        let layout = SavedLayout {
            version: LAYOUT_VERSION,
            sidebar: SavedSidebar::default(),
            window: None,
            dock: DockAreaState {
                center: tab_group(&["a.rs", "b.rs"]),
                ..Default::default()
            },
        };

        write_layout(&path, &layout).unwrap();
        let read = read_layout(&path).expect("layout reads back");
        assert_eq!(read.dock.center, layout.dock.center);
        assert!(
            !path.with_extension("json.tmp").exists(),
            "temp file renamed away"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tile_bounds_and_stacking_survive_a_write_and_read() {
        use gpui::{Bounds, point, px, size};
        use ui::dock::TileMeta;

        let metas = vec![
            TileMeta {
                bounds: Bounds {
                    origin: point(px(10.), px(20.)),
                    size: size(px(300.), px(200.)),
                },
                z_index: 1,
            },
            TileMeta {
                bounds: Bounds {
                    origin: point(px(50.), px(60.)),
                    size: size(px(400.), px(250.)),
                },
                z_index: 0,
            },
        ];
        let tiles = PanelState {
            panel_name: "Tiles".into(),
            children: vec![tab_group(&["a.rs"]), tab_group(&["b.rs"])],
            info: PanelInfo::tiles(metas.clone()),
        };
        let dir = std::env::temp_dir().join(format!("pulsar-layout-t-{}", std::process::id()));
        let path = layout_path(&dir);
        let layout = SavedLayout {
            version: LAYOUT_VERSION,
            sidebar: SavedSidebar::default(),
            window: None,
            dock: DockAreaState {
                center: tiles,
                ..Default::default()
            },
        };

        write_layout(&path, &layout).unwrap();
        let read = read_layout(&path).unwrap();
        assert_eq!(read.dock.center.info, PanelInfo::tiles(metas));
        assert_eq!(read.dock.center.children.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_geometry_round_trips_with_its_state() {
        use gpui::{point, px, size};

        let restore = Bounds {
            origin: point(px(120.), px(80.)),
            size: size(px(1500.), px(900.)),
        };
        let dir = std::env::temp_dir().join(format!("pulsar-layout-w-{}", std::process::id()));
        let layout = SavedLayout {
            version: LAYOUT_VERSION,
            sidebar: SavedSidebar::default(),
            dock: DockAreaState::default(),
            window: Some(SavedWindow::capture(WindowBounds::Maximized(restore), None)),
        };
        write_layout(&layout_path(&dir), &layout).unwrap();

        // Maximized stays maximized and keeps its restore size.
        assert_eq!(
            saved_window_bounds(&dir),
            Some(WindowBounds::Maximized(restore))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fullscreen_reopens_fullscreen_whatever_its_saved_size() {
        use gpui::{point, px, size};

        // A fullscreen window reports screen-sized bounds; the saved size is
        // the windowed one to return to, and even garbage there must not stop
        // it reopening in full screen.
        let screen = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(2560.), px(1440.)),
        };
        let windowed = Bounds {
            origin: point(px(200.), px(100.)),
            size: size(px(1400.), px(800.)),
        };

        let saved = SavedWindow::capture(WindowBounds::Fullscreen(screen), Some(windowed));
        assert_eq!(saved.state, SavedWindowState::Fullscreen);
        assert_eq!(saved.bounds, windowed, "saves the size to return to");
        assert_eq!(
            saved.to_window_bounds(),
            Some(WindowBounds::Fullscreen(windowed))
        );

        let garbage = SavedWindow {
            state: SavedWindowState::Fullscreen,
            bounds: Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(f32::NAN), px(1.)),
            },
        };
        assert!(matches!(
            garbage.to_window_bounds(),
            Some(WindowBounds::Fullscreen(_))
        ));
    }

    #[test]
    fn an_implausible_window_size_is_not_used() {
        use gpui::{point, px, size};

        let tiny = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(10.), px(10.)),
        };
        assert!(
            SavedWindow::capture(WindowBounds::Windowed(tiny), None)
                .to_window_bounds()
                .is_none()
        );
        let nan = Bounds {
            origin: point(px(0.), px(0.)),
            size: size(px(f32::NAN), px(900.)),
        };
        assert!(
            SavedWindow::capture(WindowBounds::Windowed(nan), None)
                .to_window_bounds()
                .is_none()
        );
    }

    #[test]
    fn sidebar_state_survives_a_write_and_older_files_read_without_it() {
        use super::super::nav_sidebar::model::TabKey;
        let dir = std::env::temp_dir().join(format!("pulsar-layout-sidebar-{}", std::process::id()));
        let path = layout_path(&dir);
        let layout = SavedLayout {
            version: LAYOUT_VERSION,
            sidebar: SavedSidebar {
                pinned: vec![TabKey::File("Content/Hero.class".into()), TabKey::Panel("Level Editor".into())],
                groups: vec![super::super::nav_sidebar::model::SavedGroup {
                    name: "Combat".into(),
                    members: vec![TabKey::File("Content/Sword.class".into())],
                    collapsed: false,
                }],
                collapsed_groups: vec!["Blueprint Editor".into()],
                expanded_folders: vec!["Content/Maps".into()],
            },
            window: None,
            dock: DockAreaState::default(),
        };
        write_layout(&path, &layout).unwrap();
        assert_eq!(read_layout(&path).unwrap().sidebar, layout.sidebar);

        // A file written before the sidebar existed has no `sidebar` key.
        let mut older = serde_json::to_value(&layout).unwrap();
        older.as_object_mut().unwrap().remove("sidebar");
        std::fs::write(&path, older.to_string()).unwrap();
        assert!(read_layout(&path).expect("older layout").sidebar.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_layout_of_another_version_is_ignored() {
        let dir = std::env::temp_dir().join(format!("pulsar-layout-v-{}", std::process::id()));
        let path = layout_path(&dir);
        let stale = SavedLayout {
            version: LAYOUT_VERSION + 1,
            sidebar: SavedSidebar::default(),
            window: None,
            dock: DockAreaState::default(),
        };
        write_layout(&path, &stale).unwrap();
        assert!(read_layout(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewriting_dock_panels_round_trips_a_side_dock() {
        // The side docks go through `map_dock_panels`; check it reaches inside.
        let json = serde_json::json!({
            "panel": serde_json::to_value(tab_group(&["/proj/x.rs"])).unwrap(),
            "placement": "left",
            "size": 420.0,
            "open": true,
        });
        let mut dock = Some(serde_json::from_value::<DockState>(json).unwrap());
        map_dock_panels(&mut dock, |s| relativize(s, Path::new("/proj")));
        let dock = dock.unwrap();
        assert_eq!(dock.panel().children[0].file(), Some("x.rs"));
        assert!(dock.is_open());
    }
}
