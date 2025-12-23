/// DAW Panel State Management
/// Central state for all DAW UI components

use super::super::{audio_service::AudioService, audio_types::*, project::DawProject};
use gpui::*;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::tabs::daw_editor::audio_types::{SAMPLE_RATE, AudioClip, AudioAssetData};
use ui::{VirtualListScrollHandle, scroll::ScrollbarState};

/// Main view modes
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ViewMode {
    Arrange,
    Mix,
    Edit,
}

/// Browser sidebar tabs
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum BrowserTab {
    Files,
    Instruments,
    Effects,
    Loops,
    Samples,
}

/// Inspector panel tabs
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum InspectorTab {
    Track,
    Clip,
    Automation,
    Effects,
}

/// Tool selection for timeline editing
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EditTool {
    Select,
    Draw,
    Erase,
    Cut,
    Mute,
}

/// Snap settings
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SnapMode {
    Off,
    Grid,
    Events,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SnapValue {
    Bar,
    Half,
    Quarter,
    Eighth,
    Sixteenth,
    ThirtySecond,
}

impl SnapValue {
    pub fn to_beats(&self) -> f64 {
        match self {
            SnapValue::Bar => 4.0,
            SnapValue::Half => 2.0,
            SnapValue::Quarter => 1.0,
            SnapValue::Eighth => 0.5,
            SnapValue::Sixteenth => 0.25,
            SnapValue::ThirtySecond => 0.125,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SnapValue::Bar => "1/1",
            SnapValue::Half => "1/2",
            SnapValue::Quarter => "1/4",
            SnapValue::Eighth => "1/8",
            SnapValue::Sixteenth => "1/16",
            SnapValue::ThirtySecond => "1/32",
        }
    }
}

/// Audio file metadata
#[derive(Clone, Debug)]
pub struct AudioFile {
    pub name: String,
    pub path: PathBuf,
    pub duration_seconds: Option<f32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub file_type: String,
    pub size_bytes: u64,
}

/// Timeline viewport state
#[derive(Clone, Debug)]
pub struct ViewportState {
    pub zoom: f64,              // Pixels per beat
    pub scroll_x: f64,          // Beats scrolled
    pub scroll_y: f64,          // Pixels scrolled vertically
    pub visible_tracks: usize,
    pub track_height: f32,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            zoom: 50.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            visible_tracks: 0,
            track_height: 120.0,
        }
    }
}

/// Selection state
#[derive(Clone, Debug, Default)]
pub struct SelectionState {
    pub selected_track_ids: HashSet<TrackId>,
    pub selected_clip_ids: HashSet<uuid::Uuid>,
    pub selected_automation_points: Vec<(TrackId, AutomationParameter, usize)>,
    pub playhead_position: f64, // In beats
    pub loop_start: Option<f64>,
    pub loop_end: Option<f64>,
}

/// Drag and drop state
#[derive(Clone, Debug)]
pub enum DragState {
    None,
    DraggingClip {
        clip_id: uuid::Uuid,
        track_id: TrackId,
        start_beat: f64,
        mouse_offset: (f32, f32),
    },
    DraggingClipEdge {
        clip_id: uuid::Uuid,
        track_id: TrackId,
        is_start: bool,
    },
    DraggingAutomationPoint {
        track_id: TrackId,
        param_type: AutomationParameter,
        point_index: usize,
    },
    DraggingFile {
        file_path: PathBuf,
        file_name: String,
    },
    ResizingTrack {
        track_id: TrackId,
        initial_height: f32,
    },
    DraggingFader {
        track_id: TrackId,
        start_mouse_y: f32,
        start_volume: f32,
    },
    DraggingPan {
        track_id: TrackId,
        start_mouse_x: f32,
        start_pan: f32,
    },
    DraggingTrackHeaderVolume {
        track_id: TrackId,
        start_mouse_x: Pixels,
        start_value: f32,
    },
    DraggingTrackHeaderPan {
        track_id: TrackId,
        start_mouse_x: Pixels,
        start_value: f32,
    },
    DraggingSend {
        track_id: TrackId,
        send_idx: usize,
        start_mouse_x: f32,
        start_amount: f32,
    },
}

impl Default for DragState {
    fn default() -> Self {
        Self::None
    }
}

/// Complete DAW UI state
pub struct DawUiState {
    // Core data
    pub project: Option<DawProject>,
    pub project_path: Option<PathBuf>,
    pub project_dir: Option<PathBuf>,
    pub audio_service: Option<Arc<AudioService>>,

    // Audio asset cache for getting real durations
    pub loaded_assets: std::collections::HashMap<PathBuf, Arc<AudioAssetData>>,

    // View state
    pub view_mode: ViewMode,
    pub browser_tab: BrowserTab,
    pub inspector_tab: InspectorTab,
    pub show_browser: bool,
    pub show_inspector: bool,
    pub show_mixer: bool,
    
    // Transport state
    pub is_playing: bool,
    pub is_recording: bool,
    pub is_looping: bool,
    pub metronome_enabled: bool,
    pub count_in_enabled: bool,
    
    // Edit state
    pub current_tool: EditTool,
    pub snap_mode: SnapMode,
    pub snap_value: SnapValue,
    
    // Viewport and selection
    pub viewport: ViewportState,
    pub selection: SelectionState,
    pub drag_state: DragState,
    
    // Asset library
    pub audio_files: Vec<AudioFile>,
    pub filtered_files: Vec<usize>,
    pub search_query: String,
    
    // Track state
    pub expanded_tracks: HashSet<TrackId>,
    pub solo_tracks: HashSet<TrackId>,
    pub track_heights: HashMap<TrackId, f32>,
    
    // Mixer state
    pub mixer_scroll: f32,
    pub mixer_width: f32,
    pub mixer_scroll_handle: VirtualListScrollHandle,
    pub mixer_scroll_state: ScrollbarState,

    // Metering data
    pub track_meters: std::collections::HashMap<TrackId, MeterData>,
    pub master_meter: MeterData,

    // Virtual list scroll handles for performance
    pub timeline_scroll_handle: VirtualListScrollHandle,  // For horizontal scrolling
    pub timeline_scroll_state: ScrollbarState,
    pub timeline_vertical_scroll_handle: UniformListScrollHandle,  // For vertical scrolling of rows
    pub timeline_scroll_axis_lock: Option<Axis>,  // Lock scrolling to one axis at a time
    pub timeline_scroll_lock_timeout: Option<std::time::Instant>,  // Release lock after inactivity
    
    // Undo/Redo
    pub can_undo: bool,
    pub can_redo: bool,
    
    // UI state
    pub context_menu_position: Option<Point<Pixels>>,
    pub show_track_color_picker: Option<TrackId>,
    pub renaming_track: Option<TrackId>,
    pub rename_buffer: String,
}

impl DawUiState {
    pub fn new() -> Self {
        Self {
            project: None,
            project_path: None,
            project_dir: None,
            audio_service: None,
            loaded_assets: std::collections::HashMap::new(),

            view_mode: ViewMode::Arrange, // Start in arrange view with timeline
            browser_tab: BrowserTab::Files,
            inspector_tab: InspectorTab::Track,
            show_browser: true,
            show_inspector: true,
            show_mixer: true, // Show mixer panel at bottom by default
            
            is_playing: false,
            is_recording: false,
            is_looping: false,
            metronome_enabled: false,
            count_in_enabled: false,
            
            current_tool: EditTool::Select,
            snap_mode: SnapMode::Grid,
            snap_value: SnapValue::Quarter,
            
            viewport: ViewportState::default(),
            selection: SelectionState::default(),
            drag_state: DragState::None,
            
            audio_files: Vec::new(),
            filtered_files: Vec::new(),
            search_query: String::new(),
            
            expanded_tracks: HashSet::new(),
            solo_tracks: HashSet::new(),
            track_heights: HashMap::new(),
            
            mixer_scroll: 0.0,
            mixer_width: 80.0,
            mixer_scroll_handle: VirtualListScrollHandle::new(),
            mixer_scroll_state: ScrollbarState::default(),

            track_meters: std::collections::HashMap::new(),
            master_meter: MeterData::default(),

            timeline_scroll_handle: VirtualListScrollHandle::new(),
            timeline_scroll_state: ScrollbarState::default(),
            timeline_vertical_scroll_handle: UniformListScrollHandle::new(),
            timeline_scroll_axis_lock: None,
            timeline_scroll_lock_timeout: None,
            
            can_undo: false,
            can_redo: false,
            
            context_menu_position: None,
            show_track_color_picker: None,
            renaming_track: None,
            rename_buffer: String::new(),
        }
    }

    /// Load project from file
    pub fn load_project(&mut self, path: PathBuf) -> anyhow::Result<()> {
        let project = DawProject::load(&path)?;
        
        if let Some(parent) = path.parent() {
            self.project_dir = Some(parent.to_path_buf());
            self.scan_audio_files(parent);
        }
        
        self.project_path = Some(path);
        self.project = Some(project);
        self.reset_selection();
        
        Ok(())
    }

    /// Save current project
    pub fn save_project(&self) -> anyhow::Result<()> {
        if let (Some(ref project), Some(ref path)) = (&self.project, &self.project_path) {
            project.save(path.clone())?;
        }
        Ok(())
    }

    /// Create new empty project
    pub fn new_project(&mut self, name: String, project_dir: PathBuf) {
        self.project = Some(DawProject::new(name));
        self.project_dir = Some(project_dir.clone());
        
        // Create project directory structure
        let audio_dir = project_dir.join("audio");
        let _ = std::fs::create_dir_all(&audio_dir);
        
        self.scan_audio_files(&project_dir);
        self.reset_selection();
    }

    /// Scan audio files in project directory
    pub fn scan_audio_files(&mut self, project_dir: &std::path::Path) {
        let audio_dir = project_dir.join("audio");
        if !audio_dir.exists() {
            let _ = std::fs::create_dir_all(&audio_dir);
        }
        
        self.audio_files = Self::scan_directory(&audio_dir);
        self.filtered_files = (0..self.audio_files.len()).collect();
    }

    fn scan_directory(dir: &PathBuf) -> Vec<AudioFile> {
        let mut files = Vec::new();

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    if let Some(ext) = path.extension() {
                        let ext_str = ext.to_string_lossy().to_uppercase();
                        if matches!(ext_str.as_str(), "WAV" | "MP3" | "OGG" | "FLAC" | "AIFF") {
                            if let Some(name) = path.file_stem() {
                                if let Ok(metadata) = std::fs::metadata(&path) {
                                    files.push(AudioFile {
                                        name: name.to_string_lossy().to_string(),
                                        path: path.clone(),
                                        duration_seconds: None,
                                        sample_rate: None,
                                        channels: None,
                                        file_type: ext_str.to_string(),
                                        size_bytes: metadata.len(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        files.sort_by(|a, b| a.name.cmp(&b.name));
        files
    }

    /// Filter audio files by search query
    pub fn filter_files(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_files = (0..self.audio_files.len()).collect();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_files = self
                .audio_files
                .iter()
                .enumerate()
                .filter(|(_, f)| f.name.to_lowercase().contains(&query))
                .map(|(i, _)| i)
                .collect();
        }
    }

    /// Reset selection state
    pub fn reset_selection(&mut self) {
        self.selection = SelectionState::default();
    }

    /// Select track (with multi-select support)
    pub fn select_track(&mut self, track_id: TrackId, multi: bool) {
        if !multi {
            self.selection.selected_track_ids.clear();
        }
        self.selection.selected_track_ids.insert(track_id);
        self.selection.selected_clip_ids.clear();
    }

    /// Select clip
    pub fn select_clip(&mut self, clip_id: uuid::Uuid, multi: bool) {
        if !multi {
            self.selection.selected_clip_ids.clear();
        }
        self.selection.selected_clip_ids.insert(clip_id);
    }

    /// Snap beat position to grid
    pub fn snap_beat(&self, beat: f64) -> f64 {
        match self.snap_mode {
            SnapMode::Off => beat,
            SnapMode::Grid => {
                let snap_beats = self.snap_value.to_beats();
                (beat / snap_beats).round() * snap_beats
            }
            SnapMode::Events => beat, // Would snap to nearby clip edges
        }
    }

    /// Convert beats to pixels
    pub fn beats_to_pixels(&self, beats: f64) -> f32 {
        (beats * self.viewport.zoom) as f32
    }

    /// Convert pixels to beats
    pub fn pixels_to_beats(&self, pixels: f32) -> f64 {
        pixels as f64 / self.viewport.zoom
    }
    
    /// Get current tempo from project
    pub fn get_tempo(&self) -> f32 {
        self.project.as_ref().map(|p| p.transport.tempo).unwrap_or(120.0)
    }

    /// Get track by ID
    pub fn get_track(&self, track_id: TrackId) -> Option<&Track> {
        self.project.as_ref()?.tracks.iter().find(|t| t.id == track_id)
    }

    /// Get track mutably
    pub fn get_track_mut(&mut self, track_id: TrackId) -> Option<&mut Track> {
        self.project.as_mut()?.tracks.iter_mut().find(|t| t.id == track_id)
    }

    /// Add new audio track
    pub fn add_audio_track(&mut self, name: String) -> TrackId {
        if let Some(ref mut project) = self.project {
            let track = Track::new(name, TrackType::Audio);
            let track_id = track.id;
            project.tracks.push(track);
            track_id
        } else {
            uuid::Uuid::new_v4()
        }
    }

    /// Delete track
    pub fn delete_track(&mut self, track_id: TrackId) {
        if let Some(ref mut project) = self.project {
            project.tracks.retain(|t| t.id != track_id);
            self.selection.selected_track_ids.remove(&track_id);
        }
    }

    /// Add clip to track
    pub fn add_clip(&mut self, track_id: TrackId, start_beat: f64, file_path: PathBuf) -> Option<uuid::Uuid> {
        // Get tempo first before borrowing track mutably
        let tempo = self.get_tempo();
        let samples_per_beat = (SAMPLE_RATE * 60.0) / tempo;
        let start_samples = (start_beat * samples_per_beat as f64) as u64;
        let duration_samples = (4.0 * samples_per_beat) as u64; // Default 4 beats
        
        if let Some(track) = self.get_track_mut(track_id) {
            let clip = AudioClip::new(file_path, start_samples, duration_samples);
            let clip_id = clip.id;
            track.clips.push(clip);
            Some(clip_id)
        } else {
            None
        }
    }

    /// Import audio file to project
    pub fn import_audio_file(&mut self, source_path: PathBuf) -> anyhow::Result<PathBuf> {
        // Clone project_dir to avoid borrow issue
        let proj_dir = self.project_dir.clone().ok_or_else(|| anyhow::anyhow!("No project directory"))?;
        
        let audio_dir = proj_dir.join("audio");
        std::fs::create_dir_all(&audio_dir)?;
        
        if let Some(filename) = source_path.file_name() {
            let dest = audio_dir.join(filename);
            std::fs::copy(&source_path, &dest)?;
            self.scan_audio_files(&proj_dir);
            Ok(dest)
        } else {
            Err(anyhow::anyhow!("Invalid file name"))
        }
    }

    /// Set playhead position in beats
    pub fn set_playhead(&mut self, beats: f64) {
        self.selection.playhead_position = beats.max(0.0);
    }

    /// Toggle solo on track
    pub fn toggle_solo(&mut self, track_id: TrackId) {
        if self.solo_tracks.contains(&track_id) {
            self.solo_tracks.remove(&track_id);
        } else {
            self.solo_tracks.insert(track_id);
        }
    }

    /// Get effective mute state (considering solo)
    pub fn is_track_effectively_muted(&self, track_id: TrackId) -> bool {
        if let Some(track) = self.get_track(track_id) {
            if self.solo_tracks.is_empty() {
                track.muted
            } else {
                !self.solo_tracks.contains(&track_id)
            }
        } else {
            false
        }
    }

    /// Load audio asset and cache it for duration info
    pub async fn load_audio_asset(&mut self, path: PathBuf) -> anyhow::Result<Arc<AudioAssetData>> {
        // Check cache first
        if let Some(asset) = self.loaded_assets.get(&path) {
            return Ok(asset.clone());
        }

        // Load via audio service if available
        if let Some(service) = &self.audio_service {
            let asset = service.load_asset(path.clone()).await?;
            self.loaded_assets.insert(path, asset.clone());
            Ok(asset)
        } else {
            Err(anyhow::anyhow!("Audio service not initialized"))
        }
    }

    /// Get audio asset duration in samples, loading if necessary
    pub fn get_audio_duration_samples(&self, path: &PathBuf) -> Option<u64> {
        self.loaded_assets.get(path).map(|asset| asset.asset_ref.duration_samples as u64)
    }
}
