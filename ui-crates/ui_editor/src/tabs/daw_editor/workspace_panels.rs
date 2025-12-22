//! Workspace panels for DAW Editor
//!
//! This module provides workspace panels that integrate with the unified workspace system,
//! similar to level_editor and script_editor.

use gpui::*;
use ui::{ActiveTheme, StyledExt, dock::{Panel, PanelEvent}, v_flex, h_flex};
use super::daw_ui::state::{DawUiState, BrowserTab, InspectorTab, ViewMode};
use super::audio_types::*;
use std::sync::Arc;
use parking_lot::RwLock;

/// Browser Panel - left sidebar with files, instruments, effects, loops
pub struct BrowserPanel {
    state: Arc<RwLock<DawUiState>>,
    focus_handle: FocusHandle,
}

impl BrowserPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for BrowserPanel {}

impl Render for BrowserPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Render browser content directly instead of calling the render function
                self.render_browser_content(&*state, cx)
            )
    }
}

impl BrowserPanel {
    fn render_browser_content(&self, state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{
            button::*, Icon, IconName, Sizable, Selectable, divider::Divider,
        };
        
        v_flex()
            .w_full()
            .h_full()
            .gap_0()
            // Tab bar
            .child(self.render_browser_tabs(state, cx))
            // Search bar
            .child(self.render_search_bar(state, cx))
            // Content area - placeholder for now
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .p_4()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Browser - {:?} tab", state.browser_tab))
                    )
            )
    }
    
    fn render_browser_tabs(&self, state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        
        h_flex()
            .w_full()
            .h(px(44.0))
            .px_2()
            .gap_1()
            .items_center()
            .bg(cx.theme().muted.opacity(0.2))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.render_tab_button("Files", BrowserTab::Files, state.browser_tab == BrowserTab::Files, cx))
            .child(self.render_tab_button("Instruments", BrowserTab::Instruments, state.browser_tab == BrowserTab::Instruments, cx))
            .child(self.render_tab_button("FX", BrowserTab::Effects, state.browser_tab == BrowserTab::Effects, cx))
            .child(self.render_tab_button("Loops", BrowserTab::Loops, state.browser_tab == BrowserTab::Loops, cx))
    }
    
    fn render_tab_button(&self, label: &str, tab: BrowserTab, is_active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        use ui::Selectable;
        
        let mut btn = Button::new(ElementId::Name(format!("browser-tab-{:?}", tab).into()))
            .label(label.to_string())
            .ghost()
            .compact();
        
        if is_active {
            btn = btn.selected(true);
        }
        
        btn.on_click(cx.listener(move |this, _, _, cx| {
            this.state.write().browser_tab = tab;
            cx.notify();
        }))
    }
    
    fn render_search_bar(&self, _state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .px_3()
            .py_3()
            .bg(cx.theme().muted.opacity(0.1))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .w_full()
                    .h(px(36.0))
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Search files...")
                    )
            )
    }
}

impl Focusable for BrowserPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for BrowserPanel {
    fn panel_name(&self) -> &'static str {
        "browser"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Browser".into_any_element()
    }
}

/// Timeline Panel - main arrangement view with tracks and clips
pub struct TimelinePanel {
    state: Arc<RwLock<DawUiState>>,
    focus_handle: FocusHandle,
}

impl TimelinePanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for TimelinePanel {}

impl Render for TimelinePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .gap_0()
            // Toolbar at top
            .child(self.render_toolbar(&*state, cx))
            // Transport controls below toolbar
            .child(self.render_transport(&*state, cx))
            // Main timeline content
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_xl()
                            .text_color(cx.theme().foreground)
                            .child("Timeline / Arrangement View")
                    )
            )
            // Status bar at bottom
            .child(
                div()
                    .w_full()
                    .p_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.1))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Tracks: {} | Playhead: {:.2} beats", 
                                state.project.as_ref().map(|p| p.tracks.len()).unwrap_or(0),
                                state.selection.playhead_position))
                    )
            )
    }
}

impl TimelinePanel {
    fn render_toolbar(&self, state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        use ui::Selectable;
        
        h_flex()
            .w_full()
            .h(px(48.0))
            .px_4()
            .gap_3()
            .items_center()
            .bg(cx.theme().muted.opacity(0.3))
            .border_b_1()
            .border_color(cx.theme().border)
            // View mode buttons
            .child(self.render_view_modes(state, cx))
            .child(div().flex_1())
            // Project name
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(cx.theme().foreground)
                    .child(
                        state.project.as_ref()
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| "Untitled Project".to_string())
                    )
            )
    }
    
    fn render_view_modes(&self, state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        
        h_flex()
            .gap_1()
            .items_center()
            .child(self.render_view_button("Arrange", ViewMode::Arrange, state.view_mode == ViewMode::Arrange, cx))
            .child(self.render_view_button("Mix", ViewMode::Mix, state.view_mode == ViewMode::Mix, cx))
            .child(self.render_view_button("Edit", ViewMode::Edit, state.view_mode == ViewMode::Edit, cx))
    }
    
    fn render_view_button(&self, label: &str, mode: ViewMode, is_active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        use ui::Selectable;
        
        let mut btn = Button::new(ElementId::Name(format!("view-mode-{:?}", mode).into()))
            .label(label.to_string())
            .ghost()
            .compact();
        
        if is_active {
            btn = btn.selected(true);
        }
        
        btn.on_click(cx.listener(move |this, _, _, cx| {
            this.state.write().view_mode = mode;
            cx.notify();
        }))
    }
    
    fn render_transport(&self, state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(60.0))
            .px_4()
            .gap_3()
            .items_center()
            .bg(cx.theme().muted.opacity(0.15))
            .border_b_1()
            .border_color(cx.theme().border)
            // Play button
            .child(self.render_play_button(state, cx))
            // Stop button
            .child(self.render_stop_button(state, cx))
            // Position display
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .font_family("monospace")
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(format!("{:.2} beats", state.selection.playhead_position))
                    )
            )
            .child(div().flex_1())
            // Tempo
            .child(
                div()
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .font_family("monospace")
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .child(format!("{:.1} BPM", state.get_tempo()))
                    )
            )
    }
    
    fn render_play_button(&self, state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        use ui::{Icon, IconName};
        
        Button::new(ElementId::Name("transport-play".into()))
            .icon(Icon::new(if state.is_playing { IconName::Pause } else { IconName::Play }))
            .primary()
            .compact()
            .on_click(cx.listener(move |this, _, _, cx| {
                let mut state = this.state.write();
                state.is_playing = !state.is_playing;
                
                if let Some(ref service) = state.audio_service {
                    let service = service.clone();
                    let playing = state.is_playing;
                    cx.spawn(async move |_, _| {
                        if playing {
                            let _ = service.play().await;
                        } else {
                            let _ = service.pause().await;
                        }
                    }).detach();
                }
                cx.notify();
            }))
    }
    
    fn render_stop_button(&self, _state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        use ui::{Icon, IconName};
        
        Button::new(ElementId::Name("transport-stop".into()))
            .icon(Icon::new(IconName::Square))
            .ghost()
            .compact()
            .on_click(cx.listener(move |this, _, _, cx| {
                let mut state = this.state.write();
                state.is_playing = false;
                state.selection.playhead_position = 0.0;
                
                if let Some(ref service) = state.audio_service {
                    let service = service.clone();
                    cx.spawn(async move |_, _| {
                        let _ = service.stop().await;
                    }).detach();
                }
                cx.notify();
            }))
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
pub struct MixerPanel {
    state: Arc<RwLock<DawUiState>>,
    focus_handle: FocusHandle,
}

impl MixerPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for MixerPanel {}

impl Render for MixerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read();
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_xl()
                            .text_color(cx.theme().foreground)
                            .child("Mixer View")
                    )
            )
            .child(
                div()
                    .w_full()
                    .p_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.1))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("Channels: {}", 
                                state.project.as_ref().map(|p| p.tracks.len()).unwrap_or(0)))
                    )
            )
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

/// Inspector Panel - right sidebar showing track, clip, automation, effects details
pub struct InspectorPanel {
    state: Arc<RwLock<DawUiState>>,
    focus_handle: FocusHandle,
}

impl InspectorPanel {
    pub fn new(state: Arc<RwLock<DawUiState>>, cx: &mut Context<Self>) -> Self {
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
        let selected_track_id = state.selection.selected_track_ids.iter().next().copied();
        
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            // Tab bar
            .child(self.render_tab_bar(&*state, cx))
            // Content area
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .p_4()
                    .child(match state.inspector_tab {
                        InspectorTab::Track => self.render_track_inspector(selected_track_id, &*state, cx),
                        InspectorTab::Clip => self.render_empty_message("Clip Inspector", "Select a clip to view properties", cx),
                        InspectorTab::Automation => self.render_empty_message("Automation", "Draw curves on timeline", cx),
                        InspectorTab::Effects => self.render_empty_message("Effects", "Add audio effects to track", cx),
                    })
            )
    }
}

impl InspectorPanel {
    fn render_tab_bar(&self, state: &DawUiState, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        
        h_flex()
            .w_full()
            .h(px(44.0))
            .px_2()
            .gap_1()
            .items_center()
            .bg(cx.theme().muted.opacity(0.2))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.render_tab_button("Track", InspectorTab::Track, state.inspector_tab == InspectorTab::Track, cx))
            .child(self.render_tab_button("Clip", InspectorTab::Clip, state.inspector_tab == InspectorTab::Clip, cx))
            .child(self.render_tab_button("Auto", InspectorTab::Automation, state.inspector_tab == InspectorTab::Automation, cx))
            .child(self.render_tab_button("FX", InspectorTab::Effects, state.inspector_tab == InspectorTab::Effects, cx))
    }

    fn render_tab_button(&self, label: &str, tab: InspectorTab, is_active: bool, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::button::*;
        use ui::Selectable;
        
        let mut btn = Button::new(ElementId::Name(format!("inspector-tab-{:?}", tab).into()))
            .label(label.to_string())
            .ghost()
            .compact();
        
        if is_active {
            btn = btn.selected(true);
        }
        
        btn.on_click(cx.listener(move |this, _, _, cx| {
            this.state.write().inspector_tab = tab;
            cx.notify();
        }))
    }

    fn render_track_inspector(&self, track_id: Option<TrackId>, state: &DawUiState, cx: &mut Context<Self>) -> Div {
        if let Some(track) = track_id.and_then(|id| state.get_track(id)) {
            v_flex()
                .w_full()
                .gap_3()
                .child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("TRACK NAME")
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child(track.name.clone())
                        )
                )
                .child(
                    div()
                        .w_full()
                        .h(px(1.0))
                        .bg(cx.theme().border)
                )
                .child(
                    v_flex()
                        .w_full()
                        .gap_3()
                        .child(self.render_property("Type", format!("{:?}", track.track_type), cx))
                        .child(self.render_property("Volume", format!("{:+.1} dB", track.volume_db()), cx))
                        .child(self.render_property("Pan", format!("{:.0}%", track.pan * 100.0), cx))
                        .child(self.render_property("Clips", format!("{} clips", track.clips.len()), cx))
                )
        } else {
            self.render_empty_message("Track Inspector", "Select a track to view properties", cx)
        }
    }

    fn render_property(&self, label: &str, value: String, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string())
            )
            .child(
                div()
                    .text_sm()
                    .font_family("monospace")
                    .text_color(cx.theme().foreground)
                    .child(value)
            )
    }

    fn render_empty_message(&self, title: &str, description: &str, cx: &mut Context<Self>) -> Div {
        v_flex()
            .w_full()
            .h_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.to_string())
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                    .text_center()
                    .max_w(px(220.0))
                    .line_height(rems(1.5))
                    .child(description.to_string())
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
