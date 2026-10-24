struct Owner {
    owner_index: u32,
    owner_generation: u32,
    enabled: u32,
}

struct Transform {
    position_hi: array<f32, 3>,
    position_lo: array<f32, 3>,
    rotation: array<f32, 3>,
    scale: array<f32, 3>,
}

struct Handle {
    offset: u32,
    count: u32,
}

struct Level {
    distance_threshold: f32,
    lod_bias: f32,
    mesh_asset: array<u32, 4>,
}

struct Camera {
    view: array<vec4<f32>, 4>,
    proj: array<vec4<f32>, 4>,
    view_proj: array<vec4<f32>, 4>,
    inv_view_proj: array<vec4<f32>, 4>,
    position_near: vec4<f32>,
    forward_far: vec4<f32>,
    jitter_frame: vec4<f32>,
    prev_view_proj: array<vec4<f32>, 4>,
}

struct Selection {
    level_index: u32,
    valid: u32,
    mesh_asset: array<u32, 4>,
    camera_distance: f32,
}

struct Uniforms {
    component_rows: u32,
    use_origin_relative: u32,
    _pad: vec2<u32>,
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
}

const F32_MAX: f32 = 3.4028235e38;

fn is_finite(value: f32) -> bool {
    return value == value && abs(value) <= F32_MAX;
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> owners: array<Owner>;
@group(0) @binding(2) var<storage, read> generations: array<u32>;
@group(0) @binding(3) var<storage, read> hidden: array<u32>;
@group(0) @binding(4) var<storage, read> transforms: array<Transform>;
@group(0) @binding(5) var<storage, read> level_handles: array<Handle>;
@group(0) @binding(6) var<storage, read> levels: array<Level>;
@group(0) @binding(7) var<storage, read> cameras: array<Camera>;
@group(0) @binding(8) var<storage, read_write> selections: array<Selection>;
// Generic per-object-entity flags ("lod_suppressed"): non-zero means another
// system draws this object's detail, so no authored level is selected.
@group(0) @binding(9) var<storage, read> lod_suppressed: array<u32>;

@compute @workgroup_size(64)
fn cs_select_authored_lod(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    if row >= u.component_rows {
        return;
    }

    if row >= arrayLength(&owners) || row >= arrayLength(&level_handles)
        || arrayLength(&cameras) == 0u {
        return;
    }
    let owner = owners[row];
    let handle = level_handles[row];
    if owner.enabled == 0u {
        return;
    }
    if owner.owner_index >= arrayLength(&generations) {
        return;
    }
    if generations[owner.owner_index] != owner.owner_generation {
        return;
    }
    if owner.owner_index >= arrayLength(&transforms) {
        return;
    }
    if owner.owner_index < arrayLength(&hidden) && hidden[owner.owner_index] != 0u {
        return;
    }
    if owner.owner_index < arrayLength(&lod_suppressed) && lod_suppressed[owner.owner_index] != 0u {
        return;
    }

    let transform = transforms[owner.owner_index];
    let object_position = vec3<f32>(
        transform.position_hi[0] + transform.position_lo[0],
        transform.position_hi[1] + transform.position_lo[1],
        transform.position_hi[2] + transform.position_lo[2],
    );
    var camera_position = cameras[0].position_near.xyz;
    if u.use_origin_relative != 0u {
        camera_position = camera_position - u.origin_hi.xyz - u.origin_lo.xyz;
    }
    let distance = length(object_position - camera_position);
    if handle.count == 0u || handle.offset > arrayLength(&levels)
        || handle.count > arrayLength(&levels) - handle.offset {
        return;
    }

    var selected_index = 0xffffffffu;
    var selected_asset = array<u32, 4>(0u, 0u, 0u, 0u);
    var previous_threshold = 0.0;
    for (var i = 0u; i < handle.count; i += 1u) {
        let level = levels[handle.offset + i];
        if !is_finite(level.distance_threshold) || !is_finite(level.lod_bias) {
            continue;
        }
        previous_threshold = max(previous_threshold, max(level.distance_threshold, 0.0));
        let bias = clamp(level.lod_bias, -16.0, 16.0);
        let threshold = previous_threshold * exp2(-bias);
        let has_asset = (level.mesh_asset[0] | level.mesh_asset[1]
            | level.mesh_asset[2] | level.mesh_asset[3]) != 0u;
        if distance >= threshold && has_asset {
            selected_index = i;
            selected_asset = level.mesh_asset;
        }
    }
    if selected_index != 0xffffffffu
        && (selected_asset[0] | selected_asset[1] | selected_asset[2] | selected_asset[3]) != 0u {
        if owner.owner_index < arrayLength(&selections) {
            selections[owner.owner_index] = Selection(selected_index, 1u, selected_asset, distance);
        }
    }
}
