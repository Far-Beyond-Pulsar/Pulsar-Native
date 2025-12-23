//! Workspace panels for DAW Editor
//!
//! This module provides workspace panels that integrate with the unified workspace system.
//! Each panel holds a DawPanel internally and delegates rendering to it.

use gpui::*;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex};
use super::daw_ui::state::DawUiState;
use super::daw_ui::panel::DawPanel;
use std::sync::Arc;
use parking_lot::RwLock;

/// Main Timeline Panel - wraps the timeline view with embedded toolbar and transport
pub struct TimelinePanel {
    state: Arc<RwLock<DawUiState>>,
    daw_panel: Entity<DawPanel>,
    focus_handle: FocusHandle,
}

impl TimelinePanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));
        
        Self {
            state,
            daw_panel,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for TimelinePanel {}

impl Render for TimelinePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render only the timeline (not the whole DawPanel with sidebars)
        self.daw_panel.update(cx, |panel, cx| {
            v_flex()
                .size_full()
                .bg(cx.theme().background)
                .gap_0()
                // Toolbar at top
                .child(panel.render_toolbar(cx))
                // Transport below toolbar
                .child(panel.render_transport(cx))
                // Timeline content
                .child(panel.render_timeline(cx))
                .into_any_element()
        })
    }
}

impl Focusable for TimelinePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TimelinePanel {
    fn panel_name(&self) -> &'static str {
        "timeline"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Arrangement".into_any_element()
    }
}

/// Mixer Panel - wraps the mixer view
pub struct MixerPanel {
    state: Arc<RwLock<DawUiState>>,
    daw_panel: Entity<DawPanel>,
    focus_handle: FocusHandle,
}

impl MixerPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));
        
        Self {
            state,
            daw_panel,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for MixerPanel {}

impl Render for MixerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render mixer through DawPanel
        self.daw_panel.update(cx, |panel, cx| {
            panel.render_mixer(cx).into_any_element()
        })
    }
}

impl Focusable for MixerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MixerPanel {
    fn panel_name(&self) -> &'static str {
        "mixer"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Mixer".into_any_element()
    }
}

/// Browser Files Tab Panel
pub struct BrowserFilesPanel {
    state: Arc<RwLock<DawUiState>>,
    daw_panel: Entity<DawPanel>,
    focus_handle: FocusHandle,
}

impl BrowserFilesPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));
        
        Self {
            state,
            daw_panel,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for BrowserFilesPanel {}

impl Render for BrowserFilesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render browser through DawPanel
        self.daw_panel.update(cx, |panel, cx| {
            panel.state.browser_tab = super::daw_ui::state::BrowserTab::Files;
            panel.render_browser(cx).into_any_element()
        })
    }
}

impl Focusable for BrowserFilesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BrowserFilesPanel {
    fn panel_name(&self) -> &'static str {
        "browser_files"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Files".into_any_element()
    }
}


/// Browser Instruments Tab Panel
pub struct BrowserInstrumentsPanel {
    state: Arc<RwLock<DawUiState>>,
    daw_panel: Entity<DawPanel>,
    focus_handle: FocusHandle,
}

impl BrowserInstrumentsPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));
        
        Self {
            state,
            daw_panel,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for BrowserInstrumentsPanel {}

impl Render for BrowserInstrumentsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render browser through DawPanel (all tabs use same browser render)
        self.daw_panel.update(cx, |panel, cx| {
            panel.state.browser_tab = super::daw_ui::state::BrowserTab::Instruments;
            panel.render_browser(cx).into_any_element()
        })
    }
}

impl Focusable for BrowserInstrumentsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BrowserInstrumentsPanel {
    fn panel_name(&self) -> &'static str {
        "browser_instruments"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Instruments".into_any_element()
    }
}

/// Browser Effects Tab Panel
pub struct BrowserEffectsPanel {
    state: Arc<RwLock<DawUiState>>,
    daw_panel: Entity<DawPanel>,
    focus_handle: FocusHandle,
}

impl BrowserEffectsPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));
        
        Self {
            state,
            daw_panel,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for BrowserEffectsPanel {}

impl Render for BrowserEffectsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render browser through DawPanel
        self.daw_panel.update(cx, |panel, cx| {
            panel.state.browser_tab = super::daw_ui::state::BrowserTab::Effects;
            panel.render_browser(cx).into_any_element()
        })
    }
}

impl Focusable for BrowserEffectsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BrowserEffectsPanel {
    fn panel_name(&self) -> &'static str {
        "browser_effects"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Effects".into_any_element()
    }
}

/// Browser Loops Tab Panel
pub struct BrowserLoopsPanel {
    state: Arc<RwLock<DawUiState>>,
    daw_panel: Entity<DawPanel>,
    focus_handle: FocusHandle,
}

impl BrowserLoopsPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));
        
        Self {
            state,
            daw_panel,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for BrowserLoopsPanel {}

impl Render for BrowserLoopsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render browser through DawPanel
        self.daw_panel.update(cx, |panel, cx| {
            panel.state.browser_tab = super::daw_ui::state::BrowserTab::Loops;
            panel.render_browser(cx).into_any_element()
        })
    }
}

impl Focusable for BrowserLoopsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BrowserLoopsPanel {
    fn panel_name(&self) -> &'static str {
        "browser_loops"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Loops".into_any_element()
    }
}

/// Browser Samples Tab Panel
pub struct BrowserSamplesPanel {
    state: Arc<RwLock<DawUiState>>,
    daw_panel: Entity<DawPanel>,
    focus_handle: FocusHandle,
}

impl BrowserSamplesPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));
        
        Self {
            state,
            daw_panel,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for BrowserSamplesPanel {}

impl Render for BrowserSamplesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Render browser through DawPanel
        self.daw_panel.update(cx, |panel, cx| {
            panel.state.browser_tab = super::daw_ui::state::BrowserTab::Samples;
            panel.render_browser(cx).into_any_element()
        })
    }
}

impl Focusable for BrowserSamplesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BrowserSamplesPanel {
    fn panel_name(&self) -> &'static str {
        "browser_samples"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Samples".into_any_element()
    }
}

/// Inspector Panel - shows properties of selected items
pub struct InspectorPanel {
    state: Arc<RwLock<DawUiState>>,
    focus_handle: FocusHandle,
}

impl InspectorPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for InspectorPanel {}

impl Render for InspectorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .p_4()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Inspector - Select a track or clip to view properties")
            )
    }
}

impl Focusable for InspectorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for InspectorPanel {
    fn panel_name(&self) -> &'static str {
        "inspector"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Inspector".into_any_element()
    }
}

