//! The VMC protocol source: OSC over UDP, as sent by VRM trackers and by
//! Rokoko, Xsens and Perception Neuron bridges.
//!
//! `/VMC/Ext/Bone/Pos` carries a bone's local transform, `/VMC/Ext/Root/Pos`
//! the avatar's world transform, `/VMC/Ext/Blend/Val` a blendshape and
//! `/VMC/Ext/Blend/Apply` ends a frame. A sender that never applies is still
//! sampled at 60 Hz so its data is not held back.

use crate::error::CaptureError;
use crate::frame::PoseFrame;
use crate::osc::{parse_packet, OscMessage};
use crate::rigs::{vrm_humanoid, RigIndex};
use crate::source::{
    spawn_capture_loop, CaptureSource, SourceDescriptor, SourceKind, SourceShared, SourceSink,
    SourceState, SourceStatus,
};
use editor_task_queue::ServiceHandle;
use mocap_format::RigProfile;
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::{Duration, Instant};

const COMMIT_ADDRESS: &str = "/VMC/Ext/Blend/Apply";
const FLUSH_INTERVAL: Duration = Duration::from_millis(16);

/// Turns VMC messages into dense pose frames; holds the last value of every
/// channel so a frame is always complete.
pub struct VmcDecoder {
    index: RigIndex,
    values: Vec<f32>,
    sequence: u64,
    dirty: bool,
}

impl VmcDecoder {
    /// A decoder for `rig` (the VRM humanoid for a VMC source).
    pub fn new(rig: &RigProfile) -> Self {
        let index = RigIndex::new(rig);
        Self { values: vec![f32::NAN; index.width()], index, sequence: 0, dirty: false }
    }

    fn set(&mut self, channel: &str, values: &[Option<f32>]) {
        let Some(at) = self.index.column(channel) else { return };
        if values.iter().any(Option::is_none) {
            return;
        }
        for (i, v) in values.iter().flatten().enumerate() {
            self.values[at + i] = *v;
        }
        self.dirty = true;
    }

    /// Applies one message; returns true when it commits a frame.
    pub fn apply(&mut self, message: &OscMessage) -> bool {
        let f = |i| message.float(i);
        match message.address.as_str() {
            "/VMC/Ext/Bone/Pos" => {
                let Some(name) = message.string(0) else { return false };
                self.set(&format!("{name}.rotation"), &[f(4), f(5), f(6), f(7)]);
                if name == "Hips" {
                    self.set("Hips.position", &[f(1), f(2), f(3)]);
                }
            }
            "/VMC/Ext/Root/Pos" => {
                self.set("Root.position", &[f(1), f(2), f(3)]);
                self.set("Root.rotation", &[f(4), f(5), f(6), f(7)]);
            }
            "/VMC/Ext/Blend/Val" => {
                if let Some(name) = message.string(0) {
                    self.set(&format!("blend.{name}"), &[f(1)]);
                }
            }
            COMMIT_ADDRESS => return true,
            _ => {}
        }
        false
    }

    /// True when something changed since the last [`snapshot`](Self::snapshot).
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

/// Listens on a UDP port for VMC.
pub struct VmcSource {
    port: u16,
    label: String,
    shared: Arc<SourceShared>,
    handle: Option<ServiceHandle<()>>,
}

impl VmcSource {
    /// A source listening on `port` (39539 is VMC's default) once started.
    pub fn new(port: u16) -> Self {
        Self {
            port,
            label: format!("VMC (UDP {port})"),
            shared: SourceShared::new(),
            handle: None,
        }
    }
}

fn run(socket: UdpSocket, sink: SourceSink, shared: Arc<SourceShared>, ctx: &editor_task_queue::TaskContext) {
    let mut decoder = VmcDecoder::new(&vrm_humanoid());
    let mut buffer = vec![0u8; 65_535];
    let mut last_flush = Instant::now();
    shared.set_state(SourceState::Live);
    while !ctx.is_cancelled() {
        ctx.heartbeat();
        let committed = match socket.recv(&mut buffer) {
            Ok(n) => parse_packet(&buffer[..n]).iter().fold(false, |c, m| decoder.apply(m) | c),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => false,
            Err(e) => {
                tracing::warn!("vmc receive failed: {e}");
                shared.set_state(SourceState::Failed(e.to_string()));
                return;
            }
        };
        let stale = last_flush.elapsed() >= FLUSH_INTERVAL;
        if decoder.has_pending() && (committed || stale) {
            sink.poses.push(decoder.snapshot(sink.clock.now_s()));
            shared.frame_delivered();
            last_flush = Instant::now();
        }
    }
    shared.set_state(SourceState::Idle);
}

impl CaptureSource for VmcSource {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            id: format!("vmc:{}", self.port),
            kind: SourceKind::Vmc,
            label: self.label.clone(),
            rig: Some(vrm_humanoid()),
            has_video: false,
        }
    }

    fn start(&mut self, sink: SourceSink) -> Result<(), CaptureError> {
        if self.handle.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        self.shared.set_state(SourceState::Starting);
        let open = |reason: std::io::Error| CaptureError::Open {
            what: format!("UDP port {}", self.port),
            reason: reason.to_string(),
        };
        let socket = UdpSocket::bind(("0.0.0.0", self.port)).map_err(open)?;
        socket.set_read_timeout(Some(Duration::from_millis(20))).map_err(open)?;
        let shared = Arc::clone(&self.shared);
        let handle = spawn_capture_loop("VMC receiver", &self.label, move |ctx| {
            run(socket, sink, shared, ctx)
        })?;
        self.handle = Some(handle);
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

impl Drop for VmcSource {
    fn drop(&mut self) {
        self.stop();
    }
}
