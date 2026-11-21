//! A generic OSC sensor source: any device or app that can send OSC floats
//! (IMU boards, gloves, trackers, phones, custom hardware) becomes channels in
//! a take. Each declared channel maps one OSC address to a typed channel.

use crate::error::CaptureError;
use crate::frame::PoseFrame;
use crate::osc::{parse_packet, OscMessage};
use crate::rigs::RigIndex;
use crate::source::{
    spawn_capture_loop, CaptureSource, SourceDescriptor, SourceKind, SourceShared, SourceSink,
    SourceState, SourceStatus,
};
use editor_task_queue::ServiceHandle;
use mocap_format::{ChannelDesc, ChannelKind, Quantisation, RigProfile};
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Duration;

/// One OSC address mapped to one channel: its floats fill the channel's columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SensorChannel {
    pub name: String,
    pub address: String,
    pub kind: ChannelKind,
}

impl SensorChannel {
    /// A channel called `name` read from `address`.
    pub fn new(name: &str, address: &str, kind: ChannelKind) -> Self {
        Self { name: name.to_owned(), address: address.to_owned(), kind }
    }
}

/// The rig of a sensor with `channels`: no joints, one channel each.
pub fn sensor_rig(channels: &[SensorChannel]) -> RigProfile {
    RigProfile {
        name: "osc-sensor".to_owned(),
        joints: Vec::new(),
        channels: channels
            .iter()
            .map(|c| ChannelDesc {
                name: c.name.clone(),
                kind: c.kind,
                joint: None,
                quantisation: Quantisation::Lossless,
            })
            .collect(),
    }
}

/// Maps OSC messages to a dense frame.
pub struct SensorDecoder {
    channels: Vec<(String, usize, usize)>,
    values: Vec<f32>,
    sequence: u64,
    dirty: bool,
}

impl SensorDecoder {
    /// A decoder for `channels`.
    pub fn new(channels: &[SensorChannel]) -> Self {
        let rig = sensor_rig(channels);
        let index = RigIndex::new(&rig);
        let map = channels
            .iter()
            .filter_map(|c| Some((c.address.clone(), index.column(&c.name)?, c.kind.width())))
            .collect();
        Self { channels: map, values: vec![f32::NAN; index.width()], sequence: 0, dirty: false }
    }

    /// Applies a message; a message without enough floats is ignored.
    pub fn apply(&mut self, message: &OscMessage) {
        for (address, at, width) in &self.channels {
            if *address != message.address {
                continue;
            }
            let read: Vec<f32> = (0..*width).filter_map(|i| message.float(i)).collect();
            if read.len() == *width {
                self.values[*at..at + width].copy_from_slice(&read);
                self.dirty = true;
            }
        }
    }

    /// True when something changed since the last snapshot.
    pub fn has_pending(&self) -> bool {
        self.dirty
    }

    /// The current values as a frame stamped `time_s`.
    pub fn snapshot(&mut self, time_s: f64) -> PoseFrame {
        self.sequence += 1;
        self.dirty = false;
        PoseFrame { sequence: self.sequence, time_s, values: self.values.clone() }
    }
}

/// Listens on a UDP port for OSC sensor data.
pub struct OscSensorSource {
    port: u16,
    label: String,
    channels: Vec<SensorChannel>,
    shared: Arc<SourceShared>,
    handle: Option<ServiceHandle<()>>,
}

impl OscSensorSource {
    /// A sensor on `port` with `channels`.
    pub fn new(port: u16, label: &str, channels: Vec<SensorChannel>) -> Self {
        Self { port, label: label.to_owned(), channels, shared: SourceShared::new(), handle: None }
    }

    /// A common IMU: `/imu/quat` (x y z w), `/imu/accel` and `/imu/gyro` (x y z).
    pub fn imu(port: u16) -> Self {
        Self::new(
            port,
            &format!("IMU sensor (OSC {port})"),
            vec![
                SensorChannel::new("imu.rotation", "/imu/quat", ChannelKind::Rotation4),
                SensorChannel::new("imu.accel", "/imu/accel", ChannelKind::Position3),
                SensorChannel::new("imu.gyro", "/imu/gyro", ChannelKind::Position3),
            ],
        )
    }
}

impl CaptureSource for OscSensorSource {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            id: format!("sensor:{}", self.port),
            kind: SourceKind::Sensor,
            label: self.label.clone(),
            rig: Some(sensor_rig(&self.channels)),
            has_video: false,
        }
    }

    fn start(&mut self, sink: SourceSink) -> Result<(), CaptureError> {
        if self.handle.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        self.shared.set_state(SourceState::Starting);
        let open = |e: std::io::Error| CaptureError::Open {
            what: format!("UDP port {}", self.port),
            reason: e.to_string(),
        };
        let socket = UdpSocket::bind(("0.0.0.0", self.port)).map_err(open)?;
        socket.set_read_timeout(Some(Duration::from_millis(20))).map_err(open)?;
        let (shared, channels) = (Arc::clone(&self.shared), self.channels.clone());
        self.handle = Some(spawn_capture_loop("OSC sensor", &self.label, move |ctx| {
            let mut decoder = SensorDecoder::new(&channels);
            let mut buffer = vec![0u8; 65_535];
            shared.set_state(SourceState::Live);
            while !ctx.is_cancelled() {
                ctx.heartbeat();
                if let Ok(n) = socket.recv(&mut buffer) {
                    parse_packet(&buffer[..n]).iter().for_each(|m| decoder.apply(m));
                    if decoder.has_pending() {
                        sink.poses.push(decoder.snapshot(sink.clock.now_s()));
                        shared.frame_delivered();
                    }
                }
            }
            shared.set_state(SourceState::Idle);
        })?);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.request_stop();
            let _ = handle.join();
        }
        self.shared.set_state(SourceState::Idle);
    }

    fn status(&self) -> SourceStatus {
        self.shared.status()
    }
}

impl Drop for OscSensorSource {
    fn drop(&mut self) {
        self.stop();
    }
}
