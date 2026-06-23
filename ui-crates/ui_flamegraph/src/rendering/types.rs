use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct FlamegraphUniforms {
    pub viewport_x: f32,
    pub viewport_y: f32,
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub thread_label_width: f32,
    pub y_adj: f32,
    pub row_h: f32,
}

/// Compact span data — sent to GPU storage buffer once per LOD level change.
/// Vertex shader reads this via vertex-pulling, computes screen position,
/// culls off-screen, applies pan/zoom. CPU per-frame work = O(1).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GpuSpan {
    pub start_rel_ns: f32,
    pub end_rel_ns: f32,
    pub y: f32,
    pub color_index: u32,
    pub span_count: u32,
    pub depth: u32,
    pub _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct RectInstance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub kind: u32,
    pub _pad: [u32; 3],
}
