/// Real-time audio thread using CPAL for cross-platform audio I/O
use super::audio_graph::AudioGraph;
use super::audio_types::*;
use anyhow::{Context as AnyhowContext, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Stream, StreamConfig};
use crossbeam::channel::{Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Commands sent to the audio thread
#[derive(Debug, Clone)]
pub enum AudioCommand {
    SetMasterVolume(f32),
    SetTrackVolume { track_id: TrackId, volume: f32 },
    SetTrackPan { track_id: TrackId, pan: f32 },
    SetTrackMute { track_id: TrackId, muted: bool },
    SetTrackSolo { track_id: TrackId, solo: bool },
    Play,
    Pause,
    Stop,
    Seek(SampleTime),
    SetLoop { enabled: bool, start: SampleTime, end: SampleTime },
    SetTempo(f32),
    SetMetronome(bool),
}

/// Messages sent from the audio thread
#[derive(Debug, Clone)]
pub enum AudioMessage {
    TransportUpdate(Transport),
    MeterUpdate { track_id: Option<TrackId>, meter: MeterData },
    PerformanceMetrics(PerformanceMetrics),
    Underrun,
    Error(String),
}

/// Lock-free communication structure for audio thread
pub struct AudioThreadComm {
    command_rx: Receiver<AudioCommand>,
    message_tx: Sender<AudioMessage>,
    transport: Arc<parking_lot::RwLock<Transport>>,
    running: Arc<AtomicBool>,
    position: Arc<AtomicU64>,
}

impl AudioThreadComm {
    pub fn new(
        command_rx: Receiver<AudioCommand>,
        message_tx: Sender<AudioMessage>,
        transport: Arc<parking_lot::RwLock<Transport>>,
    ) -> Self {
        Self {
            command_rx,
            message_tx,
            transport,
            running: Arc::new(AtomicBool::new(true)),
            position: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Release);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn get_position(&self) -> SampleTime {
        self.position.load(Ordering::Acquire)
    }

    pub fn set_position(&self, pos: SampleTime) {
        self.position.store(pos, Ordering::Release);
    }
}

/// Real-time audio engine
pub struct RealTimeAudio {
    _stream: Stream,
    device: Device,
    config: StreamConfig,
    command_tx: Sender<AudioCommand>,
    message_rx: Receiver<AudioMessage>,
    transport: Arc<parking_lot::RwLock<Transport>>,
    comm: Arc<AudioThreadComm>,
}

impl RealTimeAudio {
    pub fn new(audio_graph: Arc<parking_lot::RwLock<AudioGraph>>) -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .context("No output device available")?;

        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels();

        tracing::debug!(
            "Audio device: {} - {} Hz, {} channels",
            device.name().unwrap_or_else(|_| "Unknown".to_string()),
            sample_rate,
            channels
        );

        let config: StreamConfig = config.into();

        let (command_tx, command_rx) = crossbeam::channel::unbounded();
        let (message_tx, message_rx) = crossbeam::channel::unbounded();

        let transport = Arc::new(parking_lot::RwLock::new(Transport::default()));
        let comm = Arc::new(AudioThreadComm::new(
            command_rx,
            message_tx,
            transport.clone(),
        ));

        let stream_comm = comm.clone();
        let stream_transport = transport.clone();
        let stream_audio_graph = audio_graph.clone();

        let mut callback_buffer_left = vec![0.0f32; BUFFER_SIZE];
        let mut callback_buffer_right = vec![0.0f32; BUFFER_SIZE];
        let mut buffer_position = 0;
        let mut callback_count = 0u64;
        let underrun_count = 0u64;

        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let start_time = Instant::now();

                while let Ok(cmd) = stream_comm.command_rx.try_recv() {
                    match cmd {
                        AudioCommand::Play => {
                            stream_transport.write().play();
                        }
                        AudioCommand::Pause => {
                            stream_transport.write().pause();
                        }
                        AudioCommand::Stop => {
                            stream_transport.write().stop();
                            stream_comm.set_position(0);
                        }
                        AudioCommand::Seek(pos) => {
                            stream_transport.write().seek(pos);
                            stream_comm.set_position(pos);
                        }
                        AudioCommand::SetLoop { enabled, start, end } => {
                            let mut t = stream_transport.write();
                            t.loop_enabled = enabled;
                            t.loop_start = start;
                            t.loop_end = end;
                        }
                        AudioCommand::SetTempo(tempo) => {
                            stream_transport.write().tempo = tempo;
                        }
                        _ => {}
                    }
                }

                let transport_state = stream_transport.read().state;

                if transport_state == TransportState::Playing {
                    // Render new buffer if we've consumed the current one
                    if buffer_position == 0 {
                        let transport = stream_transport.read().clone();

                        // Lock audio graph and render audio
                        if let Some(mut graph) = stream_audio_graph.try_write() {
                            graph.process(
                                &transport,
                                &mut callback_buffer_left,
                                &mut callback_buffer_right,
                            );
                        } else {
                            // If we can't get the lock, output silence to avoid blocking
                            callback_buffer_left.fill(0.0);
                            callback_buffer_right.fill(0.0);
                        }
                    }
                } else {
                    data.fill(0.0);
                    return;
                }

                let frames_needed = data.len() / 2;
                let mut out_idx = 0;

                for _ in 0..frames_needed {
                    if buffer_position >= BUFFER_SIZE {
                        buffer_position = 0;

                        // Render next buffer
                        let transport = stream_transport.read().clone();
                        if let Some(mut graph) = stream_audio_graph.try_write() {
                            graph.process(
                                &transport,
                                &mut callback_buffer_left,
                                &mut callback_buffer_right,
                            );
                        } else {
                            callback_buffer_left.fill(0.0);
                            callback_buffer_right.fill(0.0);
                        }
                    }

                    data[out_idx] = callback_buffer_left[buffer_position];
                    data[out_idx + 1] = callback_buffer_right[buffer_position];
                    out_idx += 2;
                    buffer_position += 1;

                    let pos = stream_comm.get_position();
                    stream_comm.set_position(pos + 1);
                    stream_transport.write().position = pos + 1;
                }

                let duration = start_time.elapsed();
                callback_count += 1;

                if callback_count % 100 == 0 {
                    let metrics = PerformanceMetrics {
                        callback_duration_us: duration.as_micros() as u64,
                        buffer_fill_time_us: 0,
                        cpu_usage: (duration.as_micros() as f32
                            / (frames_needed as f32 / sample_rate as f32 * 1_000_000.0))
                            * 100.0,
                        buffer_underruns: underrun_count,
                    };

                    let _ = stream_comm.message_tx.try_send(AudioMessage::PerformanceMetrics(metrics));
                }
            },
            move |err| {
                tracing::error!("Audio stream error: {}", err);
            },
            None,
        )?;

        stream.play()?;

        Ok(Self {
            _stream: stream,
            device,
            config,
            command_tx,
            message_rx,
            transport,
            comm,
        })
    }

    pub fn send_command(&self, command: AudioCommand) -> Result<()> {
        self.command_tx
            .send(command)
            .context("Failed to send command to audio thread")
    }

    pub fn try_recv_message(&self) -> Option<AudioMessage> {
        self.message_rx.try_recv().ok()
    }

    pub fn get_transport(&self) -> Transport {
        self.transport.read().clone()
    }

    pub fn get_position(&self) -> SampleTime {
        self.comm.get_position()
    }

    pub fn device_name(&self) -> String {
        self.device
            .name()
            .unwrap_or_else(|_| "Unknown".to_string())
    }

    pub fn sample_rate(&self) -> u32 {
        self.config.sample_rate.0
    }

    pub fn channels(&self) -> u16 {
        self.config.channels
    }
}

impl Drop for RealTimeAudio {
    fn drop(&mut self) {
        self.comm.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_command_clone() {
        let cmd = AudioCommand::Play;
        let _cloned = cmd.clone();
    }

    #[test]
    fn test_transport_default() {
        let transport = Transport::default();
        assert_eq!(transport.state, TransportState::Stopped);
        assert_eq!(transport.position, 0);
        assert_eq!(transport.tempo, 120.0);
    }
}
