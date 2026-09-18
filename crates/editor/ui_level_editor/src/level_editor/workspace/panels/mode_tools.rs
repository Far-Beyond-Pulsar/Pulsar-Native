//! Mode Tools dock panel — the left-hand panel a [`ToolMode`] renders its
//! [`ToolWidget`] controls into when [`ModeLayout::show_mode_panel`] is `true`
//! (see design doc's tool-modes-layout addendum).
//!
//! Generic over every mode: it never names `TerrainMode` or any other
//! concrete mode, it just asks the registry for whichever mode is active and
//! renders that mode's `toolbar_controls()` vertically via the same
//! [`crate::level_editor::ui::mode_widgets`] renderer the horizontal toolbar
//! uses. A mode that never sets `show_mode_panel` never causes this panel to
//! be shown (see `ui/panel.rs`'s `sync_mode_layout`), so adding a mode that
//! doesn't use it costs this file nothing.
//!
//! [`ToolMode`]: crate::level_editor::tool_modes::ToolMode
//! [`ModeLayout::show_mode_panel`]: crate::level_editor::tool_modes::ModeLayout::show_mode_panel

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::tool_modes::{ToolModeId, ToolWidget};
use crate::level_editor::ui::mode_widgets::{active_mode_widgets, render_mode_widgets, WidgetLayout};
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme,
};

/// Self-refreshing left-hand panel showing the active tool mode's widgets.
///
/// Like `HierarchyPanelWrapper`/`PropertiesPanelWrapper`, invalidates itself
/// via a frame pump rather than relying on GPUI's entity-access tracking
/// (which cannot see through the `Arc<RwLock<LevelEditorState>>` these
/// widgets read from) — see `ui/frame_pump.rs`'s doc for why that pattern
/// exists everywhere in this editor.
pub struct ModeToolsPanel {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    focus_handle: FocusHandle,
    /// `(active mode id, that mode's current widget list)`. Comparing the
    /// whole widget list (not just the id) is what catches a brush-value
    /// change repainting this panel's sliders while staying on the same mode.
    last_signature: (ToolModeId, Vec<ToolWidget>),
    pump_started: bool,
}

impl ModeToolsPanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_signature = Self::signature(&state, &gpu_engine);
        Self {
            state,
            gpu_engine,
            focus_handle: cx.focus_handle(),
            last_signature,
            pump_started: false,
        }
    }

    fn signature(
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<std::sync::Mutex<GpuRenderer>>,
    ) -> (ToolModeId, Vec<ToolWidget>) {
        let state = state_arc.read();
        let id = state.editor.tool_mode_registry.selected_id();
        (id, active_mode_widgets(&state, gpu_engine))
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, _window, cx| {
                let signature = Self::signature(&this.state, &this.gpu_engine);
                if signature != this.last_signature {
                    this.last_signature = signature;
                    cx.notify();
                }
            },
        );
    }
}

impl EventEmitter<PanelEvent> for ModeToolsPanel {}

ui_common::panel_boilerplate!(ModeToolsPanel);

impl Render for ModeToolsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("mode tools panel: render");
        let _t = gpui::render_stats::scope("mode tools panel: render");

        self.start_pump(window, cx);

        // Record what we are about to paint, same reasoning as every other
        // frame-pumped panel in this editor: avoids the pump re-notifying for
        // a change this render already picked up.
        self.last_signature = Self::signature(&self.state, &self.gpu_engine);
        let controls = self.last_signature.1.clone();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_2()
            .gap_2()
            .child(render_mode_widgets(
                controls,
                WidgetLayout::Panel,
                self.state.clone(),
                self.gpu_engine.clone(),
                cx,
            ))
    }
}

impl Panel for ModeToolsPanel {
    fn panel_name(&self) -> &'static str {
        "mode_tools"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let state = self.state.read();
        let label = t!(state.editor.tool_mode_registry.selected().label_key());
        t!("LevelEditor.ModeTools.Title", mode => label.to_string())
            .to_string()
            .into_any_element()
    }
}
