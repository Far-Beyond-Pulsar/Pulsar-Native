//! Frame timing and GPU profiler reporting.

use super::*;

impl HelioRenderer {
    /// Record the frame's total time and warn about slow frames. Returns the
    /// total in milliseconds.
    pub(super) fn record_frame_timing(
        &mut self,
        clock: &FrameClock,
        phases: FramePhases,
        render_ms: f64,
    ) -> f32 {
        let frame_ms = clock.start.elapsed().as_secs_f32() * 1_000.0;
        self.stats_frames = (self.stats_frames.0 + 1, self.stats_frames.1.max(frame_ms));
        if frame_ms >= 50.0 {
            tracing::warn!(
                "VOXEL_FRAME_PHASES frame_ms={frame_ms:.1} {}",
                phases.describe()
            );
            tracing::warn!(
                target: "flamegraph.workload",
                frame_ms,
                render_ms,
                frame_index = self.profiler_frame_counter,
                profiling_enabled = profiling::is_profiling_enabled(),
                producer_queue = profiling::init_profiler().pending_event_count(),
                retained_events = profiling::init_profiler().retained_event_count(),
                dropped_events = profiling::init_profiler().dropped_event_count(),
                "slow Helio frame"
            );
        }
        // Emit the boundary from the render thread. The profiler collector
        // must not inspect the renderer registry or GPU mutex from a second
        // thread just to obtain this value.
        profiling::record_frame_time(frame_ms);
        self.last_phases = phases.list;
        frame_ms
    }

    /// Refresh the GPU profiler cache, emit completed GPU passes, and warn
    /// when a frame spiked.
    ///
    /// Continuous flamegraph capture needs every completed asynchronous
    /// readback. The old 30-frame cadence is kept only for the cheap
    /// always-on diagnostic cache when no instrumentation capture owns the
    /// profiler.
    pub(super) fn report_gpu_profile(&mut self, inner: &HelioInner, times: FrameTimes) {
        let profiler_id = inner.scene.renderer.profiling_instance_id();
        if profiling::is_profiling_enabled()
            || self.profiler_frame_counter >= 30
            || self.gpu_profiler_instance != profiler_id
        {
            profiling::profile_scope!("helio_gpu_profiler_update");
            self.profiler_frame_counter = 0;
            self.gpu_profiler
                .update_from_snapshot(inner.scene.renderer.timing_snapshot());
            self.gpu_profiler_instance = profiler_id;
        }

        let gpu_frame = self.gpu_profiler.gpu_frame_count;
        let gpu_sample = gpu_frame.map(|frame| (profiler_id, frame));
        let new_gpu_result = gpu_sample.is_some() && gpu_sample != self.last_reported_gpu_frame;
        if new_gpu_result {
            profiling::profile_scope!("helio_emit_gpu_passes");
            emit_helio_gpu_passes(&self.gpu_profiler, profiler_id);
        }
        let gpu_spike = new_gpu_result
            && self
                .gpu_profiler
                .total_gpu_ms
                .is_some_and(|time| time > self.spike_log_config.gpu_threshold_ms);
        let cpu_spike = times.frame_ms > self.spike_log_config.cpu_threshold_ms;
        let warning_due = self.spike_log_config.enabled
            && self
                .last_spike_warning
                .is_none_or(|last| last.elapsed() >= self.spike_log_config.min_interval);

        if warning_due && (cpu_spike || gpu_spike) {
            self.warn_frame_spike(times);
            self.last_spike_warning = Some(Instant::now());
        }
        if new_gpu_result {
            self.last_reported_gpu_frame = gpu_sample;
        }
    }

    /// Log the `HELIO FRAME SPIKE` warning with the slowest CPU and GPU pass.
    pub(super) fn warn_frame_spike(&self, times: FrameTimes) {
        let gpu_frame = self.gpu_profiler.gpu_frame_count;
        let (cpu_pass, cpu_pass_ms) = self
            .gpu_profiler
            .slowest_cpu_pass()
            .unwrap_or(("unavailable", 0.0));
        let (gpu_pass, gpu_pass_ms) = self
            .gpu_profiler
            .slowest_gpu_pass()
            .unwrap_or(("pending", 0.0));
        tracing::warn!(
            "[HELIO FRAME SPIKE] frame={:.1}ms (sync {:.1}, prepare {:.1}, submit {:.1}); \
             slowest CPU pass={} {:.1}ms; GPU frame={:?} total={:?}ms lag={:?} \
             slowest pass={} {:.1}ms drops={} overflows={}",
            times.frame_ms,
            times.sync_ms,
            times.prepare_ms,
            times.render_ms,
            cpu_pass,
            cpu_pass_ms,
            gpu_frame,
            self.gpu_profiler.total_gpu_ms,
            self.gpu_profiler.gpu_lag_frames,
            gpu_pass,
            gpu_pass_ms,
            self.gpu_profiler.readback_drops,
            self.gpu_profiler.query_overflows
        );
    }
}
