//! Bridge delayed Helio GPU durations into Pulsar's saved instrumentation trace.

use super::core::GpuProfilerData;

pub(super) fn emit_helio_gpu_passes(data: &GpuProfilerData, profiler_id: u64) {
    if !profiling::is_profiling_enabled() {
        return;
    }
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let profiler = profiling::init_profiler();
    visit_gpu_events(
        data,
        profiler_id,
        now_ns,
        profiler.get_process_id(),
        |event| {
            profiler.submit_event(event);
        },
    );
}

/// Only durations and source-frame IDs are available from this snapshot.
/// Placement is deliberately labelled as an estimate: it cannot establish
/// GPU/CPU overlap, queue latency, or the original order of GPU scopes.
fn visit_gpu_events(
    data: &GpuProfilerData,
    profiler_id: u64,
    received_ns: u64,
    process_id: u32,
    mut emit: impl FnMut(profiling::ProfileEvent),
) {
    let (Some(gpu_frame), Some(total_gpu_ms)) = (data.gpu_frame_count, data.total_gpu_ms) else {
        return;
    };
    if !total_gpu_ms.is_finite() || total_gpu_ms < 0.0 {
        return;
    }
    let duration_ns = (f64::from(total_gpu_ms) * 1_000_000.0) as u64;
    let mut cursor_ns = received_ns.saturating_sub(duration_ns);
    let gpu_frame_scope_id = profiling::allocate_scope_id();
    let metadata = format!(
        "domain=helio;track=gpu;profiler_id={};cpu_frame={};gpu_frame={};lag_frames={};\
         readback_drops={};query_overflows={};received_ns={};\
         placement=readback_estimate;pass_order=packed_durations",
        profiler_id,
        data.frame_count,
        gpu_frame,
        data.gpu_lag_frames
            .map_or_else(|| "unknown".into(), |lag| lag.to_string()),
        data.readback_drops,
        data.query_overflows,
        received_ns,
    );
    emit(profiling::ProfileEvent {
        scope_id: gpu_frame_scope_id,
        // The CPU scope active at readback did not submit this GPU frame.
        // Keep the root unparented until an actual submission link is available.
        parent_scope_id: None,
        name: "helio_frame (estimated GPU placement)".into(),
        thread_id: 0,
        thread_name: Some("GPU".into()),
        process_id,
        parent_name: None,
        start_ns: cursor_ns,
        duration_ns,
        depth: 0,
        location: None,
        metadata: Some(metadata.clone()),
        track_name: Some("GPU".into()),
    });
    let mut voxel_planet_scope = None;
    for pass in &data.render_metrics {
        // These enclose the pass durations. Appending them to the children
        // duplicated the entire graph and extended it beyond the frame bar.
        if matches!(
            pass.name,
            "__graph_frame" | "__graph_compute" | "__graph_graphics"
        ) {
            continue;
        }
        let Some(gpu_ms) = pass.gpu_ms.filter(|ms| ms.is_finite() && *ms >= 0.0) else {
            continue;
        };
        let duration_ns = (f64::from(gpu_ms) * 1_000_000.0) as u64;
        let scope_id = profiling::allocate_scope_id();
        if pass.name == "VoxelPlanet" {
            voxel_planet_scope = Some((scope_id, cursor_ns, duration_ns));
        }
        emit(profiling::ProfileEvent {
            scope_id,
            parent_scope_id: Some(gpu_frame_scope_id),
            name: pass.name.into(),
            thread_id: 0,
            thread_name: Some("GPU".into()),
            process_id,
            parent_name: Some("helio_frame (estimated GPU placement)".into()),
            start_ns: cursor_ns,
            duration_ns,
            depth: 1,
            location: None,
            metadata: Some(metadata.clone()),
            track_name: Some("GPU".into()),
        });
        cursor_ns = cursor_ns.saturating_add(duration_ns);
    }

    if let Some((parent_scope_id, parent_start_ns, parent_duration_ns)) = voxel_planet_scope {
        let mut child_cursor_ns = parent_start_ns;
        let mut remaining_ns = parent_duration_ns;
        for stage in &data.voxel_planet_stages {
            let Some(gpu_ms) = stage.gpu_ms.filter(|ms| ms.is_finite() && *ms >= 0.0) else {
                continue;
            };
            let duration_ns = ((f64::from(gpu_ms) * 1_000_000.0) as u64).min(remaining_ns);
            emit(profiling::ProfileEvent {
                scope_id: profiling::allocate_scope_id(),
                parent_scope_id: Some(parent_scope_id),
                name: stage.name.into(),
                thread_id: 0,
                thread_name: Some("GPU".into()),
                process_id,
                parent_name: Some("VoxelPlanet".into()),
                start_ns: child_cursor_ns,
                duration_ns,
                depth: 2,
                location: None,
                metadata: Some(metadata.clone()),
                track_name: Some("GPU".into()),
            });
            child_cursor_ns = child_cursor_ns.saturating_add(duration_ns);
            remaining_ns = remaining_ns.saturating_sub(duration_ns);
            if remaining_ns == 0 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subsystems::render::helio_renderer::DiagnosticMetric;

    #[test]
    fn graph_snapshot_exports_voxel_subscopes_under_the_parent() {
        let mut snapshot = helio::RenderTimingSnapshot {
            cpu_frame_index: 21,
            gpu_frame_index: Some(19),
            total_gpu_ms: Some(100.0),
            passes: [
                ("VoxelPlanet", 95.0),
                ("Lighting", 5.0),
                ("VoxelPlanet::residency", 2.0),
                ("VoxelPlanet::primary", 90.0),
                ("VoxelPlanet::shade", 3.0),
            ]
            .into_iter()
            .map(|(name, ms)| helio::RenderPassTiming {
                name,
                cpu_ms: None,
                gpu_ms: Some(ms),
            })
            .collect(),
            ..Default::default()
        };
        let mut data = GpuProfilerData::default();
        data.update_from_snapshot(&snapshot);
        assert_eq!(data.render_metrics.len(), 2);
        assert_eq!(data.voxel_planet_stages.len(), 3);
        assert_eq!(data.total_gpu_ms, Some(100.0));
        let mut events = Vec::new();
        visit_gpu_events(&data, 9, 200_000_000, 42, |event| events.push(event));
        assert_eq!(events.len(), 6);
        let parent = events.iter().find(|event| event.name == "VoxelPlanet").unwrap();
        let primary = events.iter().find(|event| event.name == "primary").unwrap();
        assert_eq!(primary.parent_scope_id, Some(parent.scope_id));
        assert_eq!(primary.depth, 2);
        assert_eq!(primary.duration_ns, 90_000_000);
        assert!(primary.metadata.as_deref().unwrap().contains("gpu_frame=19;"));

        // A later frame with no voxel work must not retain old children.
        snapshot.passes.clear();
        data.update_from_snapshot(&snapshot);
        assert!(data.voxel_planet_stages.is_empty());
    }

    #[test]
    fn delayed_gpu_trace_preserves_source_and_excludes_graph_envelopes() {
        let data = GpuProfilerData {
            frame_count: 21,
            gpu_frame_count: Some(19),
            gpu_lag_frames: Some(2),
            total_gpu_ms: Some(7.0),
            readback_drops: 3,
            query_overflows: 4,
            render_metrics: [
                ("VoxelPlanet", Some(5.0)),
                ("Lighting", Some(2.0)),
                ("Pending", None),
                ("__graph_frame", Some(7.0)),
                ("__graph_compute", Some(5.0)),
                ("__graph_graphics", Some(2.0)),
            ]
            .into_iter()
            .map(|(name, gpu_ms)| DiagnosticMetric {
                name,
                cpu_ms: None,
                gpu_ms,
            })
            .collect(),
            voxel_planet_stages: [
                ("planet_residency", 1.0),
                ("planet_horizon", 1.0),
                ("planet_primary", 1.0),
                ("planet_shade", 1.0),
                ("planet_gbuffer", 1.0),
            ]
            .into_iter()
            .map(|(name, gpu_ms)| DiagnosticMetric {
                name,
                cpu_ms: None,
                gpu_ms: Some(gpu_ms),
            })
            .collect(),
            ..Default::default()
        };
        let mut events = Vec::new();
        visit_gpu_events(&data, 9, 100_000_000, 42, |event| events.push(event));
        assert_eq!(events.len(), 8);
        let frame = &events[0];
        assert_eq!(frame.parent_scope_id, None);
        assert_eq!(frame.start_ns, 93_000_000);
        assert_eq!(frame.duration_ns, 7_000_000);
        assert_eq!(events[1].name, "VoxelPlanet");
        assert_eq!(events[1].duration_ns, 5_000_000);
        assert_eq!(events[2].name, "Lighting");
        assert_eq!(events[2].duration_ns, 2_000_000);
        for event in &events {
            assert_eq!(event.thread_id, 0);
            assert_eq!(event.process_id, 42);
            let metadata = event.metadata.as_deref().unwrap();
            assert!(metadata.contains("profiler_id=9;"));
            assert!(metadata.contains("cpu_frame=21;gpu_frame=19;lag_frames=2;"));
            assert!(metadata.contains("readback_drops=3;query_overflows=4;"));
            assert!(metadata.contains("received_ns=100000000;placement=readback_estimate;"));
        }
        for event in &events[1..3] {
            assert_eq!(event.parent_scope_id, Some(frame.scope_id));
            assert!(event.start_ns + event.duration_ns <= frame.start_ns + frame.duration_ns);
        }
        for event in &events[3..] {
            assert_eq!(event.parent_scope_id, Some(events[1].scope_id));
            assert_eq!(event.depth, 2);
            assert!(event.start_ns + event.duration_ns <= events[1].start_ns + events[1].duration_ns);
        }
        // A new graph may reuse the same frame numbers. Its events must stay
        // distinguishable without relying on a decrease in those numbers.
        visit_gpu_events(&data, 10, 101_000_000, 42, |event| events.push(event));
        assert_eq!(events.len(), 16);
        assert!(events[8]
            .metadata
            .as_deref()
            .unwrap()
            .contains("profiler_id=10;"));
        assert_ne!(events[0].scope_id, events[8].scope_id);
        // The real session store must preserve the identity and placement
        // qualification, not just the in-memory bridge.
        let database =
            profiling::database::create_database(std::path::Path::new(":memory:")).unwrap();
        profiling::database::save_events(&database, &events).unwrap();
        let restored = profiling::database::load_events(&database).unwrap();
        assert_eq!(restored.len(), events.len());
        for original in &events {
            let saved = restored
                .iter()
                .find(|event| event.scope_id == original.scope_id)
                .unwrap();
            assert_eq!(saved.metadata, original.metadata);
            assert_eq!(saved.parent_scope_id, original.parent_scope_id);
            assert_eq!(saved.duration_ns, original.duration_ns);
        }
    }

    #[test]
    fn pending_or_invalid_gpu_sample_does_not_fabricate_a_frame() {
        let mut events = Vec::new();
        for data in [
            GpuProfilerData::default(),
            GpuProfilerData {
                total_gpu_ms: Some(1.0),
                ..Default::default()
            },
            GpuProfilerData {
                gpu_frame_count: Some(3),
                total_gpu_ms: Some(f32::NAN),
                ..Default::default()
            },
        ] {
            visit_gpu_events(&data, 9, 100_000_000, 42, |event| events.push(event));
        }
        assert!(events.is_empty());
    }
}
