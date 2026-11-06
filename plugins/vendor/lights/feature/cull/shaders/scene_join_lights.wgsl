// Light join: one thread per light-instance row. A placed, enabled light is
// copied to the same row of the output with its position at the owner's
// position and its direction the owner's rotation of -Y; every other row
// stays zero (zero color and intensity: no light). Editor billboards are
// written alongside when enabled.

/// `helio::GpuLight` (128 bytes): three vec4s, then 20 scalar words.
/// Word 3 of `rest` is `_pad` (the shadow intent and policy bits). In the
/// input (`helio_component::LightSourceRow`) its bit 31, which `_pad` leaves
/// unused, is the authored `enabled` flag; the output clears it.
struct Light {
    position_range: array<f32, 4>,
    direction_outer: array<f32, 4>,
    color_intensity: array<f32, 4>,
    rest: array<u32, 20>,
}

/// `helio_pass_billboard::BillboardComponent` (48 bytes).
struct Billboard {
    world_pos: array<f32, 4>,
    scale_flags: array<f32, 4>,
    color: array<f32, 4>,
}

struct LightJoinUniforms {
    light_rows: u32,
    billboards: u32,
    _pad0: u32,
    _pad1: u32,
    // The frame's world origin (hi + lo), zero outside camera-relative frames.
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: LightJoinUniforms;
@group(0) @binding(1) var<storage, read> owners: array<Owner>;
@group(0) @binding(2) var<storage, read> generations: array<u32>;
@group(0) @binding(3) var<storage, read> hidden: array<u32>;
@group(0) @binding(4) var<storage, read> transforms: array<ObjectTransform>;
@group(0) @binding(5) var<storage, read> sources: array<Light>;
@group(0) @binding(6) var<storage, read_write> lights_out: array<Light>;
@group(0) @binding(7) var<storage, read_write> billboards_out: array<Billboard>;

const ENABLED_WORD: u32 = 3u;
const ENABLED_BIT: u32 = 0x80000000u;

@compute @workgroup_size(64)
fn cs_join_lights(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    if row >= u.light_rows || row >= arrayLength(&sources) || row >= arrayLength(&owners) {
        return;
    }
    var light = sources[row];
    if (light.rest[ENABLED_WORD] & ENABLED_BIT) == 0u {
        return;
    }
    let owner = owners[row];
    if owner.enabled == 0u {
        return;
    }
    let index = owner.owner_index;
    if index >= arrayLength(&generations) || generations[index] != owner.owner_generation {
        return;
    }
    if index < arrayLength(&hidden) && hidden[index] != 0u {
        return;
    }
    if index >= arrayLength(&transforms) {
        return;
    }
    let t = transforms[index];
    let relative = object_relative(t, u.origin_hi, u.origin_lo);
    let direction = object_rotation(t) * vec3<f32>(0.0, -1.0, 0.0);
    light.position_range[0] = relative.x;
    light.position_range[1] = relative.y;
    light.position_range[2] = relative.z;
    light.direction_outer[0] = direction.x;
    light.direction_outer[1] = direction.y;
    light.direction_outer[2] = direction.z;
    light.rest[ENABLED_WORD] = light.rest[ENABLED_WORD] & ~ENABLED_BIT;
    lights_out[row] = light;
    if u.billboards != 0u && row < arrayLength(&billboards_out) {
        billboards_out[row] = Billboard(
            array<f32, 4>(relative.x, relative.y, relative.z, 0.0),
            array<f32, 4>(0.04, 0.04, 1.0, 0.0),
            array<f32, 4>(light.color_intensity[0], light.color_intensity[1], light.color_intensity[2], 1.0),
        );
    }
}
