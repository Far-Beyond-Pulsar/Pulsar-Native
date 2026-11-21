//! The recorder: pose frames in at the source's own timing, a fixed-rate take
//! out. Raw samples only; filters and calibration are stored beside, never
//! baked in.

use crate::error::CaptureError;
use crate::frame::PoseFrame;
use mocap_format::signal::nlerp;
use mocap_format::{Calibration, ChannelKind, Marker, RigProfile, TakeMeta, TakeWriter};
use std::io::Write;

/// A source silent for longer than this records gaps (`NaN`), never a
/// fabricated interpolation across the silence.
pub const MAX_HOLD_S: f64 = 0.25;

/// Counters the HUD shows while recording.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordStats {
    pub frames: u64,
    /// Frames recorded as gaps because the source went silent.
    pub gap_frames: u64,
}

/// Resamples a pose stream to the take's frame rate and writes it.
pub struct Recorder<W: Write> {
    writer: TakeWriter<W>,
    kinds: Vec<(ChannelKind, usize)>,
    fps: f64,
    previous: Option<PoseFrame>,
    origin_s: f64,
    stats: RecordStats,
    scratch: Vec<f32>,
}

impl<W: Write> Recorder<W> {
    /// Starts a take for `rig` writing to `out`.
    pub fn new(out: W, meta: &TakeMeta, rig: &RigProfile, block_frames: usize) -> Result<Self, CaptureError> {
        let kinds = rig.channels.iter().zip(rig.column_offsets()).map(|(c, at)| (c.kind, at)).collect();
        Ok(Self {
            writer: TakeWriter::new(out, meta, rig, block_frames)?,
            kinds,
            fps: meta.fps(),
            previous: None,
            origin_s: 0.0,
            stats: RecordStats::default(),
            scratch: vec![0.0; rig.columns()],
        })
    }

    /// Counters so far.
    pub fn stats(&self) -> RecordStats {
        self.stats
    }

    /// Records every output frame whose time falls up to `frame`'s.
    pub fn push(&mut self, frame: &PoseFrame) -> Result<(), CaptureError> {
        let Some(previous) = self.previous.take() else {
            self.origin_s = frame.time_s;
            self.write(&frame.values, false)?;
            self.previous = Some(frame.clone());
            return Ok(());
        };
        if frame.time_s <= previous.time_s {
            self.previous = Some(previous);
            return Ok(());
        }
        loop {
            let t = self.origin_s + self.stats.frames as f64 / self.fps;
            if t > frame.time_s {
                break;
            }
            let span = frame.time_s - previous.time_s;
            if span > MAX_HOLD_S {
                self.scratch.fill(f32::NAN);
                self.write_scratch(true)?;
            } else {
                let f = ((t - previous.time_s) / span) as f32;
                self.interpolate(&previous, frame, f);
                self.write_scratch(false)?;
            }
        }
        self.previous = Some(frame.clone());
        Ok(())
    }

    fn interpolate(&mut self, a: &PoseFrame, b: &PoseFrame, f: f32) {
        for (kind, at) in &self.kinds {
            let n = kind.width();
            if *kind == ChannelKind::Rotation4 {
                let q = nlerp(
                    [a.values[*at], a.values[at + 1], a.values[at + 2], a.values[at + 3]],
                    [b.values[*at], b.values[at + 1], b.values[at + 2], b.values[at + 3]],
                    f,
                );
                self.scratch[*at..at + 4].copy_from_slice(&q);
            } else {
                for i in *at..at + n {
                    self.scratch[i] = a.values[i] + (b.values[i] - a.values[i]) * f;
                }
            }
        }
    }

    fn write(&mut self, values: &[f32], gap: bool) -> Result<(), CaptureError> {
        self.scratch.copy_from_slice(values);
        self.write_scratch(gap)
    }

    fn write_scratch(&mut self, gap: bool) -> Result<(), CaptureError> {
        self.writer.push_frame(&self.scratch)?;
        self.stats.frames += 1;
        self.stats.gap_frames += u64::from(gap);
        Ok(())
    }

    /// Marks the frame about to be recorded.
    pub fn mark(&mut self, label: &str) -> Result<(), CaptureError> {
        let marker = Marker { frame: self.stats.frames, label: label.to_owned() };
        Ok(self.writer.add_marker(&marker)?)
    }

    /// Stores the calibration the performer was captured under.
    pub fn set_calibration(&mut self, calibration: &Calibration) -> Result<(), CaptureError> {
        Ok(self.writer.set_calibration(calibration)?)
    }

    /// Writes the last block and the index; returns the destination.
    pub fn finish(self) -> Result<W, CaptureError> {
        Ok(self.writer.finish()?)
    }
}
