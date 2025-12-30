/// Audio graph mixing engine with tracks, buses, and sends
use super::asset_manager::AssetManager;
use super::audio_types::*;
use std::collections::HashMap;

/// Audio graph node processor
pub struct AudioGraph {
    tracks: HashMap<TrackId, Track>,
    master_track: Track,
    asset_manager: AssetManager,
    track_meters: HashMap<TrackId, MeterData>,
    master_meter: MeterData,
    any_solo: bool,
}

impl AudioGraph {
    pub fn new(asset_manager: AssetManager) -> Self {
        let master_track = Track::new("Master", TrackType::Master);
        
        Self {
            tracks: HashMap::new(),
            master_track,
            asset_manager,
            track_meters: HashMap::new(),
            master_meter: MeterData::default(),
            any_solo: false,
        }
    }

    pub fn add_track(&mut self, track: Track) -> TrackId {
        let id = track.id;
        self.tracks.insert(id, track);
        self.track_meters.insert(id, MeterData::default());
        self.update_solo_state();
        id
    }

    pub fn remove_track(&mut self, id: TrackId) {
        self.tracks.remove(&id);
        self.track_meters.remove(&id);
        self.update_solo_state();
    }

    pub fn get_track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.get(&id)
    }

    pub fn get_track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.get_mut(&id)
    }

    pub fn get_master_track(&self) -> &Track {
        &self.master_track
    }

    pub fn get_master_track_mut(&mut self) -> &mut Track {
        &mut self.master_track
    }

    pub fn get_all_tracks(&self) -> Vec<&Track> {
        self.tracks.values().collect()
    }

    pub fn get_track_meter(&self, id: TrackId) -> Option<MeterData> {
        self.track_meters.get(&id).copied()
    }

    pub fn get_master_meter(&self) -> MeterData {
        self.master_meter
    }

    fn update_solo_state(&mut self) {
        self.any_solo = self.tracks.values().any(|t| t.solo);
    }

    /// Process audio graph for a buffer
    pub fn process(
        &mut self,
        transport: &Transport,
        output_left: &mut [f32],
        output_right: &mut [f32],
    ) {
        let buffer_size = output_left.len();
        
        output_left.fill(0.0);
        output_right.fill(0.0);

        if transport.state != TransportState::Playing {
            self.master_meter = MeterData::default();
            for meter in self.track_meters.values_mut() {
                *meter = MeterData::default();
            }
            return;
        }

        let mut aux_buffers: HashMap<TrackId, (Vec<f32>, Vec<f32>)> = HashMap::new();

        for track_id in self.tracks.keys().copied().collect::<Vec<_>>() {
            if let Some(track) = self.tracks.get(&track_id) {
                if track.track_type == TrackType::Aux {
                    aux_buffers.insert(
                        track_id,
                        (vec![0.0; buffer_size], vec![0.0; buffer_size]),
                    );
                }
            }
        }

        let mut track_outputs: HashMap<TrackId, (Vec<f32>, Vec<f32>)> = HashMap::new();

        for track_id in self.tracks.keys().copied().collect::<Vec<_>>() {
            let should_process = if let Some(track) = self.tracks.get(&track_id) {
                if track.muted {
                    false
                } else if self.any_solo {
                    track.solo
                } else {
                    true
                }
            } else {
                false
            };

            if !should_process {
                continue;
            }

            let mut left = vec![0.0; buffer_size];
            let mut right = vec![0.0; buffer_size];

            if let Some(track) = self.tracks.get(&track_id) {
                self.process_track(track, transport, &mut left, &mut right);
            }

            track_outputs.insert(track_id, (left, right));
        }

        for (track_id, (left, right)) in &track_outputs {
            if let Some(track) = self.tracks.get(track_id) {
                for (send_idx, send) in track.sends.iter().enumerate() {
                    if send.enabled && send.amount > 0.0 {
                        if let Some(target_id) = send.target_track {
                            if let Some((aux_left, aux_right)) = aux_buffers.get_mut(&target_id) {
                                let send_amount = send.amount;
                                for i in 0..buffer_size {
                                    aux_left[i] += left[i] * send_amount;
                                    aux_right[i] += right[i] * send_amount;
                                }
                            }
                        }
                    }
                }
            }
        }

        for (track_id, (aux_left, aux_right)) in aux_buffers {
            if let Some(track) = self.tracks.get(&track_id) {
                if !track.muted {
                    let volume = track.volume;
                    let pan = track.pan;
                    let (pan_left, pan_right) = calculate_pan(pan);

                    for i in 0..buffer_size {
                        let l = aux_left[i] * volume;
                        let r = aux_right[i] * volume;

                        output_left[i] += l * pan_left;
                        output_right[i] += r * pan_right;
                    }

                    let meter = MeterData::from_buffer(&aux_left, &aux_right);
                    self.track_meters.insert(track_id, meter);
                }
            }
        }

        for (track_id, (left, right)) in track_outputs {
            if let Some(track) = self.tracks.get(&track_id) {
                if track.track_type != TrackType::Aux {
                    let volume = track.volume;
                    let pan = track.pan;
                    let (pan_left, pan_right) = calculate_pan(pan);

                    for i in 0..buffer_size {
                        let l = left[i] * volume;
                        let r = right[i] * volume;

                        output_left[i] += l * pan_left;
                        output_right[i] += r * pan_right;
                    }

                    let meter = MeterData::from_buffer(&left, &right);
                    self.track_meters.insert(track_id, meter);
                }
            }
        }

        let master_volume = self.master_track.volume;
        for i in 0..buffer_size {
            output_left[i] *= master_volume;
            output_right[i] *= master_volume;

            output_left[i] = output_left[i].clamp(-1.0, 1.0);
            output_right[i] = output_right[i].clamp(-1.0, 1.0);
        }

        self.master_meter = MeterData::from_buffer(output_left, output_right);
    }

    fn process_track(
        &self,
        track: &Track,
        transport: &Transport,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let buffer_size = left.len();
        let start_time = transport.position;

        for clip in &track.clips {
            if !clip.is_active_at(start_time) && !clip.is_active_at(start_time + buffer_size as u64)
            {
                continue;
            }

            if let Some(asset) = self.asset_manager.get_cached(&clip.asset_path) {
                self.render_clip(clip, &asset, start_time, buffer_size, left, right);
            }
        }

        let volume_lane = track.get_automation_lane(AutomationParameter::Volume);
        let pan_lane = track.get_automation_lane(AutomationParameter::Pan);

        for i in 0..buffer_size {
            let sample_time = start_time + i as u64;

            let volume_mult = volume_lane
                .and_then(|lane| lane.value_at(sample_time))
                .unwrap_or(1.0);

            let pan_value = pan_lane
                .and_then(|lane| lane.value_at(sample_time))
                .unwrap_or(0.0);

            left[i] *= volume_mult;
            right[i] *= volume_mult;

            let (pan_left, pan_right) = calculate_pan(pan_value);
            let l = left[i];
            let r = right[i];
            left[i] = l * pan_left;
            right[i] = r * pan_right;
        }
    }

    fn render_clip(
        &self,
        clip: &AudioClip,
        asset: &AudioAssetData,
        start_time: SampleTime,
        buffer_size: usize,
        left: &mut [f32],
        right: &mut [f32],
    ) {
        let clip_start = clip.start_time;
        let clip_end = clip.end_time();

        for i in 0..buffer_size {
            let sample_time = start_time + i as u64;

            if sample_time < clip_start || sample_time >= clip_end {
                continue;
            }

            let relative_time = sample_time - clip_start;
            let source_time = clip.offset + relative_time;

            let fade = clip.fade_at(sample_time);
            if fade == 0.0 {
                continue;
            }

            let channels = asset.asset_ref.channels;
            let source_frame = source_time as usize;

            if source_frame >= asset.asset_ref.duration_samples {
                continue;
            }

            let source_idx = source_frame * channels;

            let (sample_left, sample_right) = if channels == 1 {
                let mono = asset.samples.get(source_idx).copied().unwrap_or(0.0);
                (mono, mono)
            } else {
                let l = asset.samples.get(source_idx).copied().unwrap_or(0.0);
                let r = asset
                    .samples
                    .get(source_idx + 1)
                    .copied()
                    .unwrap_or(l);
                (l, r)
            };

            left[i] += sample_left * fade;
            right[i] += sample_right * fade;
        }
    }
}

fn calculate_pan(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * 0.25 * std::f32::consts::PI;
    let left = angle.cos();
    let right = angle.sin();
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_pan() {
        let (left, right) = calculate_pan(0.0);
        assert!((left - 0.707).abs() < 0.01);
        assert!((right - 0.707).abs() < 0.01);

        let (left, right) = calculate_pan(-1.0);
        assert!(left > 0.99);
        assert!(right < 0.01);

        let (left, right) = calculate_pan(1.0);
        assert!(left < 0.01);
        assert!(right > 0.99);
    }

    #[test]
    fn test_audio_graph_creation() {
        let manager = AssetManager::new();
        let graph = AudioGraph::new(manager);
        assert_eq!(graph.get_all_tracks().len(), 0);
    }

    #[test]
    fn test_add_remove_track() {
        let manager = AssetManager::new();
        let mut graph = AudioGraph::new(manager);
        
        let track = Track::new("Test Track", TrackType::Audio);
        let id = graph.add_track(track);
        
        assert!(graph.get_track(id).is_some());
        
        graph.remove_track(id);
        assert!(graph.get_track(id).is_none());
    }

    #[test]
    fn test_solo_state() {
        let manager = AssetManager::new();
        let mut graph = AudioGraph::new(manager);
        
        let mut track1 = Track::new("Track 1", TrackType::Audio);
        track1.solo = true;
        let id1 = graph.add_track(track1);
        
        assert!(graph.any_solo);
    }
}
