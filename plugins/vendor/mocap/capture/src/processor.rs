//! Live processing: smoothing and calibration. Both are views over the raw
//! stream; a recording stores raw samples and the calibration beside them.

use crate::frame::PoseFrame;
use mocap_format::signal::{conjugate, normalise, quat_mul, OneEuro, QuatFilter};
use mocap_format::{Calibration, ChannelKind, RigProfile};

/// One-Euro tuning shared by every channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterSettings {
    pub enabled: bool,
    /// Resting cutoff in Hz: lower is smoother.
    pub min_cutoff: f32,
    /// Speed responsiveness: higher lags less in fast motion.
    pub beta: f32,
}

impl Default for FilterSettings {
    fn default() -> Self {
        Self { enabled: true, min_cutoff: 1.5, beta: 0.4 }
    }
}

enum ChannelFilter {
    Scalar(OneEuro),
    Position([OneEuro; 3]),
    Rotation(QuatFilter),
}

/// Smooths a pose stream channel by channel.
pub struct Processor {
    settings: FilterSettings,
    filters: Vec<(usize, ChannelFilter)>,
    last_time: Option<f64>,
}

impl Processor {
    /// A processor for `rig`.
    pub fn new(rig: &RigProfile, settings: FilterSettings) -> Self {
        let filters = rig
            .channels
            .iter()
            .zip(rig.column_offsets())
            .filter(|(c, _)| c.kind != ChannelKind::Confidence)
            .map(|(c, at)| (at, Self::filter_for(c.kind, settings)))
            .collect();
        Self { settings, filters, last_time: None }
    }

    fn filter_for(kind: ChannelKind, s: FilterSettings) -> ChannelFilter {
        let one = || OneEuro::new(s.min_cutoff, s.beta);
        match kind {
            ChannelKind::Position3 => ChannelFilter::Position([one(), one(), one()]),
            ChannelKind::Rotation4 => ChannelFilter::Rotation(QuatFilter::new(s.min_cutoff, s.beta)),
            ChannelKind::Scalar | ChannelKind::Confidence => ChannelFilter::Scalar(one()),
        }
    }

    /// The current tuning.
    pub fn settings(&self) -> FilterSettings {
        self.settings
    }

    /// Changes the tuning; filter state restarts.
    pub fn set_settings(&mut self, rig: &RigProfile, settings: FilterSettings) {
        *self = Self::new(rig, settings);
    }

    /// Filters `frame` in place (a no-op when disabled).
    pub fn process(&mut self, frame: &mut PoseFrame) {
        let dt = self.last_time.map_or(1.0 / 60.0, |t| (frame.time_s - t).clamp(1e-3, 0.25)) as f32;
        self.last_time = Some(frame.time_s);
        if !self.settings.enabled {
            return;
        }
        for (at, filter) in &mut self.filters {
            let v = &mut frame.values[*at..];
            match filter {
                ChannelFilter::Scalar(f) => v[0] = f.filter(v[0], dt),
                ChannelFilter::Position(f) => {
                    for (i, axis) in f.iter_mut().enumerate() {
                        v[i] = axis.filter(v[i], dt);
                    }
                }
                ChannelFilter::Rotation(f) => {
                    let q = f.filter([v[0], v[1], v[2], v[3]], dt);
                    v[..4].copy_from_slice(&q);
                }
            }
        }
    }
}

/// Captures the performer's neutral pose from one frame: each rotation
/// channel's inverse, so later poses read relative to it, and the height scale
/// from the hips' position against `reference_hips_height`.
pub fn calibrate_neutral(rig: &RigProfile, frame: &PoseFrame, reference_hips_height: f32) -> Calibration {
    let mut calibration = Calibration { scale: 1.0, ..Calibration::default() };
    for (channel, at) in rig.channels.iter().zip(rig.column_offsets()) {
        let v = &frame.values[at..at + channel.kind.width()];
        if channel.kind == ChannelKind::Rotation4 && v.iter().all(|x| x.is_finite()) {
            calibration.offsets.insert(channel.name.clone(), conjugate(normalise([v[0], v[1], v[2], v[3]])));
        }
    }
    let hips = rig.channel_index("Hips.position").map(|i| rig.column_offsets()[i] + 1);
    if let Some(y) = hips.map(|at| frame.values[at]).filter(|y| y.is_finite() && *y > 0.1) {
        calibration.scale = reference_hips_height / y;
    }
    calibration
}

/// Applies a calibration to a frame in place: rotations relative to the
/// neutral pose, positions scaled.
pub fn apply_calibration(rig: &RigProfile, calibration: &Calibration, frame: &mut PoseFrame) {
    for (channel, at) in rig.channels.iter().zip(rig.column_offsets()) {
        // Triangulated keypoints are already metres in the subject's world: never rescaled.
        if channel.name.contains("kp3d.") {
            continue;
        }
        let v = &mut frame.values[at..at + channel.kind.width()];
        match channel.kind {
            ChannelKind::Rotation4 => {
                if let Some(offset) = calibration.offsets.get(&channel.name) {
                    let q = quat_mul(*offset, [v[0], v[1], v[2], v[3]]);
                    v.copy_from_slice(&q);
                }
            }
            ChannelKind::Position3 => v.iter_mut().for_each(|x| *x *= calibration.scale),
            _ => {}
        }
    }
}
