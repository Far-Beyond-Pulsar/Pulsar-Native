//! Mode Tools dock panels — one real `ui::dock::Panel` per [`PanelTab`] a
//! [`ToolMode`] returns from `panel_tabs()`, shown when
//! [`ModeLayout::show_mode_panel`] is `true` (see design doc's
//! tool-modes-layout addendum, §10).
//!
//! Each tab is its own dock panel, grouped into the left dock's native tab
//! strip via `DockItem::tabs` — the same mechanism the right dock already
//! uses for Properties/World Settings — rather than a bespoke tab bar drawn
//! inside one panel. This gets drag-reorder, floating, and closing for free
//! from the dock system, and keeps every panel in this editor behaving the
//! same way.
//!
//! Generic over every mode: `ModeToolsPanel` never names `TerrainMode` or any
//! other concrete mode, it just renders whichever `PanelTab` (by id) it was
//! constructed for. A mode that never sets `show_mode_panel` never causes any
//! of these to be created (see `ui/panel.rs`'s `sync_mode_layout`), so adding
//! a mode that doesn't use it costs this file nothing.
//!
//! [`ToolMode`]: crate::level_editor::tool_modes::ToolMode
//! [`ModeLayout::show_mode_panel`]: crate::level_editor::tool_modes::ModeLayout::show_mode_panel

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::tool_modes::{ToolModeId, ToolWidget};
use crate::level_editor::ui::mode_widgets::{active_mode_tabs, render_mode_widgets, WidgetLayout};
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme,
};

/// Self-refreshing dock panel showing one tab's worth of the active tool
/// mode's controls.
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
    /// Which tab (by [`PanelTab::id`](crate::level_editor::tool_modes::PanelTab::id))
    /// this panel instance renders. Fixed at construction: a panel never
    /// repurposes itself to show a different tab — `sync_mode_layout` rebuilds
    /// the whole left-dock tab set (one `ModeToolsPanel` per current tab)
    /// whenever the active mode changes instead.
    tab_id: &'static str,
    /// This tab's label, cached at construction for `title()` — the dock's
    /// tab strip calls `title()` on every paint, so this avoids a lock plus a
    /// linear scan of `panel_tabs()` per frame just to find our own label.
    label_key: &'static str,
    /// `(active mode id, this tab's current widget list)`. Comparing the
    /// widget list (not just the mode id) is what catches a brush-value
    /// change repainting this panel's sliders while staying on the same tab.
    /// If the mode changes away from whoever owns `tab_id`, the widget list
    /// resolves to empty — this panel renders nothing until
    /// `sync_mode_layout` removes it from the dock on the next mode switch.
    last_signature: (ToolModeId, Vec<ToolWidget>),
    pump_started: bool,
}

impl ModeToolsPanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        tab_id: &'static str,
        label_key: &'static str,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_signature = Self::signature(&state, &gpu_engine, tab_id);
        Self {
            state,
            gpu_engine,
            focus_handle: cx.focus_handle(),
            tab_id,
            label_key,
            last_signature,
            pump_started: false,
        }
    }

    fn signature(
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<std::sync::Mutex<GpuRenderer>>,
        tab_id: &'static str,
    ) -> (ToolModeId, Vec<ToolWidget>) {
        let state = state_arc.read();
        let id = state.editor.tool_mode_registry.selected_id();
        let widgets = active_mode_tabs(&state, gpu_engine)
            .into_iter()
            .find(|tab| tab.id == tab_id)
            .map(|tab| tab.widgets)
            .unwrap_or_default();
        (id, widgets)
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
                let signature = Self::signature(&this.state, &this.gpu_engine, this.tab_id);
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
        self.last_signature = Self::signature(&self.state, &self.gpu_engine, self.tab_id);
        let widgets = self.last_signature.1.clone();

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .p_2()
            .gap_2()
            .child(render_mode_widgets(
                widgets,
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
        t!(self.label_key).to_string().into_any_element()
    }
}
