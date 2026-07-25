//! Core data types for the Helio renderer

use glam::Vec3;

/// Rendering metrics
#[derive(Debug, Clone, Default)]
pub struct RenderMetrics {
    pub fps: f32,
    pub frame_time_ms: f32,
    pub draw_calls: u32,
    pub memory_usage_mb: f32,
    pub vertices_drawn: u64,
    pub frames_rendered: u64,
    pub pipeline_time_us: f32,
}

/// Represents a single diagnostic metric for GPU profiling
#[derive(Debug, Clone)]
pub struct DiagnosticMetric {
    pub name: &'static str,
    pub cpu_ms: Option<f32>,
    pub gpu_ms: Option<f32>,
}

/// Availability of asynchronous GPU timestamp data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GpuProfilerAvailability {
    Disabled,
    Unsupported,
    #[default]
    Pending,
    Available,
    Backpressured,
}

/// GPU Pipeline profiling data
#[derive(Debug, Clone, Default)]
pub struct GpuProfilerData {
    pub frame_count: u64,
    pub gpu_frame_count: Option<u64>,
    pub gpu_lag_frames: Option<u64>,
    pub availability: GpuProfilerAvailability,
    pub total_cpu_ms: Option<f32>,
    pub total_gpu_ms: Option<f32>,
    pub readback_drops: u64,
    pub query_overflows: u64,
    pub render_metrics: Vec<DiagnosticMetric>,
}

impl GpuProfilerData {
    /// Refresh this reusable host-side cache from Helio's read-only snapshot.
    ///
    /// The pass vector keeps its capacity between frames and pass names are
    /// static, so steady-state collection does not allocate or poll the GPU.
    pub fn update_from_snapshot(&mut self, snapshot: &helio::RenderTimingSnapshot) {
        self.frame_count = snapshot.cpu_frame_index;
        self.gpu_frame_count = snapshot.gpu_frame_index;
        self.gpu_lag_frames = snapshot.gpu_lag_frames;
        self.availability = match snapshot.gpu_availability {
            helio::GpuTimingAvailability::Disabled => GpuProfilerAvailability::Disabled,
            helio::GpuTimingAvailability::Unsupported => GpuProfilerAvailability::Unsupported,
            helio::GpuTimingAvailability::Pending => GpuProfilerAvailability::Pending,
            helio::GpuTimingAvailability::Available => GpuProfilerAvailability::Available,
            helio::GpuTimingAvailability::Backpressured => GpuProfilerAvailability::Backpressured,
        };
        self.total_cpu_ms = snapshot.total_cpu_ms;
        self.total_gpu_ms = snapshot.total_gpu_ms;
        self.readback_drops = snapshot.readback_drops;
        self.query_overflows = snapshot.query_overflows;

        self.render_metrics.clear();
        self.render_metrics
            .extend(snapshot.passes.iter().map(|pass| DiagnosticMetric {
                name: pass.name,
                cpu_ms: pass.cpu_ms,
                gpu_ms: pass.gpu_ms,
            }));
    }

    pub fn slowest_cpu_pass(&self) -> Option<(&'static str, f32)> {
        self.render_metrics
            .iter()
            .filter_map(|metric| metric.cpu_ms.map(|time| (metric.name, time)))
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
    }

    pub fn slowest_gpu_pass(&self) -> Option<(&'static str, f32)> {
        self.render_metrics
            .iter()
            .filter_map(|metric| metric.gpu_ms.map(|time| (metric.name, time)))
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
    }
}

/// Camera controller input state
#[derive(Default, Clone)]
pub struct CameraInput {
    pub forward: f32,
    pub right: f32,
    pub up: f32,
    pub mouse_delta_x: f32,
    pub mouse_delta_y: f32,
    pub pan_delta_x: f32,
    pub pan_delta_y: f32,
    pub zoom_delta: f32,
    pub move_speed: f32,
    pub pan_speed: f32,
    pub zoom_speed: f32,
    pub look_sensitivity: f32,
    pub boost: bool,
    pub orbit_mode: bool,
    pub orbit_distance: f32,
    pub focus_point: Vec3,
    pub viewport_x: f32,
    pub viewport_y: f32,
    pub viewport_width: f32,
    pub viewport_height: f32,
    pub needs_resize: bool,
}

impl CameraInput {
    pub fn new() -> Self {
        Self {
            forward: 0.0,
            right: 0.0,
            up: 0.0,
            mouse_delta_x: 0.0,
            mouse_delta_y: 0.0,
            pan_delta_x: 0.0,
            pan_delta_y: 0.0,
            zoom_delta: 0.0,
            move_speed: 10.0,      // Units per second
            pan_speed: 5.0,        // Pan sensitivity
            zoom_speed: 20.0,      // Zoom sensitivity
            look_sensitivity: 0.3, // Match Helio's default FpsCamera look_speed
            boost: false,
            orbit_mode: false,
            orbit_distance: 10.0,
            focus_point: Vec3::ZERO,
            viewport_x: 0.0,
            viewport_y: 0.0,
            viewport_width: 2560.0,
            viewport_height: 1440.0,
            needs_resize: false,
        }
    }

    pub fn clear_transient_deltas(&mut self) {
        self.mouse_delta_x = 0.0;
        self.mouse_delta_y = 0.0;
        self.pan_delta_x = 0.0;
        self.pan_delta_y = 0.0;
        self.zoom_delta = 0.0;
    }

    pub fn accumulate_look_delta(&mut self, dx: f32, dy: f32) {
        self.mouse_delta_x += dx;
        self.mouse_delta_y += dy;
    }

    pub fn accumulate_pan_delta(&mut self, dx: f32, dy: f32) {
        self.pan_delta_x += dx;
        self.pan_delta_y += dy;
    }
}

#[cfg(test)]
mod tests {
    use super::{GpuProfilerAvailability, GpuProfilerData};

    #[test]
    fn timing_snapshot_bridge_preserves_pass_order_and_unavailable_gpu_state() {
        let snapshot = helio::RenderTimingSnapshot {
            generation: 4,
            cpu_frame_index: 18,
            gpu_frame_index: None,
            gpu_lag_frames: None,
            gpu_availability: helio::GpuTimingAvailability::Unsupported,
            total_cpu_ms: Some(7.0),
            total_gpu_ms: None,
            readback_drops: 0,
            query_overflows: 1,
            passes: vec![
                helio::RenderPassTiming {
                    name: "Prepare",
                    cpu_ms: Some(2.0),
                    gpu_ms: None,
                },
                helio::RenderPassTiming {
                    name: "Draw",
                    cpu_ms: Some(5.0),
                    gpu_ms: None,
                },
            ],
        };

        let mut data = GpuProfilerData::default();
        data.update_from_snapshot(&snapshot);

        assert_eq!(data.availability, GpuProfilerAvailability::Unsupported);
        assert_eq!(data.query_overflows, 1);
        assert_eq!(
            data.render_metrics
                .iter()
                .map(|metric| metric.name)
                .collect::<Vec<_>>(),
            ["Prepare", "Draw"]
        );
        assert_eq!(data.slowest_cpu_pass(), Some(("Draw", 5.0)));
        assert_eq!(data.slowest_gpu_pass(), None);
    }

    #[test]
    fn timing_snapshot_bridge_attributes_delayed_gpu_spike() {
        let snapshot = helio::RenderTimingSnapshot {
            generation: 7,
            cpu_frame_index: 21,
            gpu_frame_index: Some(19),
            gpu_lag_frames: Some(2),
            gpu_availability: helio::GpuTimingAvailability::Available,
            total_cpu_ms: Some(1.0),
            total_gpu_ms: Some(44.0),
            readback_drops: 2,
            query_overflows: 0,
            passes: vec![
                helio::RenderPassTiming {
                    name: "FastPass",
                    cpu_ms: Some(0.5),
                    gpu_ms: Some(4.0),
                },
                helio::RenderPassTiming {
                    name: "ForcedSlowPass",
                    cpu_ms: Some(0.5),
                    gpu_ms: Some(40.0),
                },
            ],
        };

        let mut data = GpuProfilerData::default();
        data.update_from_snapshot(&snapshot);

        assert_eq!(data.gpu_frame_count, Some(19));
        assert_eq!(data.gpu_lag_frames, Some(2));
        assert_eq!(data.readback_drops, 2);
        assert_eq!(data.slowest_gpu_pass(), Some(("ForcedSlowPass", 40.0)));
    }
}
