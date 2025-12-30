/// ECS integration for game engine audio events
use super::audio_service::AudioService;
use super::audio_types::*;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Audio events that can be triggered from the ECS
#[derive(Debug, Clone)]
pub enum AudioEvent {
    PlaySound { track_id: TrackId },
    StopTrack { track_id: TrackId },
    SetVolume { track_id: TrackId, volume: f32 },
    SetPan { track_id: TrackId, pan: f32 },
    TriggerAutomation {
        track_id: TrackId,
        param: AutomationParameter,
        value: f32,
        sample_time: SampleTime,
    },
    Crossfade {
        track_a: TrackId,
        track_b: TrackId,
        duration_seconds: f32,
    },
}

/// ECS audio integration bridge
pub struct EcsAudioBridge {
    audio_service: Arc<AudioService>,
    event_queue: Arc<RwLock<Vec<AudioEvent>>>,
}

impl EcsAudioBridge {
    pub fn new(audio_service: Arc<AudioService>) -> Self {
        let event_queue = Arc::new(RwLock::new(Vec::new()));
        
        Self {
            audio_service,
            event_queue,
        }
    }

    async fn process_event(audio_service: &AudioService, event: AudioEvent) -> Result<()> {
        match event {
            AudioEvent::PlaySound { track_id: _ } => {
                audio_service.play().await?;
            }
            AudioEvent::StopTrack { track_id } => {
                audio_service.set_track_mute(track_id, true).await?;
            }
            AudioEvent::SetVolume { track_id, volume } => {
                audio_service.set_track_volume(track_id, volume).await?;
            }
            AudioEvent::SetPan { track_id, pan } => {
                audio_service.set_track_pan(track_id, pan).await?;
            }
            AudioEvent::TriggerAutomation {
                track_id,
                param,
                value,
                sample_time,
            } => {
                let point = AutomationPoint {
                    id: uuid::Uuid::new_v4(),
                    time: sample_time,
                    value,
                    curve_type: CurveType::Linear,
                    bezier_handle_in: None,
                    bezier_handle_out: None,
                };
                audio_service.add_automation_point(track_id, param, point).await?;
            }
            AudioEvent::Crossfade {
                track_a,
                track_b,
                duration_seconds,
            } => {
                let duration_samples = (duration_seconds * SAMPLE_RATE) as SampleTime;
                let current_pos = audio_service.get_position();

                let fade_out_point = AutomationPoint {
                    id: uuid::Uuid::new_v4(),
                    time: current_pos + duration_samples,
                    value: 0.0,
                    curve_type: CurveType::Linear,
                    bezier_handle_in: None,
                    bezier_handle_out: None,
                };
                audio_service
                    .add_automation_point(track_a, AutomationParameter::Volume, fade_out_point)
                    .await?;

                let fade_in_point = AutomationPoint {
                    id: uuid::Uuid::new_v4(),
                    time: current_pos + duration_samples,
                    value: 1.0,
                    curve_type: CurveType::Linear,
                    bezier_handle_in: None,
                    bezier_handle_out: None,
                };
                audio_service
                    .add_automation_point(track_b, AutomationParameter::Volume, fade_in_point)
                    .await?;
            }
        }

        Ok(())
    }

    /// Dispatch an audio event (processes immediately)
    pub async fn dispatch_event(&self, event: AudioEvent) {
        if let Err(e) = Self::process_event(&self.audio_service, event).await {
            tracing::error!("Failed to process audio event: {}", e);
        }
    }

    /// Get queryable track state for ECS systems
    pub async fn get_track_state(&self, track_id: TrackId) -> Option<TrackState> {
        let track = self.audio_service.get_track(track_id).await?;
        let meter = self.audio_service.get_track_meter(track_id).await?;
        
        Some(TrackState {
            id: track.id,
            name: track.name.clone(),
            playing: !track.muted,
            volume: track.volume,
            pan: track.pan,
            meter,
        })
    }

    /// Get all track states
    pub async fn get_all_track_states(&self) -> Vec<TrackState> {
        let tracks = self.audio_service.get_all_tracks().await;
        let mut states = Vec::new();

        for track in tracks {
            if let Some(meter) = self.audio_service.get_track_meter(track.id).await {
                states.push(TrackState {
                    id: track.id,
                    name: track.name,
                    playing: !track.muted,
                    volume: track.volume,
                    pan: track.pan,
                    meter,
                });
            }
        }

        states
    }
}

/// Queryable track state for ECS
#[derive(Debug, Clone)]
pub struct TrackState {
    pub id: TrackId,
    pub name: String,
    pub playing: bool,
    pub volume: f32,
    pub pan: f32,
    pub meter: MeterData,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_dispatch() {
        if let Ok(service) = AudioService::new().await {
            let bridge = EcsAudioBridge::new(Arc::new(service));
            
            let track_id = uuid::Uuid::new_v4();
            bridge.dispatch_event(AudioEvent::SetVolume {
                track_id,
                volume: 0.5,
            }).await;

            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
}
