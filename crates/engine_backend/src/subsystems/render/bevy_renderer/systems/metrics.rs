//! Performance metrics and GPU profiling systems

use bevy::prelude::*;
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use crate::subsystems::render::bevy_renderer::core::{
    MetricsResource, GpuProfilerResource, DiagnosticMetric,
};
use std::sync::atomic::Ordering;

/// Update performance metrics system
/// Tracks FPS, frame time, and frame count
pub fn update_metrics_system(
    _time: Res<Time>,
    mut metrics: ResMut<MetricsResource>,
    meshes: Res<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    cameras: Query<&Camera>,
) {
    profiling::profile_scope!("Bevy::UpdateMetrics");
    // Increment frame count
    let _frame_count = metrics.frames_rendered.fetch_add(1, Ordering::Relaxed);

    // Calculate FPS and frame time every frame
    if let Ok(mut last_frame_time) = metrics.last_frame_time.lock() {
        let now = std::time::Instant::now();
        let frame_duration = now.duration_since(*last_frame_time);
        let frame_time_ms = frame_duration.as_secs_f32() * 1000.0;

        // Update frame time
        if let Ok(mut stored_frame_time) = metrics.frame_time_ms.lock() {
            *stored_frame_time = frame_time_ms;
        }

        // Calculate FPS (1 / frame_time in seconds)
        let fps = if frame_time_ms > 0.0 {
            1000.0 / frame_time_ms
        } else {
            0.0
        };

        if let Ok(mut stored_fps) = metrics.fps.lock() {
            // Smooth FPS with exponential moving average
            *stored_fps = (*stored_fps * 0.9) + (fps * 0.1);
        }

        *last_frame_time = now;
    }

    // Calculate draw calls estimate based on active cameras and scene complexity
    // Each camera typically has multiple render passes
    let num_cameras = cameras.iter().count();
    let num_meshes = meshes.len();
    let num_materials = materials.len();
    
    // Estimate: cameras * (base passes + mesh instances + material batches)
    // Base passes: shadow + main + post-processing ~= 3
    let estimated_draw_calls = (num_cameras * 3) + num_meshes.min(100) + num_materials.min(50);
    metrics.draw_calls.store(estimated_draw_calls as u32, Ordering::Relaxed);

    // Calculate vertices estimate based on meshes
    let mut total_vertices = 0u64;
    for mesh in meshes.iter() {
        // count_vertices() returns usize, not Option<usize>
        let vertex_count = mesh.1.count_vertices();
        total_vertices += vertex_count as u64;
    }
    metrics.vertices_drawn.store(total_vertices, Ordering::Relaxed);

    // Estimate memory usage (very rough)
    // Textures + meshes + materials + buffers
    let mesh_memory_mb = (total_vertices * 32) as f32 / (1024.0 * 1024.0); // ~32 bytes per vertex
    let material_memory_mb = (num_materials * 1024) as f32 / (1024.0 * 1024.0); // ~1KB per material
    let total_memory_mb = mesh_memory_mb + material_memory_mb + 64.0; // +64MB base overhead
    
    if let Ok(mut memory) = metrics.memory_usage_mb.lock() {
        *memory = total_memory_mb;
    }
}

/// Update GPU Profiler system - extracts REAL GPU timing from Bevy's RenderDiagnosticsPlugin
/// Like Unreal's "stat gpu" - uses actual measured timings from GPU render passes
pub fn update_gpu_profiler_system(
    diagnostics: Res<DiagnosticsStore>,
    profiler: Res<GpuProfilerResource>,
) {
    profiling::profile_scope!("Bevy::UpdateGPUProfiler");
    use bevy::diagnostic::DiagnosticPath;
    
    // Helper to get GPU timing from diagnostic path
    let get_gpu_timing = |path_str: &str| -> f32 {
        let path = DiagnosticPath::from_components(path_str.split('/'));
        diagnostics
            .get(&path)
            .and_then(|d| d.smoothed())
            .unwrap_or(0.0) as f32
    };
    
    // Get frame time as baseline
    let frame_time_ms = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
        .and_then(|d| d.smoothed())
        .unwrap_or(16.67) as f32;
    
    // Extract REAL GPU timings from Bevy's RenderDiagnosticsPlugin
    // These are measured using GPU timestamp queries, not estimates
    // Paths follow format: "render/{pass_name}/elapsed_gpu" (in milliseconds)
    
    // Core render passes
    let shadow_ms = get_gpu_timing("render/shadow_pass/elapsed_gpu");
    let opaque_ms = get_gpu_timing("render/main_opaque_pass/elapsed_gpu") 
        + get_gpu_timing("render/opaque_pass/elapsed_gpu");
    let alpha_mask_ms = get_gpu_timing("render/alpha_mask_pass/elapsed_gpu");
    let transparent_ms = get_gpu_timing("render/main_transparent_pass/elapsed_gpu")
        + get_gpu_timing("render/transparent_pass/elapsed_gpu");
    
    // Lighting and post-processing
    let lighting_ms = get_gpu_timing("render/lighting/elapsed_gpu")
        + get_gpu_timing("render/pbr_lighting/elapsed_gpu");
    let post_ms = get_gpu_timing("render/post_processing/elapsed_gpu")
        + get_gpu_timing("render/tonemapping/elapsed_gpu")
        + get_gpu_timing("render/fxaa/elapsed_gpu")
        + get_gpu_timing("render/bloom/elapsed_gpu");
    
    // UI rendering
    let ui_ms = get_gpu_timing("render/ui_pass/elapsed_gpu");
    
    // Calculate total GPU time from all measured passes
    let total_gpu_ms = shadow_ms + opaque_ms + alpha_mask_ms + transparent_ms 
        + lighting_ms + post_ms + ui_ms;
    
    // Use total_gpu_ms if available, otherwise fall back to frame_time_ms
    let baseline_ms = if total_gpu_ms > 0.1 { total_gpu_ms } else { frame_time_ms };
    
    let calc_pct = |ms: f32| {
        if baseline_ms > 0.0 {
            (ms / baseline_ms * 100.0).max(0.0).min(100.0)
        } else {
            0.0
        }
    };
    
    // Collect dynamic diagnostic metrics
    let mut render_metrics = Vec::new();
    
    // Helper function to create human-readable names from diagnostic paths
    let path_to_name = |path: &str| -> String {
        if path == "frame_time" {
            return "Frame Time".to_string();
        }
        if path == "fps" {
            return "FPS".to_string();
        }
        if path == "frame_count" {
            return "Frame Count".to_string();
        }
        
        // Extract the render pass name from paths like "render/early_mesh_preprocessing/elapsed_gpu"
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() >= 2 && parts[0] == "render" {
            let pass_name = parts[1];
            let timing_type = parts.get(2).unwrap_or(&"");
            
            // Convert snake_case to Title Case
            let title_case = pass_name
                .split('_')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + &chars.collect::<String>(),
                    }
                })
                .collect::<Vec<String>>()
                .join(" ");
                
            // Add timing type suffix
            match *timing_type {
                "elapsed_gpu" => format!("{} (GPU)", title_case),
                "elapsed_cpu" => format!("{} (CPU)", title_case),
                _ => title_case,
            }
        } else {
            // Fallback: just capitalize the path
            path.replace('_', " ").split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + &chars.collect::<String>(),
                    }
                })
                .collect::<Vec<String>>()
                .join(" ")
        }
    };
    
    // Get FPS and frame count
    let fps = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FPS)
        .and_then(|d| d.smoothed())
        .unwrap_or(0.0) as f32;
        
    let frame_count = diagnostics
        .get(&FrameTimeDiagnosticsPlugin::FRAME_COUNT)
        .and_then(|d| d.value())
        .unwrap_or(0.0) as u64;
    
    // Iterate through all available diagnostics and collect render-related ones
    for diagnostic in diagnostics.iter() {
        let path_str = diagnostic.path().to_string();
        
        // Only include render passes, fps, and frame_time
        if path_str.starts_with("render/") || path_str == "fps" || path_str == "frame_time" || path_str == "frame_count" {
            if let Some(value) = diagnostic.smoothed() {
                let value_ms = value as f32;
                let is_gpu = path_str.contains("elapsed_gpu");
                let percentage = calc_pct(value_ms);
                
                render_metrics.push(DiagnosticMetric {
                    name: path_to_name(&path_str),
                    path: path_str,
                    value_ms,
                    percentage,
                    is_gpu,
                });
            }
        }
    }
    
    // Sort metrics by GPU/CPU type, then by value (descending)
    render_metrics.sort_by(|a, b| {
        // First sort by type (GPU first, then CPU)
        match (a.is_gpu, b.is_gpu) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => {
                // Then by value (descending)
                b.value_ms.partial_cmp(&a.value_ms).unwrap_or(std::cmp::Ordering::Equal)
            }
        }
    });

    // Update profiler data with REAL GPU timings from hardware queries
    if let Ok(mut data) = profiler.data.lock() {
        data.total_frame_ms = frame_time_ms;
        data.fps = fps;
        data.frame_count = frame_count;
        data.render_metrics = render_metrics;
        data.shadow_pass_ms = shadow_ms;
        data.shadow_pass_pct = calc_pct(shadow_ms);
        data.opaque_pass_ms = opaque_ms;
        data.opaque_pass_pct = calc_pct(opaque_ms);
        data.alpha_mask_pass_ms = alpha_mask_ms;
        data.alpha_mask_pass_pct = calc_pct(alpha_mask_ms);
        data.transparent_pass_ms = transparent_ms;
        data.transparent_pass_pct = calc_pct(transparent_ms);
        data.lighting_ms = lighting_ms;
        data.lighting_pct = calc_pct(lighting_ms);
        data.post_processing_ms = post_ms;
        data.post_processing_pct = calc_pct(post_ms);
        data.ui_pass_ms = ui_ms;
        data.ui_pass_pct = calc_pct(ui_ms);
        data.total_gpu_ms = total_gpu_ms;
    }
}
