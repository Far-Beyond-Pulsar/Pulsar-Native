// Corona particle rendering. Kept separate from the compute shader so the
// vertex stage sees read-only storage bindings, as baseline WebGPU requires.

const ATLAS_COLS: u32 = 4u;

struct GpuCoronaUniforms {
    delta_time:      f32,
    total_particles: u32,
    emitter_count:   u32,
    frame_count:     u32,
    sort_k:          u32,
    sort_j:          u32,
    sort_lo:         u32,
    sort_n:          u32,
    origin_shift:    vec4<f32>,
}

struct Particle {
    pos_and_alive:     vec4<f32>,
    velocity:          vec4<f32>,
    color:             vec4<f32>,
    size_lifetime_age: vec4<f32>,
}

struct EmitterDef {
    transform:          mat4x4<f32>,
    emit_params:        vec4<f32>,
    size_params:        vec4<f32>,
    start_color:        vec4<f32>,
    end_color:          vec4<f32>,
    velocity:           vec4<f32>,
    velocity_variation: vec4<f32>,
    extras:             vec4<f32>,
    texture_index:      i32,
    particle_offset:    u32,
    particle_count:     u32,
    spawn_cursor:       u32,
    _pad:               array<f32, 12>,
}

struct CameraUniforms {
    view:           mat4x4<f32>,
    proj:           mat4x4<f32>,
    view_proj:      mat4x4<f32>,
    inv_view_proj:  mat4x4<f32>,
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

@group(0) @binding(0)  var<uniform>       uniforms:         GpuCoronaUniforms;
@group(0) @binding(1)  var<storage, read> particles:        array<Particle>;
@group(0) @binding(2)  var<storage, read> emitters:         array<EmitterDef>;
@group(0) @binding(3)  var<storage, read> compact_buf:      array<u32>;
@group(0) @binding(6)  var<storage, read> cameras: array<CameraUniforms, 2>;
@group(0) @binding(10) var                particle_tex:     texture_2d<f32>;
@group(0) @binding(11) var                particle_sampler: sampler;

struct VOut {
    @builtin(position)              pos:          vec4<f32>,
    @location(0)                    uv:           vec2<f32>,
    @location(1)                    color:        vec4<f32>,
    @location(2) @interpolate(flat) sprite_index: u32,
}

fn quad_corner(idx: u32) -> vec2<f32> {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5), vec2<f32>(0.5, -0.5), vec2<f32>(-0.5, 0.5),
        vec2<f32>(-0.5, 0.5), vec2<f32>(0.5, -0.5), vec2<f32>(0.5, 0.5),
    );
    return corners[idx];
}

fn quad_uv(idx: u32) -> vec2<f32> {
    let uvs = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    return uvs[idx];
}

// `ii` is per-instance vertex data, which every backend offsets by the
// indirect draw's `first_instance` (`helio_pass_gbuffer::draw_slots`).
@vertex
fn vs_main(@builtin(vertex_index) vi: u32, @location(15) ii: u32) -> VOut {
    let pidx = compact_buf[ii];
    let p = particles[pidx];

    let right = vec3<f32>(cameras[0].view[0].x, cameras[0].view[1].x, cameras[0].view[2].x);
    let up = vec3<f32>(cameras[0].view[0].y, cameras[0].view[1].y, cameras[0].view[2].y);
    let size = max(p.size_lifetime_age.x, 0.001);
    let corner = quad_corner(vi);
    let world_pos = p.pos_and_alive.xyz
        + right * corner.x * size
        + up * corner.y * size;

    // The particle's tag: its emitter row + 64 × epoch (corona.wgsl).
    let emitter_idx = min(u32(p.velocity.w + 0.5) % 64u, uniforms.emitter_count - 1u);
    let tex_raw = emitters[emitter_idx].texture_index;
    let sprite = select(u32(tex_raw) % 16u, 0u, tex_raw < 0);

    var out: VOut;
    out.pos = cameras[0].view_proj * vec4<f32>(world_pos, 1.0);
    out.uv = quad_uv(vi);
    out.color = p.color;
    out.sprite_index = sprite;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    let col = in.sprite_index % ATLAS_COLS;
    let row = in.sprite_index / ATLAS_COLS;
    let atlas_uv = vec2<f32>(
        (f32(col) + in.uv.x) * 0.25,
        (f32(row) + in.uv.y) * 0.25,
    );
    let tex = textureSample(particle_tex, particle_sampler, atlas_uv);
    let alpha = tex.a * in.color.a;
    if alpha < 0.005 { discard; }
    return vec4<f32>(in.color.rgb * tex.rgb, alpha);
}
