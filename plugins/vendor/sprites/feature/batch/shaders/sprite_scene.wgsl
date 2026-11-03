enable wgpu_binding_array;

// ── Sprite Batch: scene overlay ─────────────────────────────────────────────
//
// `sprite.wgsl` for authored scene sprites (`SpriteBatchPass::scene_overlay`):
// the same vertex-pulled quads, with each sprite's image sampled from the
// scene's bindless texture table (group 1, the slot the sprite's row carries
// in `atlas_layer`) instead of the pass's own atlas array. `0xffffffff`: no
// image, the tint alone.

struct Camera {
    view_proj: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;

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

@group(1) @binding(0) var scene_textures: binding_array<texture_2d<f32>, 256>;
@group(1) @binding(1) var scene_samplers: binding_array<sampler, 256>;

const NO_TEXTURE: u32 = 0xffffffffu;

struct VertexIn {
    @location(0) quad_pos: vec2<f32>,
    @location(1) quad_uv: vec2<f32>,
}

struct VOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) texture: u32,
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
    out.texture = inst.atlas_layer;
    return out;
}

fn sample_sprite_texture(texture_index: u32, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(scene_textures[texture_index], scene_samplers[texture_index], uv, 0.0);
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    var texel = vec4<f32>(1.0);
    if in.texture != NO_TEXTURE {
        texel = sample_sprite_texture(in.texture, in.uv);
    }
    let c = texel * in.color;
    if c.a < 0.001 {
        discard;
    }
    return c;
}
