use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _}, 
    h_flex, ActiveTheme, IconName, Selectable, Sizable, Disableable,
};
use std::sync::Arc;
use rust_i18n::t;

use super::state::{LevelEditorState, MultiplayerMode, BuildConfig, TargetPlatform};

/// Toolbar - Game management and quick actions
/// 
/// This toolbar sits above the viewport and provides controls for:
/// - Play/pause/stop simulation
/// - Time scale controls (inline, not dropdown)
/// - Multiplayer mode (cycles on click)
/// - Build configuration (cycles on click)
/// - Platform target (cycles on click)
/// - Performance profiling toggle
pub struct ToolbarPanel;

impl ToolbarPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(&self, state: &LevelEditorState, state_arc: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut Context<V>) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        
        h_flex()
            .w_full()
            .h(px(44.0))
            .px_4()
            .gap_3()
            .items_center()
            .bg(theme.sidebar.opacity(0.95))
            .border_b_1()
            .border_color(theme.border)
            .child(self.render_playback_controls(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(self.render_time_scale_control(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(self.render_multiplayer_control(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(self.render_build_controls(state, state_arc.clone(), cx))
            .child(div().flex_1())
            .child(self.render_mode_indicator(state, cx))
            .child(self.render_separator(cx))
            .child(self.render_profiling_button(state, state_arc.clone(), cx))
    }

    fn render_separator<V: 'static>(&self, cx: &mut Context<V>) -> impl IntoElement {
        div()
            .h_6()
            .w_px()
            .bg(cx.theme().border.opacity(0.5))
    }

    fn render_playback_controls<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        h_flex()
            .gap_1p5()
            .items_center()
            .child({
                let state_clone = state_arc.clone();
                if state.is_edit_mode() {
                    Button::new("play")
                        .icon(IconName::Play)
                        .tooltip(t!("LevelEditor.Toolbar.StartSimulation"))
                        .on_click(move |_, _, _| {
                            state_clone.write().enter_play_mode();
                        })
                        .into_any_element()
                } else {
                    Button::new("play_active")
                        .icon(IconName::Play)
                        .tooltip(t!("LevelEditor.Toolbar.SimulationRunning"))
                        .selected(true)
                        .into_any_element()
                }
            })
            .child({
                Button::new("pause")
                    .icon(IconName::Pause)
                    .tooltip(t!("LevelEditor.Toolbar.PauseSimulation"))
                    .ghost()
                    .disabled(state.is_edit_mode())
                    .on_click(move |_, _, _| {
                        // TODO: Implement pause
                    })
            })
            .child({
                let state_clone = state_arc.clone();
                Button::new("stop")
                    .icon(IconName::Square)
                    .tooltip(t!("LevelEditor.Toolbar.StopSimulation"))
                    .disabled(state.is_edit_mode())
                    .on_click(move |_, _, _| {
                        state_clone.write().exit_play_mode();
                    })
            })
    }

    fn render_time_scale_control<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        let time_scale = state.game_time_scale;
        
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(
                        ui::Icon::new(IconName::Clock)
                            .size_4()
                            .text_color(theme.muted_foreground)
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Speed")
                    )
            )
            .child(
                div()
                    .px_2p5()
                    .py_1()
                    .rounded(px(4.0))
                    .bg(theme.muted.opacity(0.1))
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if time_scale == 1.0 {
                                theme.foreground
                            } else {
                                theme.accent
                            })
                            .child(format!("{:.2}x", time_scale))
                    )
            )
            .child({
                let state_clone = state_arc.clone();
                Button::new("time_scale_decrease")
                    .icon(IconName::Minus)
                    .small()
                    .ghost()
                    .tooltip("Decrease speed")
                    .on_click(move |_, _, _| {
                        let mut state = state_clone.write();
                        state.game_time_scale = (state.game_time_scale * 0.5).max(0.125);
                    })
            })
            .child({
                let state_clone = state_arc.clone();
                Button::new("time_scale_increase")
                    .icon(IconName::Plus)
                    .small()
                    .ghost()
                    .tooltip("Increase speed")
                    .on_click(move |_, _, _| {
                        let mut state = state_clone.write();
                        state.game_time_scale = (state.game_time_scale * 2.0).min(8.0);
                    })
            })
            .child({
                let state_clone = state_arc.clone();
                Button::new("time_scale_reset")
                    .icon(IconName::Refresh)
                    .small()
                    .ghost()
                    .tooltip("Reset to 1.0x")
                    .on_click(move |_, _, _| {
                        state_clone.write().game_time_scale = 1.0;
                    })
            })
    }

    fn render_multiplayer_control<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let mode_label = match state.multiplayer_mode {
            MultiplayerMode::Offline => "Offline",
            MultiplayerMode::Host => "Host",
            MultiplayerMode::Client => "Client",
        };
        
        let mode_icon = match state.multiplayer_mode {
            MultiplayerMode::Offline => IconName::CircleX,
            MultiplayerMode::Host => IconName::Server,
            MultiplayerMode::Client => IconName::Network,
        };
        
        Button::new("multiplayer_button")
            .label(mode_label)
            .icon(mode_icon)
            .small()
            .ghost()
            .tooltip("Click to cycle multiplayer mode")
            .on_click({
                let state = state_arc.clone();
                move |_, _, _| {
                    let mut s = state.write();
                    s.multiplayer_mode = match s.multiplayer_mode {
                        MultiplayerMode::Offline => MultiplayerMode::Host,
                        MultiplayerMode::Host => MultiplayerMode::Client,
                        MultiplayerMode::Client => MultiplayerMode::Offline,
                    };
                }
            })
    }

    fn render_build_controls<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let config_label = match state.build_config {
            BuildConfig::Debug => "Debug",
            BuildConfig::Release => "Release",
            BuildConfig::Shipping => "Ship",
        };
        
        let platform_label = match state.target_platform {
            TargetPlatform::Windows => "Win",
            TargetPlatform::Linux => "Linux",
            TargetPlatform::MacOS => "macOS",
            TargetPlatform::Android => "Android",
            TargetPlatform::IOS => "iOS",
            TargetPlatform::Web => "Web",
        };
        
        h_flex()
            .gap_1p5()
            .items_center()
            .child(
                Button::new("build_config_button")
                    .label(config_label)
                    .icon(IconName::Settings)
                    .small()
                    .ghost()
                    .tooltip("Click to cycle build config")
                    .on_click({
                        let state = state_arc.clone();
                        move |_, _, _| {
                            let mut s = state.write();
                            s.build_config = match s.build_config {
                                BuildConfig::Debug => BuildConfig::Release,
                                BuildConfig::Release => BuildConfig::Shipping,
                                BuildConfig::Shipping => BuildConfig::Debug,
                            };
                        }
                    })
            )
            .child(
                Button::new("platform_button")
                    .label(platform_label)
                    .icon(IconName::ChevronRight)
                    .small()
                    .ghost()
                    .tooltip("Click to cycle platform target")
                    .on_click({
                        let state = state_arc.clone();
                        move |_, _, _| {
                            let mut s = state.write();
                            s.target_platform = match s.target_platform {
                                TargetPlatform::Windows => TargetPlatform::Linux,
                                TargetPlatform::Linux => TargetPlatform::MacOS,
                                TargetPlatform::MacOS => TargetPlatform::Android,
                                TargetPlatform::Android => TargetPlatform::IOS,
                                TargetPlatform::IOS => TargetPlatform::Web,
                                TargetPlatform::Web => TargetPlatform::Windows,
                            };
                        }
                    })
            )
    }

    fn render_mode_indicator<V: 'static>(
        &self,
        state: &LevelEditorState,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded(px(6.0))
            .bg(if state.is_play_mode() {
                theme.accent.opacity(0.15)
            } else {
                theme.muted.opacity(0.1)
            })
            .border_1()
            .border_color(if state.is_play_mode() {
                theme.accent.opacity(0.3)
            } else {
                theme.border
            })
            .child(
                div()
                    .size(px(6.0))
                    .rounded(px(3.0))
                    .bg(if state.is_play_mode() {
                        gpui::green()
                    } else {
                        theme.muted_foreground
                    })
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if state.is_play_mode() {
                        theme.accent
                    } else {
                        theme.foreground
                    })
                    .child(if state.is_play_mode() {
                        "Playing"
                    } else {
                        "Editing"
                    })
            )
    }

    fn render_profiling_button<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let state_clone = state_arc.clone();
        Button::new("toggle_profiling")
            .icon(IconName::Activity)
            .tooltip(t!("LevelEditor.Toolbar.TogglePerformance"))
            .selected(state.show_performance_overlay)
            .on_click(move |_, _, _| {
                state_clone.write().show_performance_overlay = !state_clone.read().show_performance_overlay;
            })
    }
}
