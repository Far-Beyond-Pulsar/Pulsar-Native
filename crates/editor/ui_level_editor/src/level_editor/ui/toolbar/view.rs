//! `ToolbarView` — the level editor toolbar as its own cached GPUI view.
//!
//! The toolbar used to be rendered inline by `LevelEditorPanel::render`, which
//! meant its whole element tree — playback controls, four dropdowns, feature
//! toggles, the Build Core split button and every `t!()` lookup behind them —
//! was rebuilt on every frame the panel was dirty. The panel is dirty on every
//! frame Helio publishes, so that was ~60 full rebuilds a second of a toolbar
//! that changes only when the user clicks something.
//!
//! As a separate entity it can be rendered with [`gpui::AnyView::cached`], which
//! replays the previous frame's prepaint/paint ranges and skips `render()`
//! entirely while the view is clean.
//!
//! Invalidation is explicit because the toolbar's inputs live in
//! `Arc<parking_lot::RwLock<LevelEditorState>>`, which GPUI's automatic
//! entity-access tracking cannot see. A per-frame [`frame_pump`] compares a
//! [`ToolbarSignature`] — a `PartialEq` snapshot of exactly the fields the
//! toolbar renders — and notifies only on a real change.
//!
//! **If you add a state read to any toolbar sub-renderer, add the corresponding
//! field to [`ToolbarSignature`]**, or the toolbar will render stale.

use std::sync::Arc;

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use ui::dock::PanelEvent;

use super::ToolbarPanel;
use crate::level_editor::state::terrain::SculptMode;
use crate::level_editor::state::{
    BuildConfig, BuildMode, EditorMode, MultiplayerMode, TargetPlatform,
};
use crate::level_editor::tool_modes::ToolModeId;
use crate::level_editor::ui::frame_pump::spawn_frame_pump;
use crate::level_editor::LevelEditorState;

/// Every piece of [`LevelEditorState`] the toolbar's element tree depends on.
///
/// Kept deliberately flat and `Copy`-ish so building one per frame is trivial
/// next to rebuilding the toolbar itself.
#[derive(Clone, Copy, PartialEq)]
pub struct ToolbarSignature {
    // tool_mode_dropdown / mode_indicator
    tool_mode: ToolModeId,
    // terrain controls
    terrain_sculpt_mode: SculptMode,
    terrain_radius_m: f32,
    terrain_strength: f32,
    terrain_falloff: f32,
    // terrain foliage sub-mode controls
    terrain_paint_foliage: bool,
    terrain_foliage_density: f32,
    terrain_foliage_radius_m: f32,
    terrain_foliage_slope_min: f32,
    terrain_foliage_slope_max: f32,
    // spline mode controls (Milestone 5 extensibility demo)
    spline_point_count: usize,
    spline_length_m: f32,
    // playback_controls / mode_indicator
    editor_mode: EditorMode,
    // time_scale_dropdown
    time_scale: f32,
    // multiplayer_dropdown
    multiplayer_mode: MultiplayerMode,
    // build_dropdowns / build_core
    build_config: BuildConfig,
    target_platform: TargetPlatform,
    build_mode: BuildMode,
    game_running: bool,
    // feature_toggles
    feature_lighting_enabled: bool,
    feature_shadows_enabled: bool,
    feature_bloom_enabled: bool,
    feature_materials_enabled: bool,
    // profiling button
    show_performance_overlay: bool,
}

impl ToolbarSignature {
    fn of(state: &LevelEditorState) -> Self {
        Self {
            tool_mode: state.editor.tool_mode_registry.selected_id(),
            terrain_sculpt_mode: state.editor.terrain.sculpt.mode,
            terrain_radius_m: state.editor.terrain.sculpt.radius_m,
            terrain_strength: state.editor.terrain.sculpt.strength,
            terrain_falloff: state.editor.terrain.sculpt.falloff,
            terrain_paint_foliage: state.editor.terrain.paint_foliage,
            terrain_foliage_density: state.editor.terrain.foliage.density,
            terrain_foliage_radius_m: state.editor.terrain.foliage.radius_m,
            terrain_foliage_slope_min: state.editor.terrain.foliage.slope_limit.0,
            terrain_foliage_slope_max: state.editor.terrain.foliage.slope_limit.1,
            spline_point_count: state.editor.spline.points.len(),
            spline_length_m: state.editor.spline.total_length_m(),
            editor_mode: state.scene.editor_mode,
            time_scale: state.play.time_scale,
            multiplayer_mode: state.play.multiplayer_mode,
            build_config: state.build.config,
            target_platform: state.build.target_platform,
            build_mode: state.build.mode,
            game_running: state.build.game_running,
            feature_lighting_enabled: state.editor.feature_lighting_enabled,
            feature_shadows_enabled: state.editor.feature_shadows_enabled,
            feature_bloom_enabled: state.editor.feature_bloom_enabled,
            feature_materials_enabled: state.editor.feature_materials_enabled,
            show_performance_overlay: state.overlays.state.show_performance_overlay,
        }
    }
}

pub struct ToolbarView {
    toolbar: ToolbarPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    last_signature: ToolbarSignature,
    pump_started: bool,
}

impl ToolbarView {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    ) -> Self {
        let last_signature = ToolbarSignature::of(&state.read());
        Self {
            toolbar: ToolbarPanel::new(),
            state,
            gpu_engine,
            last_signature,
            pump_started: false,
        }
    }

    /// The style the cached view lays itself out with. Must match the root style
    /// of what `ToolbarPanel::render` produces: on a cache hit GPUI lays the
    /// view out from this refinement alone, without consulting its content.
    pub fn cache_style() -> StyleRefinement {
        StyleRefinement::default().w_full().h(px(48.0))
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let signature = ToolbarSignature::of(&this.state.read());
            if signature != this.last_signature {
                this.last_signature = signature;
                cx.notify();
            }
        });
    }
}

impl EventEmitter<PanelEvent> for ToolbarView {}

impl Render for ToolbarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("toolbar: render");
        let _t = gpui::render_stats::scope("toolbar: render");

        self.start_pump(window, cx);

        // Record what we are about to paint so the pump doesn't immediately
        // notify again for a change we have already picked up (e.g. a render
        // triggered by an action handler rather than by the pump).
        self.last_signature = ToolbarSignature::of(&self.state.read());

        let state = self.state.read();
        self.toolbar
            .render(&state, self.state.clone(), self.gpu_engine.clone(), cx)
    }
}
