//! LevelEditorPanel — the level editor's dock container.
//!
//! Owns construction, the dock workspace and its reconciliation with the active
//! tool mode / Play-In-Editor state, and the root element tree. The `on_*`
//! action handlers live in the sibling [`handlers`] module; the
//! Play-In-Editor dylib build pipeline lives in [`pie`].

mod handlers;
pub(super) mod pie;
pub(super) use pie::{begin_pie, end_pie};
mod camera;
mod lifecycle;
mod workspace_sync;

use gpui::*;
use rust_i18n::t;
use ui::{
    dock::{DockItem, Panel, PanelEvent},
    resizable::ResizableState,
    v_flex,
    workspace::Workspace,
};
// HelioViewport — GPUI-native Helio 3D viewport
use super::viewport::helio_viewport::HelioViewport;

use engine_backend::services::gpu_renderer::{GpuRenderer, GpuRendererBuilder};
use std::sync::{Arc, Mutex};
use ui::settings::EngineSettings;
use ui::{notification::Notification, ContextModal as _};

use super::actions::*;
use super::{StatusBarView, ToolbarView, ViewportPanel};
use crate::ai_sessions;
use crate::level_editor::scene_database::LevelEditorCameraState;
use crate::level_editor::{LevelEditorState, TransformTool};
use engine_backend::subsystems::render::{EditorCameraState, HelioEditorMailbox};
use plugin_manager;

/// Main Level Editor Panel - Orchestrates all sub-components
///
/// Invalidation model: this panel owns almost nothing that other panels need.
/// Every sub-panel (toolbar, status bar, hierarchy, properties, viewport) is
/// its own cached view with its own frame pump watching exactly the state it
/// renders, so scene edits invalidate only the panels that display them.
/// This panel re-renders only for what IT displays: the Play-In-Editor Game
/// tab, and the tab title (scene path / unsaved marker). See
/// `frame_pump`'s doc for why polling beats cross-panel notify cascades here.
pub struct LevelEditorPanel {
    focus_handle: FocusHandle,

    // UI Components. Both are separate entities rendered with `AnyView::cached`
    // so they survive the per-frame invalidation the viewport propagates up the
    // ancestor chain; each owns a frame pump that notifies it only when the
    // state it actually reads changes. See `toolbar::view` / `status_bar_view`.
    toolbar: Entity<ToolbarView>,
    status_bar: Entity<StatusBarView>,

    // Helio viewport rendered via WgpuSurfaceHandle
    viewport: Entity<HelioViewport>,
    gpu_engine: Arc<Mutex<GpuRenderer>>, // Full GPU renderer from backend
    // Cheap, `gpu_engine`-lock-free handle for one-shot editor commands
    // (gizmo mode, deselect, force-full-resync) -- see `HelioEditorMailbox`'s
    // doc. Fetched once at construction, not re-locked per command.
    helio_mailbox: Option<HelioEditorMailbox>,
    /// Voxel terrain edit seam, fetched once alongside `helio_mailbox`.
    /// Undo/redo and the save path use it; see `TerrainEditApi`.
    terrain_api: Option<engine_backend::services::terrain_edit::TerrainEditApi>,
    render_enabled: Arc<std::sync::atomic::AtomicBool>,

    // Shared state for all panels (single source of truth)
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,

    // Workspace for draggable panels
    workspace: Option<Entity<Workspace>>,

    // Play In Editor (issue #243): the Game tab is opened when the game starts
    // and removed on stop. `game_panel` is the live tab entity, if open.
    game_panel: Option<Entity<crate::level_editor::ui::viewport::game_viewport::GameViewport>>,

    /// Last `(building, active, pending_start, has_error)` tuple that
    /// [`Self::sync_game_tab`] acted on. Render runs several times per second
    /// (viewport publishes dirty every ancestor); without this guard each of
    /// those renders would take a write lock on the shared state for nothing.
    applied_pie_signature: Option<(bool, bool, bool, bool)>,

    /// Last tool-mode id [`Self::sync_mode_layout`] reconciled the dock area
    /// against, guarding it the same way `applied_pie_signature` guards
    /// `sync_game_tab` — this runs on every render, so a plain `!=` check on
    /// a `Copy` id is what keeps it from touching the dock area for nothing.
    /// Because this only fires on an actual mode switch (a rare, deliberate
    /// user action), `sync_mode_layout` is free to fully rebuild the left
    /// dock's panel set each time rather than caching individual panel
    /// entities — unlike `sync_game_tab`, there is no per-render cost to
    /// avoid here.
    applied_mode_layout: Option<crate::level_editor::tool_modes::ToolModeId>,

    /// Mode-contributed right-dock panels currently inserted, by panel id.
    ///
    /// `sync_mode_layout` rebuilds the right dock only when this set actually
    /// changes between mode switches — the common case (modes contributing no
    /// right panels) shares the empty set, so the Properties/World Settings
    /// tab group and `PropertiesPanelWrapper`'s cached sections survive
    /// untouched. Only a mode that newly contributes right panels (or stops
    /// doing so) pays for a right-dock rebuild, and only on the switch itself.
    /// Left-dock contributions need no such tracking: the left dock is a full
    /// `set_left_dock` rebuild on every mode switch already.
    mode_right_panels: Vec<&'static str>,

    // Keeps the polling task alive for the lifetime of the panel.
    _root_input_poller: gpui::Task<()>,
}


impl Drop for LevelEditorPanel {
    fn drop(&mut self) {
        if let Some(path) = self.shared_state.read().scene.current_scene.clone() {
            ai_sessions::unregister_open_scene(&path);
        }
    }
}

impl Panel for LevelEditorPanel {
    fn panel_name(&self) -> &'static str {
        "Level Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let state = self.shared_state.read();
        div()
            .child(if let Some(ref scene) = state.scene.current_scene {
                format!(
                    "Level Editor - {}{}",
                    scene
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("Untitled"),
                    if state.scene.has_unsaved_changes {
                        " *"
                    } else {
                        ""
                    }
                )
            } else {
                "Level Editor".to_string()
            })
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }

    fn panel_file_path(&self, _cx: &App) -> Option<std::path::PathBuf> {
        self.shared_state.read().scene.current_scene.clone()
    }

    fn tab_icon(&self, _cx: &App) -> Option<ui::IconName> {
        let state = self.shared_state.read();
        let file_path = state.scene.current_scene.as_ref()?;

        // Get the file type icon from the plugin manager registry
        if let Some(plugin_mgr) = plugin_manager::global() {
            if let Some(file_type_def) = plugin_mgr.read().get_file_type_for_path(file_path) {
                return Some(file_type_def.icon.clone());
            }
        }

        None
    }

    fn tab_unsaved(&self, _cx: &App) -> bool {
        self.shared_state.read().scene.has_unsaved_changes
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut App) {
        if active {
            self.viewport.update(cx, |v, _| v.mark_tab_activated());
        }
    }
}

ui_common::panel_boilerplate!(LevelEditorPanel);

impl EventEmitter<PanelEvent> for LevelEditorPanel {}

impl Render for LevelEditorPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Outermost cached view in the level editor. Its rebuild scope contains
        // every nested one, so if this tracks the draw count the whole tree is
        // being walked every frame no matter how well the panels below cache.
        gpui::render_stats::count("level editor root: render");
        let _t = gpui::render_stats::scope("level editor root: render");

        // Initialize workspace on first render
        self.initialize_workspace(window, cx);

        // Open/close the Play-In-Editor Game tab as the game starts/stops.
        // Guarded: a no-op unless the PiE tuple actually changed since the
        // last render that acted on it (see its doc).
        self.sync_game_tab(window, cx);

        // Reconcile the dock area with the active tool mode's layout.
        // Guarded: a no-op unless the active mode changed since the last
        // render that acted on it (see its doc).
        self.sync_mode_layout(window, cx);

        // NOTE: There is deliberately no per-render engine or panel work left
        // here. Selection/tool sync to Helio runs in the 50ms root-input
        // poller (spawned in `new_internal`), and every sub-panel invalidates
        // itself via its own frame pump. Render only builds this panel's own
        // element tree.

        v_flex()
            .size_full()
            // NO BACKGROUND - allow transparency for viewport
            .key_context("LevelEditor")
            .track_focus(&self.focus_handle)
            // Scene operations
            .on_action(cx.listener(Self::on_new_scene))
            .on_action(cx.listener(Self::on_open_scene))
            .on_action(cx.listener(Self::on_save_scene))
            .on_action(cx.listener(Self::on_save_scene_as))
            // Transform tools - KEYBOARD: Q/W/E/R
            .on_action(cx.listener(Self::on_select_tool))
            .on_action(cx.listener(Self::on_move_tool))
            .on_action(cx.listener(Self::on_rotate_tool))
            .on_action(cx.listener(Self::on_scale_tool))
            // Toolbar actions
            .on_action(cx.listener(Self::on_set_tool_mode))
            .on_action(cx.listener(Self::on_set_time_scale))
            .on_action(cx.listener(Self::on_set_multiplayer_mode))
            .on_action(cx.listener(Self::on_set_build_config))
            .on_action(cx.listener(Self::on_set_target_platform))
            .on_action(cx.listener(Self::on_set_build_mode))
            // Object operations
            .on_action(cx.listener(Self::on_add_object))
            .on_action(cx.listener(Self::on_add_object_of_type))
            .on_action(cx.listener(Self::on_delete_object))
            .on_action(cx.listener(Self::on_duplicate_object))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_select_object))
            .on_action(cx.listener(Self::on_toggle_object_expanded))
            .on_action(cx.listener(Self::on_focus_selected))
            // View operations
            .on_action(cx.listener(Self::on_toggle_grid))
            .on_action(cx.listener(Self::on_toggle_wireframe))
            .on_action(cx.listener(Self::on_toggle_lighting))
            .on_action(cx.listener(Self::on_toggle_performance_overlay))
            .on_action(cx.listener(Self::on_toggle_camera_mode_selector))
            .on_action(cx.listener(Self::on_toggle_viewport_options))
            .on_action(cx.listener(Self::on_toggle_fps_graph_type))
            // Performance metrics toggles
            .on_action(cx.listener(Self::on_toggle_fps_graph))
            .on_action(cx.listener(Self::on_toggle_tps_graph))
            .on_action(cx.listener(Self::on_toggle_frame_time_graph))
            .on_action(cx.listener(Self::on_toggle_memory_graph))
            .on_action(cx.listener(Self::on_toggle_draw_calls_graph))
            .on_action(cx.listener(Self::on_toggle_vertices_graph))
            .on_action(cx.listener(Self::on_toggle_input_latency_graph))
            .on_action(cx.listener(Self::on_toggle_ui_consistency_graph))
            // Play/Edit mode
            .on_action(cx.listener(Self::on_play_scene))
            .on_action(cx.listener(Self::on_stop_scene))
            // Camera modes
            .on_action(cx.listener(Self::on_perspective_view))
            .on_action(cx.listener(Self::on_orthographic_view))
            .on_action(cx.listener(Self::on_top_view))
            .on_action(cx.listener(Self::on_front_view))
            .on_action(cx.listener(Self::on_side_view))
            // Keyboard shortcuts - LETTER KEYS for fast workflow
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Respond if this panel or any child (e.g. viewport) has focus,
                // and no modifier keys are held.
                if !this.focus_handle.contains_focused(window, cx)
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.shift
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.function
                {
                    return;
                }

                match event.keystroke.key.as_ref() {
                    "escape" => {
                        // Update UI state unconditionally — always clear GPUI selection.
                        this.shared_state.write().scene.select_object(None);
                        // Mailbox, not `gpu_engine.lock()` -- `queue_deselect`
                        // was already just an atomic-flag write internally;
                        // the lock here was pure incidental overhead from the
                        // wrapper shape, not a real dependency.
                        if let Some(mailbox) = &this.helio_mailbox {
                            mailbox.queue_deselect();
                        }
                        cx.notify();
                    }
                    // Tool selection — call handlers directly to avoid action-dispatch drift.
                    "q" => this.on_select_tool(&SelectTool, window, cx),
                    "w" => this.on_move_tool(&MoveTool, window, cx),
                    "g" => this.on_move_tool(&MoveTool, window, cx), // Blender: G = Grab/Move
                    "e" => this.on_rotate_tool(&RotateTool, window, cx),
                    "r" => this.on_rotate_tool(&RotateTool, window, cx), // Blender: R = Rotate
                    "s" => this.on_scale_tool(&ScaleTool, window, cx),   // Blender: S = Scale
                    "l" => {}
                    "f" => cx.dispatch_action(&FocusSelected),
                    _ => {}
                }
            }))
            // Additional keyboard shortcuts for Alt+Up/Down (object reordering)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                // Only respond if Alt key is pressed and this panel or any child has focus
                if !this.focus_handle.contains_focused(window, cx) || !event.keystroke.modifiers.alt
                {
                    return;
                }

                match event.keystroke.key.as_ref() {
                    "up" => {
                        // Move selected object up in hierarchy
                        if let Some(id) = this.shared_state.read().scene.selected_object() {
                            this.shared_state.read().scene.database.move_object_up(&id);
                            cx.notify();
                        }
                    }
                    "down" => {
                        // Move selected object down in hierarchy
                        if let Some(id) = this.shared_state.read().scene.selected_object() {
                            this.shared_state
                                .read()
                                .scene
                                .database
                                .move_object_down(&id);
                            cx.notify();
                        }
                    }
                    _ => {}
                }
            }))
            .child(
                // Toolbar at the top. Cached: this panel is marked dirty on
                // every frame the viewport publishes, but the toolbar only
                // needs to rebuild when its own state changes.
                AnyView::from(self.toolbar.clone()).cached(ToolbarView::cache_style()),
            )
            .child(
                // Workspace with draggable panels
                if let Some(ref workspace) = self.workspace {
                    workspace.clone().into_any_element()
                } else {
                    div().child("Loading workspace...").into_any_element()
                },
            )
            .child(
                // Status bar at the bottom. Cached for the same reason as the
                // toolbar above.
                AnyView::from(self.status_bar.clone()).cached(StatusBarView::cache_style()),
            )
    }
}
