//! Debug line renderer for visualizing raycasts and other debug geometry.
//!
//! Each line segment is rendered as a thin world-space quad (two triangles),
//! using TriangleList topology and a single position vertex attribute — the
//! same pattern as gizmo_feature.rs which is known to work.

use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use std::sync::Arc;
use std::ptr;

/// Maximum number of debug line segments rendered per frame.
const MAX_DEBUG_LINES: usize = 512;
/// Width of each rendered line in world units (total, not half-width).
const LINE_WIDTH: f32 = 0.08;

// ── GPU types ──────────────────────────────────────────────────────────────

/// Single position vertex — identical layout to GizmoVertex.
#[derive(blade_macros::Vertex, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct DebugLineVertex {
    position: [f32; 3],
}

/// View-projection uniforms (name must match WGSL `var<uniform> camera`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DebugLineCameraUniforms {
    view_proj: [[f32; 4]; 4],
}
#[derive(blade_macros::ShaderData)]
struct DebugLineCameraData {
    camera: DebugLineCameraUniforms,
}

/// Per-line color uniform (name must match WGSL `var<uniform> line_data`).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DebugLineColorUniforms {
    color: [f32; 4],
}
#[derive(blade_macros::ShaderData)]
struct DebugLineColorData {
    line_data: DebugLineColorUniforms,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// A single debug line segment with a frame-count TTL.
pub struct DebugLine {
    pub start: [f32; 3],
    pub end: [f32; 3],
    /// RGBA color in [0, 1].
    pub color: [f32; 4],
    /// Decremented each `render()` call; removed when it reaches zero.
    pub ttl_frames: u32,
}

/// Renders debug line segments as a TriangleList overlay.
///
/// Each line → thin world-space quad (6 vertices / 2 triangles).
/// One draw call per line so each can carry its own colour.
pub struct DebugLineRenderer {
    context: Option<Arc<gpu::Context>>,
    pipeline: Option<gpu::RenderPipeline>,
    /// Pre-allocated vertex buffer: MAX_DEBUG_LINES × 6 vertices.
    vertex_buffer: Option<gpu::Buffer>,
    lines: Vec<DebugLine>,
}

impl DebugLineRenderer {
    pub fn new() -> Self {
        tracing::info!("[DEBUG LINES] Creating debug line renderer");
        Self {
            context: None,
            pipeline: None,
            vertex_buffer: None,
            lines: Vec::new(),
        }
    }

    pub fn init(
        &mut self,
        context: &Arc<gpu::Context>,
        color_format: gpu::TextureFormat,
        _depth_format: gpu::TextureFormat,
    ) {
        tracing::info!("[DEBUG LINES] Initialising GPU resources");
        self.context = Some(Arc::clone(context));

        // Pre-allocate: MAX_DEBUG_LINES × 6 vertices × 12 bytes each.
        let buf_size = (MAX_DEBUG_LINES * 6 * std::mem::size_of::<DebugLineVertex>()) as u64;
        let vbuf = context.create_buffer(gpu::BufferDesc {
            name: "debug_line_vertices",
            size: buf_size,
            memory: gpu::Memory::Shared,
        });
        unsafe { ptr::write_bytes(vbuf.data() as *mut u8, 0, buf_size as usize) };
        context.sync_buffer(vbuf);
        self.vertex_buffer = Some(vbuf);

        let camera_layout = <DebugLineCameraData as gpu::ShaderData>::layout();
        let color_layout  = <DebugLineColorData  as gpu::ShaderData>::layout();

        let shader_source = include_str!("shaders/debug_lines.wgsl");
        let shader = context.create_shader(gpu::ShaderDesc { source: shader_source });

        // No depth stencil at all — we always render on top, depth is irrelevant.
        let pipeline = context.create_render_pipeline(gpu::RenderPipelineDesc {
            name: "debug_lines",
            data_layouts: &[&camera_layout, &color_layout],
            vertex: shader.at("vs_main"),
            vertex_fetches: &[gpu::VertexFetchState {
                layout: &<DebugLineVertex as gpu::Vertex>::layout(),
                instanced: false,
            }],
            primitive: gpu::PrimitiveState {
                topology: gpu::PrimitiveTopology::TriangleList,
                front_face: gpu::FrontFace::Ccw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            fragment: Some(shader.at("fs_main")),
            color_targets: &[gpu::ColorTargetState {
                format: color_format,
                blend: Some(gpu::BlendState::ALPHA_BLENDING),
                write_mask: gpu::ColorWrites::default(),
            }],
            multisample_state: gpu::MultisampleState::default(),
        });

        self.pipeline = Some(pipeline);
        tracing::info!("[DEBUG LINES] ✅ Initialised (capacity: {} lines, width: {})", MAX_DEBUG_LINES, LINE_WIDTH);
    }

    /// Queue a line segment for `ttl_frames` frames.
    /// Hit rays: `[1.0, 0.0, 0.0, 1.0]` (red).
    /// Miss rays: `[1.0, 0.4, 0.0, 1.0]` (orange).
    pub fn add_line(
        &mut self,
        start: [f32; 3],
        end: [f32; 3],
        color: [f32; 4],
        ttl_frames: u32,
    ) {
        if self.lines.len() >= MAX_DEBUG_LINES {
            println!("[DEBUG LINES] ❌ Buffer full — dropping line");
            return;
        }
        println!(
            "[DEBUG LINES] ➕ add_line start=[{:.2},{:.2},{:.2}] end=[{:.2},{:.2},{:.2}] ttl={}",
            start[0], start[1], start[2], end[0], end[1], end[2], ttl_frames
        );
        self.lines.push(DebugLine { start, end, color, ttl_frames });
        println!("[DEBUG LINES] 📦 {} line(s) queued", self.lines.len());
    }

    /// Record the overlay render pass. Call after `gizmo_renderer.render()`.
    pub fn render(
        &mut self,
        encoder: &mut gpu::CommandEncoder,
        target_view: gpu::TextureView,
        view_proj: [[f32; 4]; 4],
    ) {
        // Tick TTLs, prune expired.
        self.lines.retain_mut(|l| {
            if l.ttl_frames == 0 { false } else { l.ttl_frames -= 1; true }
        });

        if self.lines.is_empty() {
            return;
        }

        println!("[DEBUG LINES] render() — {} line(s) active", self.lines.len());

        let pipeline = match &self.pipeline {
            Some(p) => p,
            None => { println!("[DEBUG LINES] ❌ pipeline missing"); return; }
        };
        let vbuf = match self.vertex_buffer {
            Some(b) => b,
            None => { println!("[DEBUG LINES] ❌ vertex_buffer missing"); return; }
        };
        let context = match &self.context {
            Some(c) => Arc::clone(c),
            None => { println!("[DEBUG LINES] ❌ context missing"); return; }
        };

        let max_lines = self.lines.len().min(MAX_DEBUG_LINES);

        // Build all quad vertices upfront.
        let mut all_verts: Vec<DebugLineVertex> = Vec::with_capacity(max_lines * 6);
        for line in self.lines.iter().take(max_lines) {
            let quads = line_to_quad(Vec3::from(line.start), Vec3::from(line.end), LINE_WIDTH * 0.5);
            all_verts.extend_from_slice(&quads);
        }

        println!("[DEBUG LINES] 🔺 uploading {} vertices for {} quads", all_verts.len(), max_lines);

        unsafe {
            ptr::copy_nonoverlapping(
                all_verts.as_ptr(),
                vbuf.data() as *mut DebugLineVertex,
                all_verts.len(),
            );
        }
        context.sync_buffer(vbuf);

        let camera_data = DebugLineCameraData {
            camera: DebugLineCameraUniforms { view_proj },
        };

        let mut pass = encoder.render(
            "debug_lines_overlay",
            gpu::RenderTargetSet {
                colors: &[gpu::RenderTarget {
                    view: target_view,
                    init_op: gpu::InitOp::Load,
                    finish_op: gpu::FinishOp::Store,
                }],
                depth_stencil: None,
            },
        );

        let mut rc = pass.with(pipeline);
        rc.bind(0, &camera_data);
        rc.bind_vertex(0, vbuf.into());

        for (i, line) in self.lines.iter().enumerate().take(max_lines) {
            // No fade — full opacity for debugging visibility.
            let color_data = DebugLineColorData {
                line_data: DebugLineColorUniforms {
                    color: [line.color[0], line.color[1], line.color[2], 1.0],
                },
            };
            rc.bind(1, &color_data);
            let base = i * 6;
            println!(
                "[DEBUG LINES] 🖊  quad {}: verts [{:.1},{:.1},{:.1}]→[{:.1},{:.1},{:.1}]",
                i,
                all_verts[base].position[0],   all_verts[base].position[1],   all_verts[base].position[2],
                all_verts[base+2].position[0], all_verts[base+2].position[1], all_verts[base+2].position[2],
            );
            rc.bind_vertex(0, vbuf.into());
            rc.draw(i as u32 * 6, 6, 0, 1);
        }

        drop(rc);
        drop(pass);
    }

    pub fn cleanup(&mut self) {
        let context = match self.context.take() {
            Some(c) => c,
            None => return,
        };
        if let Some(mut p) = self.pipeline.take() {
            context.destroy_render_pipeline(&mut p);
        }
        if let Some(b) = self.vertex_buffer.take() {
            context.destroy_buffer(b);
        }
        tracing::info!("[DEBUG LINES] Cleaned up");
    }
}

// ── Geometry helper ─────────────────────────────────────────────────────────

/// Build 6 vertices (2 triangles) forming a flat world-space quad along A→B.
/// `half_width` is the perpendicular half-extent in world units.
fn line_to_quad(a: Vec3, b: Vec3, half_width: f32) -> [DebugLineVertex; 6] {
    let dir = (b - a).normalize();

    // Pick an up axis that isn't parallel to the line direction.
    let up = if dir.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
    let right = dir.cross(up).normalize() * half_width;

    let v0 = pos(a - right);
    let v1 = pos(a + right);
    let v2 = pos(b + right);
    let v3 = pos(b - right);

    // Two CCW triangles.
    [v0, v1, v2, v0, v2, v3]
}

#[inline]
fn pos(v: Vec3) -> DebugLineVertex {
    DebugLineVertex { position: v.to_array() }
}
