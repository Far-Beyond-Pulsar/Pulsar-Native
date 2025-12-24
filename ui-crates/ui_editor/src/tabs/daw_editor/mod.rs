/// Complete embedded DAW engine for Pulsar game engine
/// 
/// This module provides a production-ready Digital Audio Workstation (DAW) engine
/// with real-time multi-track mixing, sample-accurate automation, GPU-accelerated DSP,
/// and a complete GPUI-based user interface.

mod asset_manager;
mod audio_graph;
mod audio_service;
mod audio_types;
mod ecs_integration;
mod gpu_dsp;
mod project;
mod real_time_audio;
mod daw_ui;
mod workspace_panels;

pub use audio_service::AudioService;
pub use daw_ui::DawPanel;
pub use workspace_panels::*;

use gpui::*;
use ui::dock::{Panel, PanelEvent, DockArea, DockItem, DockPlacement};
use ui::workspace::Workspace;
use std::path::PathBuf;
use std::sync::Arc;
use parking_lot::RwLock;
use daw_ui::state::DawUiState;

/// Main DAW Editor Panel that integrates with the engine's panel system
pub struct DawEditorPanel {
    focus_handle: FocusHandle,
    workspace: Entity<Workspace>,
    state: Arc<RwLock<daw_ui::state::DawUiState>>,
    daw_panel: Entity<DawPanel>,
    project_path: Option<PathBuf>,
    audio_service: Option<Arc<AudioService>>,
}

impl DawEditorPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = Arc::new(RwLock::new(DawUiState::new()));

        // Create workspace with Channel 2 to isolate DAW tabs from other editors
        let workspace = cx.new(|cx| {
            use ui::dock::DockChannel;
            Workspace::new_with_channel(
                "daw-workspace",
                DockChannel(2),
                window,
                cx
            )
        });

        // Create shared DawPanel that will be used by all workspace panels
        let daw_panel = cx.new(|cx| DawPanel::new(window, cx));

        let mut panel = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            state: state.clone(),
            daw_panel: daw_panel.clone(),
            project_path: None,
            audio_service: None,
        };

        // Initialize workspace layout
        panel.setup_workspace(window, cx);

        // Initialize audio service
        panel.initialize_audio_service(window, cx);

        panel
    }

    pub fn new_with_project(
        project_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(window, cx);
        panel.load_project(project_path, window, cx);
        panel
    }
    
    fn setup_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let daw_panel = self.daw_panel.clone();

        // Create browser tab panels - all share the same DawPanel
        let browser_files = cx.new(|cx| workspace_panels::BrowserFilesPanel::new(state.clone(), daw_panel.clone(), window, cx));
        let browser_instruments = cx.new(|cx| workspace_panels::BrowserInstrumentsPanel::new(state.clone(), daw_panel.clone(), window, cx));
        let browser_effects = cx.new(|cx| workspace_panels::BrowserEffectsPanel::new(state.clone(), daw_panel.clone(), window, cx));
        let browser_loops = cx.new(|cx| workspace_panels::BrowserLoopsPanel::new(state.clone(), daw_panel.clone(), window, cx));
        let browser_samples = cx.new(|cx| workspace_panels::BrowserSamplesPanel::new(state.clone(), daw_panel.clone(), window, cx));

        // Create main panels - all share the same DawPanel
        let timeline_panel = cx.new(|cx| workspace_panels::TimelinePanel::new(state.clone(), daw_panel.clone(), window, cx));
        let mixer_panel = cx.new(|cx| workspace_panels::MixerPanel::new(state.clone(), daw_panel.clone(), window, cx));
        let inspector_panel = cx.new(|cx| workspace_panels::InspectorPanel::new(state.clone(), window, cx));
        
        self.workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area();
            let weak_dock = dock_area.downgrade();
            
            // Create center area: vertical split with timeline and mixer
            let center = DockItem::split(
                gpui::Axis::Vertical,
                vec![
                    // Timeline takes most space (includes toolbar and transport at top)
                    DockItem::tab(timeline_panel.clone(), &weak_dock, window, cx),
                    // Mixer at bottom
                    DockItem::tab(mixer_panel.clone(), &weak_dock, window, cx),
                ],
                &weak_dock,
                window,
                cx,
            );
            
            // Create left sidebar: split vertically
            // Top half: Files and Samples tabs
            let left_top = DockItem::tabs(
                vec![
                    Arc::new(browser_files) as Arc<dyn ui::dock::PanelView>,
                    Arc::new(browser_samples) as Arc<dyn ui::dock::PanelView>,
                ],
                Some(0), // Start with Files tab selected
                &weak_dock,
                window,
                cx,
            );

            // Bottom half: Instruments and Effects tabs
            let left_bottom = DockItem::tabs(
                vec![
                    Arc::new(browser_instruments) as Arc<dyn ui::dock::PanelView>,
                    Arc::new(browser_effects) as Arc<dyn ui::dock::PanelView>,
                ],
                Some(0), // Start with Instruments tab selected
                &weak_dock,
                window,
                cx,
            );

            let left = DockItem::split(
                gpui::Axis::Vertical,
                vec![left_top, left_bottom],
                &weak_dock,
                window,
                cx,
            );

            // Create right sidebar: split vertically
            // Top half: Inspector
            let right_top = DockItem::tab(inspector_panel.clone(), &weak_dock, window, cx);

            // Bottom half: Loops
            let right_bottom = DockItem::tab(browser_loops.clone(), &weak_dock, window, cx);

            let right = DockItem::split(
                gpui::Axis::Vertical,
                vec![right_top, right_bottom],
                &weak_dock,
                window,
                cx,
            );

            // Initialize workspace with custom dock sizes
            // Set center first
            dock_area.update(cx, |dock_area, cx| {
                dock_area.set_center(center, window, cx);

                // Set left dock with custom width (400px)
                dock_area.set_left_dock(left, Some(px(400.0)), true, window, cx);

                // Set right dock with custom width (400px)
                dock_area.set_right_dock(right, Some(px(400.0)), true, window, cx);
            });
        });
    }

    fn initialize_audio_service(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.clone();
        let daw_panel = self.daw_panel.clone();

        cx.spawn(async move |this, cx| {
            match AudioService::new().await {
                Ok(service) => {
                    let service = Arc::new(service);

                    cx.update(|cx| {
                        // Set audio service on DawEditorPanel
                        this.update(cx, |this, cx| {
                            this.audio_service = Some(service.clone());

                            // Set audio service on shared state
                            this.state.write().audio_service = Some(service.clone());

                            // Set audio service on DawPanel's state
                            this.daw_panel.update(cx, |panel, _cx| {
                                panel.state.audio_service = Some(service.clone());
                            });

                            // Start playhead and meter sync
                            this.start_playhead_sync(cx);
                            this.start_meter_sync(cx);

                            cx.notify();
                        }).ok();
                    }).ok();
                }
                Err(e) => {
                    eprintln!("❌ Failed to initialize audio service: {}", e);
                }
            }
        })
        .detach();
    }

    pub fn load_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.project_path = Some(path.clone());

        match self.state.write().load_project(path.clone()) {
            Ok(_) => {
                eprintln!("✅ DAW: Project loaded successfully");

                // Sync project to DawPanel's state
                self.daw_panel.update(cx, |panel, _cx| {
                    let _ = panel.state.load_project(path);
                });

                self.sync_project_to_audio_service(cx);
                cx.notify();
            }
            Err(e) => {
                eprintln!("❌ DAW: Failed to load project: {}", e);
            }
        }

        if self.audio_service.is_none() {
            self.initialize_audio_service(window, cx);
        }
    }

    pub fn save_project(&self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        if let Some(path) = &self.project_path {
            self.state.read().save_project()?;
        }
        Ok(())
    }

    pub fn new_project(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ref project_dir) = self.state.read().project_dir {
            let project_dir = project_dir.clone();
            self.state.write().new_project(name.clone(), project_dir.clone());

            // Sync to DawPanel's state
            self.daw_panel.update(cx, |panel, _cx| {
                panel.state.new_project(name, project_dir);
            });

            self.sync_project_to_audio_service(cx);
            cx.notify();
        }

        if self.audio_service.is_none() {
            self.initialize_audio_service(window, cx);
        }
    }

    pub fn get_audio_service(&self) -> Option<Arc<AudioService>> {
        self.audio_service.clone()
    }
    
    fn sync_project_to_audio_service(&self, cx: &mut Context<Self>) {
        let state_clone = self.state.clone();
        
        if let Some(ref service) = self.audio_service {
            let service = service.clone();
            
            cx.spawn(async move |_this, _cx| {
                let state = state_clone.read();
                
                if let Some(ref project) = state.project {
                    eprintln!("🔄 Syncing project to audio service...");

                    let tempo = project.transport.tempo;
                    let loop_enabled = project.transport.loop_enabled;
                    let loop_start = project.transport.loop_start;
                    let loop_end = project.transport.loop_end;
                    let metronome_enabled = project.transport.metronome_enabled;
                    let master_volume = project.master_track.volume;

                    if let Err(e) = service.set_tempo(tempo).await {
                        eprintln!("❌ Failed to set tempo: {}", e);
                    }

                    if let Err(e) = service.set_loop(loop_enabled, loop_start, loop_end).await {
                        eprintln!("❌ Failed to set loop: {}", e);
                    }

                    if let Err(e) = service.set_metronome(metronome_enabled).await {
                        eprintln!("❌ Failed to set metronome: {}", e);
                    }

                    for track in &project.tracks {
                        let track_id = service.add_track(track.clone()).await;
                        eprintln!("  ✅ Added track: '{}' ({})", track.name, track_id);

                        let _ = service.set_track_volume(track_id, track.volume).await;
                        let _ = service.set_track_pan(track_id, track.pan).await;
                        let _ = service.set_track_mute(track_id, track.muted).await;
                        let _ = service.set_track_solo(track_id, track.solo).await;
                    }

                    if let Err(e) = service.set_master_volume(master_volume).await {
                        eprintln!("❌ Failed to set master volume: {}", e);
                    }

                    eprintln!("✅ Project sync complete");
                }
            }).detach();
        }
    }
    
    fn start_playhead_sync(&self, cx: &mut Context<Self>) {
        if let Some(ref service) = self.audio_service {
            use futures::channel::mpsc;
            use futures::{SinkExt, StreamExt};
            use std::time::Duration;
            
            let monitor = service.get_position_monitor();
            let (tx, mut rx) = mpsc::unbounded();

            let tx_clone = tx.clone();
            cx.background_executor()
                .spawn(async move {
                    let mut tx = tx_clone;
                    loop {
                        Timer::after(Duration::from_millis(50)).await;
                        let position = monitor.get_position();
                        let transport = monitor.get_transport();
                        let is_playing = transport.state == audio_types::TransportState::Playing;

                        if tx.send((position, is_playing)).await.is_err() {
                            break;
                        }
                    }
                })
                .detach();

            let state = self.state.clone();
            cx.spawn(async move |_this, cx| {
                while let Some((position, is_playing)) = rx.next().await {
                    cx.update(|cx| {
                        let mut state = state.write();
                        let tempo = state.get_tempo();
                        let seconds = position as f64 / audio_types::SAMPLE_RATE as f64;
                        let beats = (seconds * tempo as f64) / 60.0;

                        state.selection.playhead_position = beats;
                        state.is_playing = is_playing;
                    }).ok();
                }
            }).detach();

            eprintln!("✅ Playhead sync started");
        }
    }

    fn start_meter_sync(&self, cx: &mut Context<Self>) {
        if let Some(ref service) = self.audio_service {
            use std::time::Duration;
            
            let service = service.clone();
            let state = self.state.clone();

            cx.spawn(async move |_this, cx| {
                loop {
                    Timer::after(Duration::from_millis(33)).await;

                    let master_meter = service.get_master_meter().await;

                    let track_ids: Vec<audio_types::TrackId> = cx.update(|cx| {
                        state.read().project.as_ref()
                            .map(|p| p.tracks.iter().map(|t| t.id).collect())
                            .unwrap_or_default()
                    }).ok().unwrap_or_default();

                    let mut track_meters = std::collections::HashMap::new();
                    for track_id in track_ids {
                        if let Some(meter) = service.get_track_meter(track_id).await {
                            track_meters.insert(track_id, meter);
                        }
                    }

                    cx.update(|cx| {
                        let mut state = state.write();
                        state.master_meter = master_meter;
                        state.track_meters = track_meters;
                    }).ok();
                }
            }).detach();

            eprintln!("✅ Meter sync started at 30 FPS");
        }
    }
}

impl Panel for DawEditorPanel {
    fn panel_name(&self) -> &'static str {
        "DAW Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div()
            .child(
                if let Some(path) = &self.project_path {
                    format!(
                        "DAW - {}",
                        path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Untitled")
                    )
                } else {
                    "DAW Editor".to_string()
                }
            )
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        let info = self.project_path.as_ref().map(|p| {
            serde_json::json!({
                "project_path": p.to_string_lossy().to_string()
            })
        }).unwrap_or(serde_json::Value::Null);

        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            info: ui::dock::PanelInfo::Panel(info),
            ..Default::default()
        }
    }
}

impl Focusable for DawEditorPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for DawEditorPanel {}

impl Render for DawEditorPanel {
    fn render(&mut self, _: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.workspace.clone()
    }
}