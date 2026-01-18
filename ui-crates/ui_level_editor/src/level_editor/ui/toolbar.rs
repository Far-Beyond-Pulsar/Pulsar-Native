use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _, DropdownButton}, 
    h_flex, v_flex, ActiveTheme, IconName, Selectable, Sizable,
    popup_menu::PopupMenuExt,
};
use std::sync::Arc;

use super::state::{LevelEditorState, TransformTool, MultiplayerMode, BuildConfig, TargetPlatform};

// Temporary no-op action for dropdowns
gpui::actions!(level_editor, [NoOpAction]);
use crate::level_editor::scene_database::{ObjectType, MeshType, LightType};

/// Toolbar - Game management and quick actions
/// 
/// This toolbar sits above the viewport and provides controls for:
/// - Play/pause/stop simulation
/// - Time scale controls
/// - Multiplayer server management  
/// - Build configuration and deployment
/// - Performance profiling
pub struct ToolbarPanel;

impl ToolbarPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(&self, state: &LevelEditorState, state_arc: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut Context<V>) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        h_flex()
            .w_full()
            .h(px(40.0))
            .px_3()
            .gap_2()
            .items_center()
            .bg(cx.theme().sidebar)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.render_playback_controls(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(self.render_time_scale_dropdown(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(self.render_multiplayer_dropdown(state, state_arc.clone(), cx))
            .child(self.render_separator(cx))
            .child(self.render_build_dropdown(state, state_arc.clone(), cx))
            .child(div().flex_1())
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
            .gap_1()
            .child({
                let state_clone = state_arc.clone();
                if state.is_edit_mode() {
                    Button::new("play")
                        .icon(IconName::Play)
                        .tooltip("Start Simulation (F5)")
                        .on_click(move |_, _, _| {
                            state_clone.write().enter_play_mode();
                        })
                        .into_any_element()
                } else {
                    Button::new("play_disabled")
                        .icon(IconName::Play)
                        .tooltip("Simulation Running")
                        .ghost()
                        .into_any_element()
                }
            })
            .child({
                Button::new("pause")
                    .icon(IconName::Pause)
                    .tooltip("Pause Simulation (F6)")
                    .ghost()
                    .on_click(move |_, _, _| {
                        // TODO: Implement pause
                    })
            })
            .child({
                let state_clone = state_arc.clone();
                if state.is_play_mode() {
                    Button::new("stop")
                        .icon(IconName::Close)
                        .tooltip("Stop Simulation (Shift+F5)")
                        .on_click(move |_, _, _| {
                            state_clone.write().exit_play_mode();
                        })
                        .into_any_element()
                } else {
                    Button::new("stop_disabled")
                        .icon(IconName::Close)
                        .tooltip("Not Playing")
                        .ghost()
                        .into_any_element()
                }
            })
    }

    fn render_time_scale_dropdown<V: 'static>(
        &self,
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let time_scale = state.game_time_scale;
        let time_scale_label = format!("{}x", time_scale);
        
        DropdownButton::new("time_scale_dropdown")
            .button(
                Button::new("time_scale_button")
                    .label(&time_scale_label)
                    .icon(IconName::Clock)
                    .tooltip("Time Scale")
            )
            .popup_menu(move |menu, _window, _cx| {
                menu
                    .label("Select Time Scale")
                    .separator()
                    .menu_with_icon("0.25x", IconName::Clock, Box::new(NoOpAction))
                    .menu_with_icon("0.5x", IconName::Clock, Box::new(NoOpAction))
                    .menu_with_icon("1.0x (Normal)", IconName::Clock, Box::new(NoOpAction))
                    .menu_with_icon("2.0x", IconName::Clock, Box::new(NoOpAction))
                    .menu_with_icon("4.0x", IconName::Clock, Box::new(NoOpAction))
            })
    }

    fn render_multiplayer_dropdown<V: 'static>(
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
            MultiplayerMode::Host => "Hosting",
            MultiplayerMode::Client => "Client",
        };
        
        DropdownButton::new("multiplayer_dropdown")
            .button(
                Button::new("multiplayer_button")
                    .label(mode_label)
                    .icon(IconName::Network)
                    .tooltip("Multiplayer Mode")
            )
            .popup_menu(move |menu, _window, _cx| {
                menu
                    .label("Multiplayer Mode")
                    .separator()
                    .menu_with_icon("Offline", IconName::PhoneDisabled, Box::new(NoOpAction))
                    .menu_with_icon("Host Server", IconName::Server, Box::new(NoOpAction))
                    .menu_with_icon("Connect as Client", IconName::Link, Box::new(NoOpAction))
            })
    }

    fn render_build_dropdown<V: 'static>(
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
            BuildConfig::Shipping => "Shipping",
        };
        
        let platform_label = match state.target_platform {
            TargetPlatform::Windows => "Windows",
            TargetPlatform::Linux => "Linux",
            TargetPlatform::MacOS => "macOS",
            TargetPlatform::Web => "Web",
            TargetPlatform::Android => "Android",
            TargetPlatform::IOS => "iOS",
        };
        
        h_flex()
            .gap_1()
            .child(
                DropdownButton::new("build_config_dropdown")
                    .button(
                        Button::new("build_config_button")
                            .label(config_label)
                            .tooltip("Build Configuration")
                    )
                    .popup_menu(move |menu, _window, _cx| {
                        menu
                            .label("Build Configuration")
                            .separator()
                            .menu_with_icon("Debug (Fast Compile)", IconName::Bug, Box::new(NoOpAction))
                            .menu_with_icon("Release (Optimized)", IconName::Flash, Box::new(NoOpAction))
                            .menu_with_icon("Shipping (Final)", IconName::Package, Box::new(NoOpAction))
                    })
            )
            .child(
                DropdownButton::new("platform_dropdown")
                    .button(
                        Button::new("platform_button")
                            .label(platform_label)
                            .tooltip("Target Platform")
                    )
                    .popup_menu(move |menu, window, cx| {
                        menu
                            .label("Target Platform")
                            .separator()
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Computer)), "Windows", window, cx, |menu, _, _| {
                                menu
                                    .menu_with_icon("x86_64 (64-bit)", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("x86 (32-bit)", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("ARM64", IconName::CPU, Box::new(NoOpAction))
                            })
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Linux)), "Linux", window, cx, |menu, _, _| {
                                menu
                                    .menu_with_icon("x86_64", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("x86", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("ARM64", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("ARMv7", IconName::CPU, Box::new(NoOpAction))
                            })
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Apple)), "macOS", window, cx, |menu, _, _| {
                                menu
                                    .menu_with_icon("Apple Silicon (ARM64)", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("Intel (x86_64)", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("Universal", IconName::CPU, Box::new(NoOpAction))
                            })
                            .separator()
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Android)), "Android", window, cx, |menu, _, _| {
                                menu
                                    .menu_with_icon("ARM64-v8a", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("ARMv7", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("x86_64", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("x86", IconName::CPU, Box::new(NoOpAction))
                            })
                            .submenu_with_icon(Some(ui::Icon::new(IconName::Phone)), "iOS", window, cx, |menu, _, _| {
                                menu
                                    .menu_with_icon("ARM64 (Device)", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("ARM64 Simulator", IconName::CPU, Box::new(NoOpAction))
                                    .menu_with_icon("Universal", IconName::CPU, Box::new(NoOpAction))
                            })
                    })
            )
            .child(
                Button::new("build_deploy")
                    .icon(IconName::Package)
                    .tooltip("Build & Deploy")
                    .on_click(move |_, _, _| {
                        // TODO: Trigger build
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
            .tooltip("Toggle Performance Overlay")
            .selected(state.show_performance_overlay)
            .on_click(move |_, _, _| {
                state_clone.write().show_performance_overlay = !state_clone.read().show_performance_overlay;
            })
    }
}
