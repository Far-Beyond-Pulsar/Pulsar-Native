use gpui::*;
use ui::{button::{Button, ButtonVariants as _}, h_flex, ActiveTheme};
use std::sync::Arc;
use rust_i18n::t;

mod actions;
mod playback_controls;
mod time_scale_dropdown;
mod multiplayer_dropdown;
mod build_dropdowns;
mod mode_indicator;

pub use actions::*;
use playback_controls::PlaybackControls;
use time_scale_dropdown::TimeScaleDropdown;
use multiplayer_dropdown::MultiplayerDropdown;
use build_dropdowns::BuildDropdowns;
use mode_indicator::ModeIndicator;

use super::state::LevelEditorState;

/// Premium Toolbar - A beautifully crafted control panel for game development
/// 
/// Features:
/// - **Playback Controls**: Intuitive play/pause/stop for simulation
/// - **Time Scale**: Smooth dropdown for speed control with checkmarks
/// - **Multiplayer Mode**: Clean dropdown for networking options
/// - **Build Settings**: Professional dropdowns for config & platform
/// - **Mode Indicator**: Polished badge showing current mode
/// - **Performance Toggle**: Quick access to profiling overlay
/// 
/// Design Philosophy:
/// - Use dropdowns where choice clarity matters (time scale, multiplayer, build)
/// - Use badges for status display (mode indicator)
/// - Use icon buttons for toggles (profiling)
/// - Everything styled to blend seamlessly with the editor
/// - All controls modify real state immediately
pub struct ToolbarPanel;

impl ToolbarPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        
        h_flex()
            .w_full()
            .h(px(48.0))
            .px_4()
            .gap_3()
            .items_center()
            .bg(theme.sidebar.opacity(0.98))
            .border_b_1()
            .border_color(theme.border.opacity(0.8))
            .shadow_sm()
            .child(PlaybackControls::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(TimeScaleDropdown::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(MultiplayerDropdown::render(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(BuildDropdowns::render(state, state_arc.clone(), cx))
            .child(div().flex_1())
            .child(ModeIndicator::render(state, cx))
            .child(self.render_separator(cx))
            .child(self.render_profiling_button(state, state_arc.clone(), cx))
    }

    fn render_separator<V: 'static>(&self, cx: &mut Context<V>) -> impl IntoElement {
        div()
            .h_6()
            .w_px()
            .bg(cx.theme().border.opacity(0.4))
    }

    fn render_profiling_button<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let state_clone = state_arc.clone();
        let is_profiling = state.show_performance_overlay;
        let btn = Button::new("toggle_profiling")
            .icon(ui::IconName::Activity)
            .tooltip(t!("LevelEditor.Toolbar.TogglePerformance"))
            .on_click(move |_, _, _| {
                let mut s = state_clone.write();
                s.show_performance_overlay = !s.show_performance_overlay;
            });
        if is_profiling {
            btn.primary()
        } else {
            btn
        }
    }
}
