/// DAW project serialization and file format (.pdaw)
use super::audio_types::*;
use anyhow::{Context as AnyhowContext, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// DAW project file format version
const PROJECT_VERSION: u32 = 1;

/// Complete DAW project state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DawProject {
    pub version: u32,
    pub name: String,
    pub created_at: String,
    pub modified_at: String,
    pub sample_rate: f32,
    pub tracks: Vec<Track>,
    pub transport: Transport,
    pub master_track: Track,
}

impl DawProject {
    pub fn new(name: impl Into<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        
        Self {
            version: PROJECT_VERSION,
            name: name.into(),
            created_at: now.clone(),
            modified_at: now,
            sample_rate: SAMPLE_RATE,
            tracks: Vec::new(),
            transport: Transport::default(),
            master_track: Track::new("Master", TrackType::Master),
        }
    }

    /// Load project from file
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let contents = std::fs::read_to_string(path.as_ref())
            .context("Failed to read project file")?;
        
        let project: DawProject = serde_json::from_str(&contents)
            .context("Failed to parse project file")?;

        if project.version > PROJECT_VERSION {
            return Err(anyhow::anyhow!(
                "Project version {} is newer than supported version {}",
                project.version,
                PROJECT_VERSION
            ));
        }

        Ok(project)
    }

    /// Save project to file
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let mut project_clone = self.clone();
        project_clone.modified_at = chrono::Utc::now().to_rfc3339();
        
        let contents = serde_json::to_string_pretty(&project_clone)
            .context("Failed to serialize project")?;
        
        std::fs::write(path.as_ref(), contents)
            .context("Failed to write project file")?;

        Ok(())
    }

    /// Add a track to the project
    pub fn add_track(&mut self, track: Track) {
        self.tracks.push(track);
    }

    /// Remove a track from the project
    pub fn remove_track(&mut self, id: TrackId) {
        self.tracks.retain(|t| t.id != id);
    }

    /// Get a track by ID
    pub fn get_track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// Get a mutable track by ID
    pub fn get_track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    /// Get all tracks
    pub fn get_all_tracks(&self) -> &[Track] {
        &self.tracks
    }

    /// Get duration of project in samples
    pub fn duration(&self) -> SampleTime {
        let mut max_time = 0u64;
        
        for track in &self.tracks {
            for clip in &track.clips {
                let end_time = clip.end_time();
                if end_time > max_time {
                    max_time = end_time;
                }
            }
        }

        max_time
    }

    /// Get duration in seconds
    pub fn duration_seconds(&self) -> f64 {
        self.duration() as f64 / self.sample_rate as f64
    }

    /// Get duration as formatted time string
    pub fn duration_string(&self) -> String {
        let seconds = self.duration_seconds();
        let hours = (seconds / 3600.0) as u32;
        let minutes = ((seconds % 3600.0) / 60.0) as u32;
        let secs = (seconds % 60.0) as u32;
        let millis = ((seconds % 1.0) * 1000.0) as u32;
        
        format!("{:02}:{:02}:{:02}.{:03}", hours, minutes, secs, millis)
    }

    /// Validate project integrity
    pub fn validate(&self) -> Result<()> {
        for track in &self.tracks {
            for clip in &track.clips {
                if clip.start_time >= clip.end_time() {
                    return Err(anyhow::anyhow!(
                        "Invalid clip timing in track '{}': start >= end",
                        track.name
                    ));
                }
            }

            for automation in &track.automation {
                if automation.points.is_empty() {
                    continue;
                }

                for window in automation.points.windows(2) {
                    if window[0].time >= window[1].time {
                        return Err(anyhow::anyhow!(
                            "Invalid automation timing in track '{}'",
                            track.name
                        ));
                    }
                }
            }
        }

        Ok(())
    }

    /// Export project to a different format (stub for future formats)
    pub fn export(&self, path: impl AsRef<Path>, format: ExportFormat) -> Result<()> {
        match format {
            ExportFormat::Json => self.save(path),
            ExportFormat::Ron => {
                let contents = ron::to_string(&self)
                    .context("Failed to serialize project to RON")?;
                std::fs::write(path.as_ref(), contents)
                    .context("Failed to write RON file")?;
                Ok(())
            }
        }
    }

    /// Import project from a different format (stub for future formats)
    pub fn import(path: impl AsRef<Path>, format: ExportFormat) -> Result<Self> {
        match format {
            ExportFormat::Json => Self::load(path),
            ExportFormat::Ron => {
                let contents = std::fs::read_to_string(path.as_ref())
                    .context("Failed to read RON file")?;
                let project: DawProject = ron::from_str(&contents)
                    .context("Failed to parse RON file")?;
                Ok(project)
            }
        }
    }
}

/// Export format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Json,
    Ron,
}

/// Create a default demo project
pub fn create_demo_project() -> DawProject {
    let mut project = DawProject::new("Demo Project");
    
    let mut track1 = Track::new("Drums", TrackType::Audio);
    track1.color = [0.9, 0.3, 0.3];
    project.add_track(track1);

    let mut track2 = Track::new("Bass", TrackType::Audio);
    track2.color = [0.3, 0.9, 0.3];
    project.add_track(track2);

    let mut track3 = Track::new("Synth", TrackType::Audio);
    track3.color = [0.3, 0.3, 0.9];
    project.add_track(track3);

    let mut aux_track = Track::new("Reverb", TrackType::Aux);
    aux_track.color = [0.7, 0.7, 0.3];
    project.add_track(aux_track);

    project
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_project_creation() {
        let project = DawProject::new("Test Project");
        assert_eq!(project.name, "Test Project");
        assert_eq!(project.version, PROJECT_VERSION);
        assert_eq!(project.tracks.len(), 0);
    }

    #[test]
    fn test_add_remove_track() {
        let mut project = DawProject::new("Test");
        let track = Track::new("Track 1", TrackType::Audio);
        let id = track.id;
        
        project.add_track(track);
        assert_eq!(project.tracks.len(), 1);
        
        project.remove_track(id);
        assert_eq!(project.tracks.len(), 0);
    }

    #[test]
    fn test_save_load() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let path = temp_dir.path().join("test.pdaw");
        
        let mut project = DawProject::new("Test Save");
        project.add_track(Track::new("Track 1", TrackType::Audio));
        
        project.save(&path)?;
        
        let loaded = DawProject::load(&path)?;
        assert_eq!(loaded.name, "Test Save");
        assert_eq!(loaded.tracks.len(), 1);
        
        Ok(())
    }

    #[test]
    fn test_demo_project() {
        let project = create_demo_project();
        assert!(project.tracks.len() > 0);
        assert!(project.validate().is_ok());
    }

    #[test]
    fn test_export_ron() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let path = temp_dir.path().join("test.ron");
        
        let project = DawProject::new("Test RON");
        project.export(&path, ExportFormat::Ron)?;
        
        let loaded = DawProject::import(&path, ExportFormat::Ron)?;
        assert_eq!(loaded.name, "Test RON");
        
        Ok(())
    }
}
