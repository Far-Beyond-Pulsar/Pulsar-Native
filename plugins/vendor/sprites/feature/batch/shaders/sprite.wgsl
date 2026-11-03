// ── Sprite Batch ─────────────────────────────────────────────────────────────
//
// Vertex-pulling, not vertex-buffer instancing: the unit quad (binding 0,
// per-vertex) is drawn `visible_count` times, and each invocation looks up
// its own sprite via `draw_order[instance_index]` into the persistent
// `instances` storage array. This decouples the two things that change on
// very different schedules:
//
//   - `instances` is a stable, handle-addressed pool the CPU side only
//     touches (and re-uploads) the slots that actually changed this frame
//     (see `SpriteBatchPass::update_sprite`) — most frames in a real scene
//     touch a small fraction of it.
//   - `draw_order` is a small per-frame index list (culled, then radix-sorted
//     back-to-front by depth for correct alpha blending) that's rebuilt
//     whenever visibility-relevant state changes, independent of whether the
//     underlying sprite data changed.
//
// A `VertexStepMode::Instance` buffer couldn't do this: it always reads
// instance N's data from slot N, so reordering draw order would require
// physically reordering the data buffer — which is exactly the per-frame
// full-rewrite this design avoids.

struct Camera {
    view_proj: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var atlas_tex: texture_2d_array<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct SpriteInstance {
    position: vec2<f32>,
    size: vec2<f32>,
    rotation: f32,
    depth: f32,
    _pad_uv: vec2<f32>,
    uv_rect: vec4<f32>,
    color: vec4<f32>,
    atlas_layer: u32,
}
@group(0) @binding(3) var<storage, read> instances: array<SpriteInstance>;
@group(0) @binding(4) var<storage, read> draw_order: array<u32>;

struct VertexIn {
    @location(0) quad_pos: vec2<f32>,
    @location(1) quad_uv: vec2<f32>,
}

struct VOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) atlas_layer: u32,
}

@vertex
fn vs_main(v: VertexIn, @builtin(instance_index) instance_index: u32) -> VOut {
    let inst = instances[draw_order[instance_index]];

    let c = cos(inst.rotation);
    let s = sin(inst.rotation);
    let local = v.quad_pos * inst.size;
    let rotated = vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);
    let world = rotated + inst.position;

    var out: VOut;
    out.clip_pos = camera.view_proj * vec4<f32>(world, 0.0, 1.0);
    out.uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, v.quad_uv);
    out.color = inst.color;
    out.atlas_layer = inst.atlas_layer;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let sampled = textureSample(atlas_tex, atlas_samp, in.uv, i32(in.atlas_layer));
    let c = sampled * in.color;
    if c.a < 0.001 {
        discard;
    }
    return c;
}
