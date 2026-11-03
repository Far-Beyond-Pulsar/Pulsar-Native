//! Editor-only portal-surface indicator.
//!
//! In game mode a portal is meant to be perfectly seamless — nothing marks
//! where its surface actually is, only the duplicated content shows (see
//! `gbuffer_portal.wgsl` / `portal_mask.wgsl`). That's exactly right for
//! play, but invisible trigger-plane-sized geometry is miserable to work
//! with in an editor: you can't see it, click it, or tell where its bounds
//! are relative to the room around it.
//!
//! This pass draws a checkerboard over each portal's opening, alpha-blended
//! into `pre_aa` *after* deferred lighting (same slot Corona/Billboard/
//! LensFlare use for their own overlays) — one checker color is fully
//! transparent (`discard`, zero cost, the seamless look is untouched there),
//! the other is a low-alpha darkening tint. The result reads as a faint
//! marker grid over the portal's true footprint without ever fully hiding
//! the duplicated content behind it. Only runs when the pass's `editor_mode`
//! flag is set — zero draws, zero cost, in game builds.

struct Camera {
    view:           mat4x4<f32>,
    proj:           mat4x4<f32>,
    view_proj:      mat4x4<f32>,
    inv_view_proj:  mat4x4<f32>,
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

/// Must match libhelio::GpuPortalView (144 bytes) — see gbuffer_portal.wgsl.
struct GpuPortalView {
    transform:         mat4x4<f32>,
    inverse_transform: mat4x4<f32>,
    half_extent:       vec2<f32>,
    coordinate_space:  u32,
    _pad:              u32,
}

struct GlyphInstance {
    world_position: vec4<f32>,
    color: vec4<f32>,
    bitmap: vec2<u32>,
    screen_offset: vec2<f32>,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> portal_views: array<GpuPortalView>;
@group(0) @binding(2) var<storage, read> debug_glyphs: array<GlyphInstance>;
@group(0) @binding(3) var<storage, read> ghost_vertices: array<vec4<f32>>;

const LOCAL_CORNERS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
);

// Checkerboard cell size, in world units (metres). Tune to taste — this is
// an editor-visualization constant, not something scenes need to configure.
const CELL_SIZE: f32 = 0.5;
// Alpha of the darkened checker cell; the other cell is fully transparent.
const DARKEN_ALPHA: f32 = 0.18;

struct OverlayOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local_xy: vec2<f32>,
    @location(1) @interpolate(flat) portal_flags: u32,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> OverlayOutput {
    let portal = portal_views[instance_index];
    let local = LOCAL_CORNERS[vertex_index] * portal.half_extent;
    let world_pos = portal.transform * vec4<f32>(local, 0.0, 1.0);

    var out: OverlayOutput;
    out.clip_position = cameras[0].view_proj * world_pos;
    out.local_xy = local;
    out.portal_flags = portal._pad;
    return out;
}

@fragment
fn fs_main(input: OverlayOutput) -> @location(0) vec4<f32> {
    if (input.portal_flags & 1u) != 0u {
        discard;
    }
    let cell = vec2<i32>(floor(input.local_xy / CELL_SIZE));
    // Bitwise AND on the low bit — well-defined for negative i32 in WGSL,
    // unlike `%` which can return negative results and break the parity.
    let checker = (cell.x ^ cell.y) & 1;
    if checker == 0 {
        discard;
    }
    return vec4<f32>(0.0, 0.0, 0.0, DARKEN_ALPHA);
}

struct GlyphOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) bitmap: vec2<u32>,
}

const GLYPH_CORNERS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);
// Screen-space text stays legible at any camera distance. These NDC units
// make a glyph about 18 px tall in a typical editor viewport.
const GLYPH_WIDTH: f32 = 0.018;
const GLYPH_HEIGHT: f32 = 0.026;

@vertex
fn vs_glyph_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> GlyphOutput {
    let glyph = debug_glyphs[instance_index];
    let uv = GLYPH_CORNERS[vertex_index];
    let local = vec2<f32>(glyph.screen_offset.x + uv.x * GLYPH_WIDTH,
                          glyph.screen_offset.y - uv.y * GLYPH_HEIGHT);
    let clip = cameras[0].view_proj * vec4<f32>(glyph.world_position.xyz, 1.0);
    var out: GlyphOutput;
    out.clip_position = clip + vec4<f32>(local * clip.w, 0.0, 0.0);
    out.uv = uv;
    out.color = glyph.color;
    out.bitmap = glyph.bitmap;
    return out;
}

@fragment
fn fs_glyph_main(input: GlyphOutput) -> @location(0) vec4<f32> {
    let pixel = vec2<u32>(min(vec2<f32>(4.0, 6.0), floor(input.uv * vec2<f32>(5.0, 7.0))));
    let bit = pixel.y * 5u + pixel.x;
    var lit_bit = false;
    if bit < 32u {
        lit_bit = ((input.bitmap.x >> bit) & 1u) != 0u;
    } else {
        lit_bit = ((input.bitmap.y >> (bit - 32u)) & 1u) != 0u;
    }
    if lit_bit {
        return input.color;
    }
    // A dark backing preserves contrast over both bright portal contents and
    // the checker overlay, so the link number remains readable.
    return vec4<f32>(0.015, 0.02, 0.035, 0.82);
}

@vertex
fn vs_ghost_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    return cameras[0].view_proj * vec4<f32>(ghost_vertices[vertex_index].xyz, 1.0);
}

@fragment
fn fs_ghost_main() -> @location(0) vec4<f32> {
    return vec4<f32>(0.10, 0.86, 1.0, 0.28);
}
