//! Mode Tools dock panel — the left-hand panel a [`ToolMode`] renders its
//! [`PanelTab`]s into when [`ModeLayout::show_mode_panel`] is `true` (see
//! design doc's tool-modes-layout addendum, §10).
//!
//! Generic over every mode: it never names `TerrainMode` or any other
//! concrete mode, it just asks the registry for whichever mode is active and
//! renders that mode's `panel_tabs()` — a mode with a small control set gets
//! one unnamed tab for free (the trait's default), a mode with a lot to show
//! (Terrain's Sculpt/Foliage split) gets real tab navigation. A mode that
//! never sets `show_mode_panel` never causes this panel to be shown (see
//! `ui/panel.rs`'s `sync_mode_layout`), so adding a mode that doesn't use it
//! costs this file nothing.
//!
//! [`ToolMode`]: crate::level_editor::tool_modes::ToolMode
//! [`ModeLayout::show_mode_panel`]: crate::level_editor::tool_modes::ModeLayout::show_mode_panel

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::tool_modes::{PanelTab, ToolModeId};
use crate::level_editor::ui::mode_widgets::{active_mode_tabs, render_mode_widgets, WidgetLayout};
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme, Sizable,
};

/// Self-refreshing left-hand panel showing the active tool mode's tabbed
/// controls.
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
    /// `(active mode id, that mode's current tabs)`. Comparing the whole tab
    /// list (not just the id) is what catches a brush-value change
    /// repainting this panel's sliders while staying on the same mode.
    last_signature: (ToolModeId, Vec<PanelTab>),
    /// Id of the selected tab. Reset to the first tab whenever the active
    /// mode id changes (switching from Terrain to Level Edit and back must
    /// not leave a stale tab selected against Level Edit's own — currently
    /// empty — tab list), but preserved across a same-mode tab-content
    /// change (a brush value changing must not silently switch tabs).
    active_tab: &'static str,
    pump_started: bool,
}

impl ModeToolsPanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_signature = Self::signature(&state, &gpu_engine);
        let active_tab = last_signature
            .1
            .first()
            .map(|tab| tab.id)
            .unwrap_or("default");
        Self {
            state,
            gpu_engine,
            focus_handle: cx.focus_handle(),
            last_signature,
            active_tab,
            pump_started: false,
        }
    }

    fn signature(
        state_arc: &Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: &Arc<std::sync::Mutex<GpuRenderer>>,
    ) -> (ToolModeId, Vec<PanelTab>) {
        let state = state_arc.read();
        let id = state.editor.tool_mode_registry.selected_id();
        (id, active_mode_tabs(&state, gpu_engine))
    }

    /// Bring `active_tab` in line with a freshly observed signature: reset
    /// to the first tab only when the mode itself changed, or when the
    /// previously selected tab id no longer exists in the new tab list
    /// (a mode that changes its own tab set dynamically must not leave this
    /// panel pointed at a tab that vanished).
    fn reconcile_active_tab(&mut self, new_signature: &(ToolModeId, Vec<PanelTab>)) {
        let mode_changed = new_signature.0 != self.last_signature.0;
        let tab_still_exists = new_signature
            .1
            .iter()
            .any(|tab| tab.id == self.active_tab);
        if mode_changed || !tab_still_exists {
            self.active_tab = new_signature
                .1
                .first()
                .map(|tab| tab.id)
                .unwrap_or("default");
        }
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
                    this.reconcile_active_tab(&signature);
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
        let signature = Self::signature(&self.state, &self.gpu_engine);
        self.reconcile_active_tab(&signature);
        self.last_signature = signature;
        let tabs = self.last_signature.1.clone();

        let theme = cx.theme();
        let show_tab_bar = tabs.len() > 1;
        let active_widgets = tabs
            .iter()
            .find(|tab| tab.id == self.active_tab)
            .map(|tab| tab.widgets.clone())
            .unwrap_or_default();

        let mut root = v_flex().size_full().bg(theme.sidebar);

        if show_tab_bar {
            let mut tab_bar = ui::h_flex()
                .w_full()
                .gap_1()
                .p_1()
                .border_b_1()
                .border_color(theme.border.opacity(0.6));
            for tab in &tabs {
                let is_active = tab.id == self.active_tab;
                let entity = cx.entity();
                let tab_id = tab.id;
                let btn = Button::new(format!("mode_tools_tab_{}", tab.id))
                    .label(t!(tab.label_key))
                    .small()
                    .on_click(move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            this.active_tab = tab_id;
                            cx.notify();
                        });
                    });
                tab_bar = tab_bar.child(if is_active { btn.primary() } else { btn.ghost() });
            }
            root = root.child(tab_bar);
        }

        root.child(
            v_flex()
                .size_full()
                .p_2()
                .gap_2()
                .child(render_mode_widgets(
                    active_widgets,
                    WidgetLayout::Panel,
                    self.state.clone(),
                    self.gpu_engine.clone(),
                    cx,
                )),
        )
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
