//! Webcam capture (`camera` feature).
//!
//! The camera is opened on its own capture thread (a camera handle is not
//! `Send` on every platform) and every frame is decoded to RGBA into the
//! session's [`VideoMailbox`]: the viewport uploads the newest, nothing queues.

use crate::error::CaptureError;
use crate::frame::{PixelFormat, VideoFrame};
use crate::source::{
    spawn_capture_loop, CaptureSource, SourceDescriptor, SourceKind, SourceShared, SourceSink,
    SourceState, SourceStatus,
};
use editor_task_queue::{ServiceHandle, TaskContext};
use nokhwa::pixel_format::RgbAFormat;
use nokhwa::utils::{
    ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    Resolution,
};
use nokhwa::Camera;
use std::sync::Arc;

/// A camera the OS lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraInfo {
    /// The OS index, as a string (`"0"`).
    pub index: String,
    pub name: String,
}

/// A capture mode a camera is asked for (it picks the closest it offers).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CameraMode {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
}

impl Default for CameraMode {
    fn default() -> Self {
        Self { width: 1280, height: 720, fps: 30 }
    }
}

/// Lists the cameras the OS reports. Enumeration can take seconds: run it as a
/// task, never on the UI thread.
pub fn enumerate_cameras() -> Result<Vec<CameraInfo>, CaptureError> {
    let cameras = nokhwa::query(ApiBackend::Auto).map_err(|e| CaptureError::Open {
        what: "the camera list".to_owned(),
        reason: e.to_string(),
    })?;
    Ok(cameras
        .into_iter()
        .map(|c| CameraInfo { index: c.index().to_string(), name: c.human_name() })
        .collect())
}

/// One camera as a capture source.
pub struct CameraSource {
    info: CameraInfo,
    mode: CameraMode,
    shared: Arc<SourceShared>,
    handle: Option<ServiceHandle<()>>,
}

impl CameraSource {
    /// A source for `info` asking for `mode`.
    pub fn new(info: CameraInfo, mode: CameraMode) -> Self {
        Self { info, mode, shared: SourceShared::new(), handle: None }
    }
}

fn open(info: &CameraInfo, mode: CameraMode) -> Result<Camera, String> {
    let index = info.index.parse::<u32>().map(CameraIndex::Index).unwrap_or_else(|_| CameraIndex::String(info.index.clone()));
    let wanted = CameraFormat::new(Resolution::new(mode.width, mode.height), FrameFormat::MJPEG, mode.fps);
    let requested = RequestedFormat::new::<RgbAFormat>(RequestedFormatType::Closest(wanted));
    let mut camera = Camera::new(index, requested).map_err(|e| e.to_string())?;
    camera.open_stream().map_err(|e| e.to_string())?;
    Ok(camera)
}

fn run(info: CameraInfo, mode: CameraMode, sink: SourceSink, shared: Arc<SourceShared>, ctx: &TaskContext) {
    let mut camera = match open(&info, mode) {
        Ok(camera) => camera,
        Err(reason) => {
            tracing::warn!("camera {} did not open: {reason}", info.name);
            shared.set_state(SourceState::Failed(reason));
            return;
        }
    };
    shared.set_state(SourceState::Live);
    let mut sequence = 0u64;
    let mut failing = false;
    while !ctx.is_cancelled() {
        ctx.heartbeat();
        let decoded = camera.frame().map_err(|e| e.to_string()).and_then(|buffer| decode(&buffer));
        match decoded {
            Ok(image) => {
                let (width, height) = image.dimensions();
                sink.video.put(VideoFrame {
                    width,
                    height,
                    format: PixelFormat::Rgba8,
                    data: image.into_raw().into(),
                    sequence,
                    time_s: sink.clock.now_s(),
                });
                sequence += 1;
                failing = false;
                shared.frame_delivered();
            }
            Err(e) => {
                if !failing {
                    tracing::warn!("camera {} is not delivering frames: {e}", info.name);
                }
                failing = true;
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
    if let Err(e) = camera.stop_stream() {
        tracing::debug!("camera {} did not stop cleanly: {e}", info.name);
    }
    shared.set_state(SourceState::Idle);
}

impl CaptureSource for CameraSource {
    fn descriptor(&self) -> SourceDescriptor {
        SourceDescriptor {
            id: format!("camera:{}", self.info.index),
            kind: SourceKind::Camera,
            label: self.info.name.clone(),
            rig: None,
            has_video: true,
        }
    }

    fn start(&mut self, sink: SourceSink) -> Result<(), CaptureError> {
        if self.handle.is_some() {
            return Err(CaptureError::AlreadyRunning);
        }
        self.shared.set_state(SourceState::Starting);
        let (info, mode, shared) = (self.info.clone(), self.mode, Arc::clone(&self.shared));
        let title = format!("Camera: {}", info.name);
        self.handle = Some(spawn_capture_loop(&title, "camera", move |ctx| run(info, mode, sink, shared, ctx))?);
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

impl Drop for CameraSource {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Decodes a camera buffer to RGBA. MJPEG (what most webcams send at their
/// higher modes) goes through the `image` crate, so the build needs no native
/// JPEG library; every other format is converted by `nokhwa`.
fn decode(buffer: &nokhwa::Buffer) -> Result<image::RgbaImage, String> {
    if buffer.source_frame_format() == FrameFormat::MJPEG {
        return image::load_from_memory_with_format(buffer.buffer(), image::ImageFormat::Jpeg)
            .map(|decoded| decoded.to_rgba8())
            .map_err(|e| format!("MJPEG frame did not decode: {e}"));
    }
    buffer.decode_image::<RgbAFormat>().map_err(|e| e.to_string())
}
