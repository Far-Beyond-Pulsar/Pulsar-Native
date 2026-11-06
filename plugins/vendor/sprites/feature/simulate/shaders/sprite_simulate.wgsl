//!use sprite_instance
// ── Sprite Bounce Simulation ────────────────────────────────────────────────
//
// One thread per pool slot. Integrates position by velocity, bounces off a
// fixed world-space box (reflecting velocity, clamping position), and writes
// both straight back into the shared `instances` storage buffer — the same
// buffer `helio-pass-sprite-cull`'s `SpriteCullPass` reads and
// `helio-pass-sprite-batch`'s `SpriteBatchPass` renders from. The CPU never
// sees a position after the sprite is first inserted: this pass, cull, and
// the batch draw are the entire per-frame path, and none of them touch the
// CPU-side `Vec<SpriteInstance>` (which is why `SpriteBatchPass::prepare()`
// must not be given new `update_sprite` calls once a `SpriteSimulatePass` is
// driving a slot — see that crate's module doc comment).
//
// `depth` is kept equal to world Y here (matching the Y-sort convention the
// non-GPU-simulated demos use via `SpriteInstance::with_depth`), so the
// paired cull pass's radix sort still does real, order-changing work every
// frame as sprites cross each other vertically.

struct SimUniforms {
    bounds_min: vec2<f32>,
    bounds_max: vec2<f32>,
    dt: f32,
    slot_count: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> su: SimUniforms;
@group(0) @binding(1) var<storage, read> slot_alive: array<u32>;
@group(0) @binding(2) var<storage, read_write> instances: array<SpriteInstance>;
@group(0) @binding(3) var<storage, read_write> velocities: array<vec2<f32>>;

const WG_SIZE: u32 = 256u;

@compute @workgroup_size(WG_SIZE)
fn cs_simulate(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= su.slot_count {
        return;
    }
    if slot_alive[i] == 0u {
        return;
    }

    var pos = instances[i].position;
    var vel = velocities[i];

    pos += vel * su.dt;

    if pos.x < su.bounds_min.x || pos.x > su.bounds_max.x {
        vel.x = -vel.x;
        pos.x = clamp(pos.x, su.bounds_min.x, su.bounds_max.x);
    }
    if pos.y < su.bounds_min.y || pos.y > su.bounds_max.y {
        vel.y = -vel.y;
        pos.y = clamp(pos.y, su.bounds_min.y, su.bounds_max.y);
    }

    instances[i].position = pos;
    instances[i].depth = pos.y;
    velocities[i] = vel;
}
