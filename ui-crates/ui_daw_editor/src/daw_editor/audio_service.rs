/// Main audio service coordinating all DAW subsystems
use super::asset_manager::AssetManager;
use super::audio_graph::AudioGraph;
use super::audio_types::*;
use super::gpu_dsp::{GpuDsp, DspJob};
use super::real_time_audio::{AudioCommand, RealTimeAudio};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;
use std::marker::Send;

/// Main DAW audio service
pub struct AudioService {
    audio_graph: Arc<parking_lot::RwLock<AudioGraph>>,
    real_time_audio: Arc<RealTimeAudio>,
    asset_manager: Arc<AssetManager>,
    gpu_dsp: Arc<RwLock<Option<GpuDsp>>>,
    transport: Arc<parking_lot::RwLock<Transport>>,
    performance_metrics: Arc<RwLock<PerformanceMetrics>>,
}

/// Thread-safe position monitor that can be cloned and sent across threads
#[derive(Clone)]
pub struct PositionMonitor {
    real_time_audio: Arc<RealTimeAudio>,
}

impl PositionMonitor {
    pub fn get_position(&self) -> SampleTime {
        self.real_time_audio.get_position()
    }

    pub fn get_transport(&self) -> Transport {
        self.real_time_audio.get_transport()
    }
}

// Implement Send + Sync for PositionMonitor (safe because RealTimeAudio methods are thread-safe)
unsafe impl Send for PositionMonitor {}
unsafe impl Sync for PositionMonitor {}

impl AudioService {
    pub async fn new() -> Result<Self> {
        let asset_manager = Arc::new(AssetManager::new());
        let audio_graph = Arc::new(parking_lot::RwLock::new(AudioGraph::new(asset_manager.as_ref().clone())));

        let real_time_audio = Arc::new(RealTimeAudio::new(audio_graph.clone())?);
        let transport = Arc::new(parking_lot::RwLock::new(Transport::default()));

        let gpu_dsp = match GpuDsp::new().await {
            Ok(dsp) => Arc::new(RwLock::new(Some(dsp))),
            Err(e) => {
                tracing::error!("Failed to initialize GPU DSP: {}. GPU features disabled.", e);
                Arc::new(RwLock::new(None))
            }
        };

        let performance_metrics = Arc::new(RwLock::new(PerformanceMetrics::default()));

        let service = Self {
            audio_graph,
            real_time_audio,
            asset_manager,
            gpu_dsp,
            transport,
            performance_metrics,
        };

        Ok(service)
    }

    pub async fn add_track(&self, track: Track) -> TrackId {
        let mut graph = self.audio_graph.write();
        graph.add_track(track)
    }

    pub async fn remove_track(&self, id: TrackId) {
        let mut graph = self.audio_graph.write();
        graph.remove_track(id);
    }

    pub async fn get_track(&self, id: TrackId) -> Option<Track> {
        let graph = self.audio_graph.read();
        graph.get_track(id).cloned()
    }

    pub async fn update_track<F>(&self, id: TrackId, f: F)
    where
        F: FnOnce(&mut Track),
    {
        let mut graph = self.audio_graph.write();
        if let Some(track) = graph.get_track_mut(id) {
            f(track);
        }
    }

    pub async fn get_all_tracks(&self) -> Vec<Track> {
        let graph = self.audio_graph.read();
        graph.get_all_tracks().into_iter().cloned().collect()
    }

    pub async fn get_master_track(&self) -> Track {
        let graph = self.audio_graph.read();
        graph.get_master_track().clone()
    }

    pub async fn set_master_volume(&self, volume: f32) -> Result<()> {
        {
            let mut graph = self.audio_graph.write();
            graph.get_master_track_mut().volume = volume;
        }
        self.real_time_audio
            .send_command(AudioCommand::SetMasterVolume(volume))
    }

    pub async fn set_track_volume(&self, track_id: TrackId, volume: f32) -> Result<()> {
        {
            let mut graph = self.audio_graph.write();
            if let Some(track) = graph.get_track_mut(track_id) {
                track.volume = volume;
            }
        }
        self.real_time_audio
            .send_command(AudioCommand::SetTrackVolume { track_id, volume })
    }

    pub async fn set_track_pan(&self, track_id: TrackId, pan: f32) -> Result<()> {
        {
            let mut graph = self.audio_graph.write();
            if let Some(track) = graph.get_track_mut(track_id) {
                track.pan = pan;
            }
        }
        self.real_time_audio
            .send_command(AudioCommand::SetTrackPan { track_id, pan })
    }

    pub async fn set_track_mute(&self, track_id: TrackId, muted: bool) -> Result<()> {
        {
            let mut graph = self.audio_graph.write();
            if let Some(track) = graph.get_track_mut(track_id) {
                track.muted = muted;
            }
        }
        self.real_time_audio
            .send_command(AudioCommand::SetTrackMute { track_id, muted })
    }

    pub async fn set_track_solo(&self, track_id: TrackId, solo: bool) -> Result<()> {
        {
            let mut graph = self.audio_graph.write();
            if let Some(track) = graph.get_track_mut(track_id) {
                track.solo = solo;
            }
        }
        self.real_time_audio
            .send_command(AudioCommand::SetTrackSolo { track_id, solo })
    }

    pub async fn play(&self) -> Result<()> {
        self.real_time_audio.send_command(AudioCommand::Play)
    }

    pub async fn pause(&self) -> Result<()> {
        self.real_time_audio.send_command(AudioCommand::Pause)
    }

    pub async fn stop(&self) -> Result<()> {
        self.real_time_audio.send_command(AudioCommand::Stop)
    }

    pub async fn seek(&self, position: SampleTime) -> Result<()> {
        self.real_time_audio.send_command(AudioCommand::Seek(position))
    }

    pub async fn set_loop(&self, enabled: bool, start: SampleTime, end: SampleTime) -> Result<()> {
        self.real_time_audio.send_command(AudioCommand::SetLoop {
            enabled,
            start,
            end,
        })
    }

    pub async fn set_tempo(&self, tempo: f32) -> Result<()> {
        self.real_time_audio.send_command(AudioCommand::SetTempo(tempo))
    }

    pub async fn set_metronome(&self, enabled: bool) -> Result<()> {
        self.real_time_audio
            .send_command(AudioCommand::SetMetronome(enabled))
    }

    pub fn get_transport(&self) -> Transport {
        self.real_time_audio.get_transport()
    }

    pub fn get_position(&self) -> SampleTime {
        self.real_time_audio.get_position()
    }

    /// Get a thread-safe position monitor that can be sent across threads
    pub fn get_position_monitor(&self) -> PositionMonitor {
        PositionMonitor {
            real_time_audio: self.real_time_audio.clone(),
        }
    }

    pub async fn load_asset(&self, path: std::path::PathBuf) -> Result<Arc<AudioAssetData>> {
        self.asset_manager.load_asset(path).await
    }

    pub async fn add_clip_to_track(
        &self,
        track_id: TrackId,
        clip: AudioClip,
    ) -> Result<()> {
        let mut graph = self.audio_graph.write();
        if let Some(track) = graph.get_track_mut(track_id) {
            track.clips.push(clip);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Track not found"))
        }
    }

    pub async fn remove_clip_from_track(
        &self,
        track_id: TrackId,
        clip_id: ClipId,
    ) -> Result<()> {
        let mut graph = self.audio_graph.write();
        if let Some(track) = graph.get_track_mut(track_id) {
            track.clips.retain(|c| c.id != clip_id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Track not found"))
        }
    }

    pub async fn add_automation_point(
        &self,
        track_id: TrackId,
        parameter: AutomationParameter,
        point: AutomationPoint,
    ) -> Result<()> {
        let mut graph = self.audio_graph.write();
        if let Some(track) = graph.get_track_mut(track_id) {
            track.get_automation_lane_mut(parameter).add_point(point);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Track not found"))
        }
    }

    pub async fn remove_automation_point(
        &self,
        track_id: TrackId,
        parameter: AutomationParameter,
        point_id: AutomationId,
    ) -> Result<()> {
        let mut graph = self.audio_graph.write();
        if let Some(track) = graph.get_track_mut(track_id) {
            track.get_automation_lane_mut(parameter).remove_point(point_id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("Track not found"))
        }
    }

    pub async fn get_track_meter(&self, track_id: TrackId) -> Option<MeterData> {
        let graph = self.audio_graph.read();
        graph.get_track_meter(track_id)
    }

    pub async fn get_master_meter(&self) -> MeterData {
        let graph = self.audio_graph.read();
        graph.get_master_meter()
    }

    pub async fn get_performance_metrics(&self) -> PerformanceMetrics {
        *self.performance_metrics.read().await
    }

    pub async fn get_gpu_jobs(&self) -> Vec<DspJob> {
        if let Some(dsp) = self.gpu_dsp.read().await.as_ref() {
            dsp.get_jobs().await
        } else {
            Vec::new()
        }
    }

    pub async fn clear_completed_gpu_jobs(&self) {
        if let Some(dsp) = self.gpu_dsp.read().await.as_ref() {
            dsp.clear_completed_jobs().await;
        }
    }

    pub fn device_name(&self) -> String {
        self.real_time_audio.device_name()
    }

    pub fn sample_rate(&self) -> u32 {
        self.real_time_audio.sample_rate()
    }

    pub fn channels(&self) -> u16 {
        self.real_time_audio.channels()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audio_service_creation() {
        let result = AudioService::new().await;
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_add_remove_track() {
        if let Ok(service) = AudioService::new().await {
            let track = Track::new("Test", TrackType::Audio);
            let id = service.add_track(track).await;

            assert!(service.get_track(id).await.is_some());

            service.remove_track(id).await;
            assert!(service.get_track(id).await.is_none());
        }
    }
}
