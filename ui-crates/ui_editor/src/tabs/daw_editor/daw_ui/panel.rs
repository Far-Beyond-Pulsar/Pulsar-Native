/// Main DAW Panel Component
/// Top-level container that assembles all UI components

use super::state::*;
use super::super::{audio_service::AudioService, audio_types::*};
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{v_flex, h_flex, StyledExt, ActiveTheme, PixelsExt, Sizable, Selectable, button::{Button, ButtonVariants}};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use crate::tabs::daw_editor::audio_types::SAMPLE_RATE;
use futures::channel::mpsc;
use futures::{SinkExt, StreamExt};

pub struct DawPanel {
    focus_handle: FocusHandle,
    pub state: DawUiState,
    /// Timeline element bounds for coordinate conversion (GPUI mouse events are window-relative)
    pub timeline_element_bounds: Option<gpui::Bounds<gpui::Pixels>>,
}

impl DawPanel {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            state: DawUiState::new(),
            timeline_element_bounds: None,
        }
    }

    pub fn load_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        match self.state.load_project(path) {
            Ok(_) => {
                eprintln!("✅ DAW: Project loaded successfully");

                // Sync loaded project to audio service
                self.sync_project_to_audio_service(cx);

                cx.notify();
            }
            Err(e) => {
                eprintln!("❌ DAW: Failed to load project: {}", e);
            }
        }
    }

    /// Sync the current project state to the audio service
    fn sync_project_to_audio_service(&self, cx: &mut Context<Self>) {
        if let (Some(ref project), Some(ref service)) = (&self.state.project, &self.state.audio_service) {
            let service = service.clone();
            let project = project.clone();

            cx.spawn(async move |_this, _cx| {
                eprintln!("🔄 Syncing project to audio service...");

                // Extract values first to avoid borrow issues
                let tempo = project.transport.tempo;
                let loop_enabled = project.transport.loop_enabled;
                let loop_start = project.transport.loop_start;
                let loop_end = project.transport.loop_end;
                let metronome_enabled = project.transport.metronome_enabled;
                let master_volume = project.master_track.volume;

                // Set tempo
                if let Err(e) = service.set_tempo(tempo).await {
                    eprintln!("❌ Failed to set tempo: {}", e);
                }

                // Set loop settings
                if let Err(e) = service.set_loop(
                    loop_enabled,
                    loop_start,
                    loop_end
                ).await {
                    eprintln!("❌ Failed to set loop: {}", e);
                }

                // Set metronome
                if let Err(e) = service.set_metronome(metronome_enabled).await {
                    eprintln!("❌ Failed to set metronome: {}", e);
                }

                // Add all tracks
                for track in &project.tracks {
                    let track_id = service.add_track(track.clone()).await;
                    eprintln!("  ✅ Added track: '{}' ({})", track.name, track_id);

                    // Sync track state
                    if let Err(e) = service.set_track_volume(track_id, track.volume).await {
                        eprintln!("    ❌ Failed to set volume: {}", e);
                    }
                    if let Err(e) = service.set_track_pan(track_id, track.pan).await {
                        eprintln!("    ❌ Failed to set pan: {}", e);
                    }
                    if let Err(e) = service.set_track_mute(track_id, track.muted).await {
                        eprintln!("    ❌ Failed to set mute: {}", e);
                    }
                    if let Err(e) = service.set_track_solo(track_id, track.solo).await {
                        eprintln!("    ❌ Failed to set solo: {}", e);
                    }
                }

                // Set master track volume
                if let Err(e) = service.set_master_volume(master_volume).await {
                    eprintln!("❌ Failed to set master volume: {}", e);
                }

                eprintln!("✅ Project sync complete");
            }).detach();
        }
    }

    pub fn save_project(&self, _path: PathBuf) -> anyhow::Result<()> {
        self.state.save_project()
    }

    pub fn new_project(&mut self, name: String, cx: &mut Context<Self>) {
        if let Some(ref project_dir) = self.state.project_dir {
            self.state.new_project(name, project_dir.clone());

            // Sync new project to audio service
            self.sync_project_to_audio_service(cx);

            cx.notify();
        }
    }

    pub fn set_audio_service(&mut self, service: Arc<AudioService>, cx: &mut Context<Self>) {
        self.state.audio_service = Some(service);

        // Sync existing project to audio service if one exists
        self.sync_project_to_audio_service(cx);

        // Start periodic playhead sync
        self.start_playhead_sync(cx);

        // Start periodic meter sync
        self.start_meter_sync(cx);
    }

    /// Start a periodic task to sync playhead position from audio service
    /// Uses GPUI's background executor to poll position without blocking UI
    fn start_playhead_sync(&self, cx: &mut Context<Self>) {
        if let Some(ref service) = self.state.audio_service {
            // Get a thread-safe position monitor
            let monitor = service.get_position_monitor();

            // Create channel for sending updates from background thread to UI
            let (tx, mut rx) = mpsc::unbounded();

            // Spawn background task using GPUI's background executor
            let tx_clone = tx.clone();
            cx.background_executor()
                .spawn(async move {
                    let mut tx = tx_clone;
                    loop {
                        // Use GPUI's Timer for async sleep
                        Timer::after(Duration::from_millis(50)).await;

                        let position = monitor.get_position();
                        let transport = monitor.get_transport();
                        let is_playing = transport.state == TransportState::Playing;

                        // Send to UI thread (use SinkExt trait)
                        if tx.send((position, is_playing)).await.is_err() {
                            break; // Channel closed
                        }
                    }
                })
                .detach();

            // Receive updates in UI thread
            cx.spawn(async move |this, cx| {
                while let Some((position, is_playing)) = rx.next().await {
                    cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            let tempo = this.state.get_tempo();
                            let seconds = position as f64 / SAMPLE_RATE as f64;
                            let beats = (seconds * tempo as f64) / 60.0;

                            this.state.selection.playhead_position = beats;
                            this.state.is_playing = is_playing;
                            cx.notify();
                        }).ok();
                    }).ok();
                }
            }).detach();

            eprintln!("✅ Playhead sync started with GPUI background executor");
        }
    }

    /// Start a periodic task to sync meter data from audio service
    /// Updates visual meters at 30 FPS for smooth visualization
    fn start_meter_sync(&self, cx: &mut Context<Self>) {
        if let Some(ref service) = self.state.audio_service {
            let service = service.clone();

            // Poll meters at 30 FPS (every ~33ms)
            cx.spawn(async move |this, cx| {
                loop {
                    Timer::after(Duration::from_millis(33)).await;

                    // Get meter data from audio service
                    let master_meter = service.get_master_meter().await;

                    // Get all track IDs first
                    let track_ids: Vec<TrackId> = cx.update(|cx| {
                        this.upgrade()
                            .and_then(|entity| entity.read(cx).state.project.as_ref()
                                .map(|p| p.tracks.iter().map(|t| t.id).collect()))
                            .unwrap_or_default()
                    }).ok().unwrap_or_default();

                    // Get meter data for all tracks
                    let mut track_meters = std::collections::HashMap::new();
                    for track_id in track_ids {
                        if let Some(meter) = service.get_track_meter(track_id).await {
                            track_meters.insert(track_id, meter);
                        }
                    }

                    // Update UI state
                    cx.update(|cx| {
                        this.update(cx, |this, cx| {
                            this.state.master_meter = master_meter;
                            this.state.track_meters = track_meters;
                            cx.notify();
                        }).ok();
                    }).ok();
                }
            }).detach();

            eprintln!("✅ Meter sync started at 30 FPS");
        }
    }

    /// Convert window-relative coordinates to timeline element coordinates
    /// Following GPUI best practices from gpui-mouse-position.md
    pub fn window_to_timeline_pos(window_pos: Point<Pixels>, panel: &Self) -> Point<Pixels> {
        if let Some(bounds) = &panel.timeline_element_bounds {
            Point::new(
                window_pos.x - bounds.origin.x,
                window_pos.y - bounds.origin.y,
            )
        } else {
            // Before first frame, bounds aren't captured yet
            window_pos
        }
    }
}

impl Focusable for DawPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DawPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("DawPanel")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .overflow_hidden()
            // Handle mouse move for dragging with proper coordinates
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                // Update drag visual feedback position and values
                match &this.state.drag_state.clone() {
                    DragState::DraggingFile { .. } => {
                        // Just trigger re-render for visual feedback
                        cx.notify();
                    }
                    DragState::DraggingClip { .. } => {
                        // Trigger re-render for clip drag visual feedback
                        cx.notify();
                    }
                    DragState::DraggingFader { track_id, start_mouse_y, start_volume } => {
                        // Update fader position based on mouse drag
                        let current_y = event.position.y.as_f32();
                        let delta_y = *start_mouse_y - current_y; // Inverted: up = increase
                        let delta_volume = delta_y / 100.0; // Sensitivity factor
                        let new_volume = (*start_volume + delta_volume).clamp(0.0, 1.5);

                        if let Some(ref mut project) = this.state.project {
                            // Handle master fader (nil UUID)
                            if track_id.is_nil() {
                                project.master_track.volume = new_volume;

                                // Sync to audio service in real-time
                                if let Some(ref service) = this.state.audio_service {
                                    let service = service.clone();
                                    cx.spawn(async move |_this, _cx| {
                                        let _ = service.set_master_volume(new_volume).await;
                                    }).detach();
                                }
                            } else if let Some(track) = project.tracks.iter_mut().find(|t| t.id == *track_id) {
                                track.volume = new_volume;

                                // Sync to audio service in real-time
                                if let Some(ref service) = this.state.audio_service {
                                    let service = service.clone();
                                    let track_id_val = *track_id;
                                    cx.spawn(async move |_this, _cx| {
                                        let _ = service.set_track_volume(track_id_val, new_volume).await;
                                    }).detach();
                                }
                            }
                        }
                        cx.notify();
                    }
                    DragState::DraggingPan { track_id, start_mouse_x, start_pan } => {
                        // Update pan position based on mouse drag
                        let current_x = event.position.x.as_f32();
                        let delta_x = current_x - *start_mouse_x;
                        let delta_pan = delta_x / 50.0; // Sensitivity factor
                        let new_pan = (*start_pan + delta_pan).clamp(-1.0, 1.0);

                        if let Some(ref mut project) = this.state.project {
                            if let Some(track) = project.tracks.iter_mut().find(|t| t.id == *track_id) {
                                track.pan = new_pan;

                                // Sync to audio service in real-time
                                if let Some(ref service) = this.state.audio_service {
                                    let service = service.clone();
                                    let track_id_val = *track_id;
                                    cx.spawn(async move |_this, _cx| {
                                        let _ = service.set_track_pan(track_id_val, new_pan).await;
                                    }).detach();
                                }
                            }
                        }
                        cx.notify();
                    }
                    DragState::DraggingTrackHeaderVolume { track_id, start_mouse_x, start_value } => {
                        // Update track header volume slider (horizontal)
                        let current_x = event.position.x;
                        let delta_px = current_x - *start_mouse_x;
                        let delta_value = delta_px / px(200.0); // Sensitivity factor (200 pixels = full range)
                        let new_value = (*start_value + delta_value).clamp(0.0, 1.0);

                        // Convert slider value (0..1) to dB then to linear
                        let db = (new_value * 72.0) - 60.0; // Map 0..1 to -60..+12 dB
                        let linear = 10f32.powf(db / 20.0);

                        if let Some(ref mut project) = this.state.project {
                            if let Some(track) = project.tracks.iter_mut().find(|t| t.id == *track_id) {
                                track.volume = linear.clamp(0.0, 2.0);

                                // Sync to audio service in real-time
                                if let Some(ref service) = this.state.audio_service {
                                    let service = service.clone();
                                    let track_id_val = *track_id;
                                    let volume = track.volume;
                                    cx.spawn(async move |_this, _cx| {
                                        let _ = service.set_track_volume(track_id_val, volume).await;
                                    }).detach();
                                }
                            }
                        }
                        cx.notify();
                    }
                    DragState::DraggingTrackHeaderPan { track_id, start_mouse_x, start_value } => {
                        // Update track header pan slider (horizontal)
                        let current_x = event.position.x;
                        let delta_px = current_x - *start_mouse_x;
                        let delta_value = delta_px / px(100.0); // Sensitivity factor (100 pixels = full range)
                        let new_value = (*start_value + delta_value).clamp(0.0, 1.0);

                        // Convert slider value (0..1) to pan (-1..1)
                        let pan = (new_value * 2.0 - 1.0) as f32;

                        if let Some(ref mut project) = this.state.project {
                            if let Some(track) = project.tracks.iter_mut().find(|t| t.id == *track_id) {
                                track.pan = pan.clamp(-1.0, 1.0);

                                // Sync to audio service in real-time
                                if let Some(ref service) = this.state.audio_service {
                                    let service = service.clone();
                                    let track_id_val = *track_id;
                                    cx.spawn(async move |_this, _cx| {
                                        let _ = service.set_track_pan(track_id_val, pan).await;
                                    }).detach();
                                }
                            }
                        }
                        cx.notify();
                    }
                    DragState::DraggingSend { track_id, send_idx, start_mouse_x, start_amount } => {
                        // Update send level (horizontal drag)
                        let current_x = event.position.x.as_f32();
                        let delta_x = current_x - *start_mouse_x;
                        let delta_amount = delta_x / 100.0; // Sensitivity: 100 pixels = full range (0.0 to 1.0)
                        let new_amount = (*start_amount + delta_amount).clamp(0.0, 1.0);

                        if let Some(ref mut project) = this.state.project {
                            if let Some(track) = project.tracks.iter_mut().find(|t| t.id == *track_id) {
                                // Ensure send exists
                                while track.sends.len() <= *send_idx {
                                    track.sends.push(super::super::audio_types::Send {
                                        target_track: None,
                                        amount: 0.0,
                                        pre_fader: false,
                                        enabled: false,
                                    });
                                }
                                if let Some(send) = track.sends.get_mut(*send_idx) {
                                    send.amount = new_amount;
                                }
                            }
                        }
                        cx.notify();
                    }
                    _ => {}
                }
            }))
            // Handle mouse up to clear drag state
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                // Sync changes to audio service when drag completes
                match &this.state.drag_state {
                    DragState::DraggingFader { .. } => {
                        // Volume already synced in real-time during drag
                    }
                    DragState::DraggingPan { .. } => {
                        // Pan already synced in real-time during drag
                    }
                    DragState::DraggingTrackHeaderVolume { .. } => {
                        // Volume already synced in real-time during drag
                    }
                    DragState::DraggingTrackHeaderPan { .. } => {
                        // Pan already synced in real-time during drag
                    }
                    DragState::DraggingSend { track_id, send_idx, .. } => {
                        let track_id_val = *track_id;
                        let send_idx_val = *send_idx;
                        if let Some(ref service) = this.state.audio_service {
                            let service = service.clone();
                            let send_amount = this.state.project.as_ref()
                                .and_then(|p| p.tracks.iter().find(|t| t.id == track_id_val))
                                .and_then(|t| t.sends.get(send_idx_val))
                                .map(|s| s.amount)
                                .unwrap_or(0.0);

                            // Future: Sync send levels to audio service
                            eprintln!("🎚️ Send {} level set to: {:.0}%", send_idx_val, send_amount * 100.0);
                            // let _ = service.set_send_level(track_id_val, send_idx_val, send_amount).await;
                        }
                    }
                    DragState::DraggingFile { .. } => {
                        // File drop is handled by timeline drop zones
                        // Don't clear it here
                    }
                    _ => {}
                }

                // Clear drag state when mouse is released (except for DraggingFile which is handled by drop zones)
                if !matches!(this.state.drag_state, DragState::None | DragState::DraggingFile { .. }) {
                    this.state.drag_state = DragState::None;
                    cx.notify();
                }
            }))
            .child(self.render_content(cx))
            // Render drag cursor overlay
            .child(self.render_drag_cursor(cx))
    }
}

impl DawPanel {
    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_0()
            // Toolbar
            .child(self.render_toolbar(cx))
            // Transport controls
            .child(self.render_transport(cx))
            // Main content area
            .child(
                h_flex()
                    .flex_1()
                    .overflow_hidden()
                    .gap_0()
                    // Left sidebar (browser)
                    .when(self.state.show_browser, |this| {
                        this.child(self.render_browser(cx))
                    })
                    // Center content (timeline/mixer)
                    .child(self.render_main_area(cx))
                    // Right sidebar (inspector)
                    .when(self.state.show_inspector, |this| {
                        this.child(self.render_inspector(cx))
                    })
            )
    }

    // Toolbar implementation
    pub fn render_toolbar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        super::toolbar::render_toolbar(&mut self.state, cx)
    }

    pub fn render_transport(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        super::transport::render_transport(&mut self.state, cx)
    }

    fn render_main_area(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .h_full()
            .min_w_0()  // Allow shrinking below content width
            .overflow_hidden()  // Prevent content overflow
            .bg(cx.theme().background)
            .gap_0()
            // Main content area (timeline/editor) - takes up most of the space
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .min_w_0()  // Allow shrinking
                    .overflow_hidden()
                    .child(match self.state.view_mode {
                        ViewMode::Arrange => self.render_timeline(cx).into_any_element(),
                        ViewMode::Mix => div().child("Full Mix View").into_any_element(),
                        ViewMode::Edit => self.render_clip_editor(cx).into_any_element(),
                    })
            )
            // Mixer panel at the bottom - fixed height
            .when(self.state.show_mixer, |this| {
                this.child(
                    div()
                        .w_full()
                        .h(px(420.0)).flex_shrink_0()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(self.render_mixer(cx))
                )
            })
    }

    fn render_inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        use ui::{button::*, Sizable};

        let selected_track_id = self.state.selection.selected_track_ids.iter().next().copied();

        v_flex()
            .w(px(300.0))
            .h_full()
            .bg(cx.theme().background)
            .border_l_1()
            .border_color(cx.theme().border)
            // Tab bar
            .child(
                h_flex()
                    .w_full()
                    .h(px(44.0))
                    .px_2()
                    .gap_1()
                    .items_center()
                    .bg(cx.theme().muted.opacity(0.2))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(self.render_inspector_tab_button("Track", InspectorTab::Track, cx))
                    .child(self.render_inspector_tab_button("Clip", InspectorTab::Clip, cx))
                    .child(self.render_inspector_tab_button("Auto", InspectorTab::Automation, cx))
                    .child(self.render_inspector_tab_button("FX", InspectorTab::Effects, cx))
            )
            // Content area
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .p_4()
                    .child(match self.state.inspector_tab {
                        InspectorTab::Track => self.render_track_inspector(selected_track_id, cx).into_any_element(),
                        InspectorTab::Clip => self.render_empty_inspector_message("Clip Inspector", "Select a clip to view and edit its properties", cx).into_any_element(),
                        InspectorTab::Automation => self.render_empty_inspector_message("Automation", "Draw automation curves on the timeline to control parameters over time", cx).into_any_element(),
                        InspectorTab::Effects => self.render_empty_inspector_message("Effects", "Add audio effects and processors to track inserts", cx).into_any_element(),
                    })
            )
    }

    fn render_track_inspector(&mut self, track_id: Option<TrackId>, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(track) = track_id.and_then(|id| self.state.get_track(id)) {
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
                        .child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Type")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().foreground)
                                        .child(format!("{:?}", track.track_type))
                                )
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Volume")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family("monospace")
                                        .text_color(cx.theme().foreground)
                                        .child(format!("{:+.1} dB", track.volume_db()))
                                )
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Pan")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_family("monospace")
                                        .text_color(cx.theme().foreground)
                                        .child(format!("{:.0}%", track.pan * 100.0))
                                )
                        )
                        .child(
                            v_flex()
                                .w_full()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Clips")
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().foreground)
                                        .child(format!("{} clips", track.clips.len()))
                                )
                        )
                )
                .into_any_element()
        } else {
            self.render_empty_inspector_message("Track Inspector", "Select a track to view and edit its properties", cx)
                .into_any_element()
        }
    }

    fn render_inspector_tab_button(&self, label: &str, tab: InspectorTab, cx: &mut Context<Self>) -> impl IntoElement {
        let is_active = self.state.inspector_tab == tab;
        let label = label.to_string();

        Button::new(ElementId::Name(format!("inspector-tab-{:?}", tab).into()))
            .label(label)
            .ghost()
            .compact()
            .small()
            .when(is_active, |btn| btn.selected(true))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.inspector_tab = tab;
                cx.notify();
            }))
    }

    fn render_empty_inspector_message(&self, title: &str, description: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let title = title.to_string();
        let description = description.to_string();

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
                    .child(title)
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                    .text_center()
                    .max_w(px(220.0))
                    .line_height(rems(1.5))
                    .child(description)
            )
    }

    pub fn render_timeline(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        super::timeline::render_timeline(&mut self.state, cx)
    }

    pub fn render_mixer(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        super::mixer::render_mixer(&mut self.state, cx)
    }

    pub fn render_browser(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        super::browser::render_browser(&mut self.state, cx)
    }

    fn render_clip_editor(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child("Clip Editor Placeholder")
    }

    fn render_drag_cursor(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        match &self.state.drag_state {
            DragState::DraggingFile { file_name, .. } => {
                div()
                    .absolute()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(cx.theme().accent.opacity(0.9))
                            .border_1()
                            .border_color(cx.theme().accent)
                            .shadow_lg()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        ui::Icon::new(ui::IconName::MusicNote)
                                            .size_4()
                                            .text_color(cx.theme().accent_foreground)
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .text_color(cx.theme().accent_foreground)
                                            .child(file_name.clone())
                                    )
                            )
                    )
                    .into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    /// Convert element coordinates to timeline coordinates (accounting for scroll and zoom)
    pub fn element_to_timeline_coords(
        element_pos: Point<Pixels>,
        viewport: &ViewportState,
    ) -> Point<f32> {
        Point::new(
            element_pos.x.as_f32() + viewport.scroll_x as f32,
            element_pos.y.as_f32() + viewport.scroll_y as f32,
        )
    }

    /// Convert timeline coordinates to element coordinates
    pub fn timeline_to_element_coords(
        timeline_pos: Point<f32>,
        viewport: &ViewportState,
    ) -> Point<Pixels> {
        Point::new(
            px(timeline_pos.x - viewport.scroll_x as f32),
            px(timeline_pos.y - viewport.scroll_y as f32),
        )
    }

    /// One-shot conversion: window → element → timeline
    pub fn window_to_timeline_coords(
        window_pos: Point<Pixels>,
        panel: &DawPanel,
        viewport: &ViewportState,
    ) -> Point<f32> {
        let element_pos = Self::window_to_timeline_pos(window_pos, panel);
        Self::element_to_timeline_coords(element_pos, viewport)
    }
}

