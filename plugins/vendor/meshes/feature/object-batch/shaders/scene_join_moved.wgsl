// Moved-owner mesh join: rewrites only the placement of the object rows whose
// owner's transform row was flushed this frame, so the cost follows what moved
// and not how many objects exist. Everything that is not a placement (mesh,
// material, flags, visibility) is a structural input and unchanged on this
// path, so those rows and the materials stay as the last full join left them.
//
// Three entry points run in order:
// - `cs_join_moved`: one thread per flushed owner row; walks the owner's
//   placed instances (the lists the full join linked), rewrites their object
//   rows' transform, previous transform, normal matrix and bounds, stamps the
//   slots and appends them to this frame's moved list.
// - `cs_moved_finish`: one thread; records how many slots are this frame's own
//   and sizes the carry dispatch.
// - `cs_moved_carry`: one thread per slot of the previous frame's own list not
//   moved again this frame; appended so the object batch also settles their
//   previous-frame transform.
//
// The moved list is `[total, fresh, slot, slot, ..]`.

struct Handle {
    offset: u32,
    count: u32,
}

/// `helio_pass_gbuffer::StaticObjectComponent` (252 bytes).
struct StaticObjectRow {
    mesh_slot: u32,
    mesh_generation: u32,
    material_slot: u32,
    material_generation: u32,
    transform: array<array<f32, 4>, 4>,
    prev_transform: array<array<f32, 4>, 4>,
    normal_mat: array<array<f32, 4>, 3>,
    bounds: array<f32, 4>,
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    flags: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

struct MovedUniforms {
    output_rows: u32,
    link_base: u32,
    run: u32,
    _pad: u32,
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: MovedUniforms;
@group(0) @binding(1) var<storage, read> transforms: array<ObjectTransform>;
@group(0) @binding(2) var<storage, read> section_handles: array<Handle>;
@group(0) @binding(3) var<storage, read> mesh_bounds: array<array<f32, 4>>;
@group(0) @binding(4) var<storage, read_write> objects_out: array<StaticObjectRow>;
// `[count, owner rows..]`, SceneDB's rows of the last transform flush.
@group(0) @binding(5) var<storage, read> flushed: array<u32>;
@group(0) @binding(6) var<storage, read_write> link: array<atomic<u32>>;
@group(0) @binding(7) var<storage, read_write> stamp: array<u32>;
@group(0) @binding(8) var<storage, read_write> moved_new: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read> moved_prev: array<u32>;
@group(0) @binding(10) var<storage, read_write> carry_args: array<u32>;

fn safe_inverse(v: f32) -> f32 {
    if v == 0.0 {
        return 0.0;
    }
    return 1.0 / v;
}

fn push_slot(slot: u32) {
    let at = atomicAdd(&moved_new[0], 1u);
    if 2u + at < arrayLength(&moved_new) {
        atomicStore(&moved_new[2u + at], slot);
    }
}

@compute @workgroup_size(64)
fn cs_join_moved(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= flushed[0] {
        return;
    }
    let owner = flushed[1u + gid.x];
    if owner >= u.link_base || owner >= arrayLength(&transforms) {
        return;
    }
    let t = transforms[owner];
    let rotation = object_rotation(t);
    let scale = object_scale(t);
    let position = object_relative(t, u.origin_hi, u.origin_lo);
    let c0 = rotation[0] * scale.x;
    let c1 = rotation[1] * scale.y;
    let c2 = rotation[2] * scale.z;
    let n0 = rotation[0] * safe_inverse(scale.x);
    let n1 = rotation[1] * safe_inverse(scale.y);
    let n2 = rotation[2] * safe_inverse(scale.z);
    let model = array<array<f32, 4>, 4>(
        array<f32, 4>(c0.x, c0.y, c0.z, 0.0),
        array<f32, 4>(c1.x, c1.y, c1.z, 0.0),
        array<f32, 4>(c2.x, c2.y, c2.z, 0.0),
        array<f32, 4>(position.x, position.y, position.z, 1.0),
    );
    let normal_mat = array<array<f32, 4>, 3>(
        array<f32, 4>(n0.x, n0.y, n0.z, 0.0),
        array<f32, 4>(n1.x, n1.y, n1.z, 0.0),
        array<f32, 4>(n2.x, n2.y, n2.z, 0.0),
    );
    let abs_scale = abs(scale);
    let largest = max(abs_scale.x, max(abs_scale.y, abs_scale.z));

    var next = atomicLoad(&link[owner]);
    var guard = 0u;
    while next != 0u && guard < 4096u {
        guard = guard + 1u;
        let row = next - 1u;
        next = atomicLoad(&link[u.link_base + row]);
        if row >= arrayLength(&section_handles) {
            continue;
        }
        let draws = section_handles[row];
        var local = array<f32, 4>(0.0, 0.0, 0.0, 0.5);
        if row < arrayLength(&mesh_bounds) {
            local = mesh_bounds[row];
        }
        let center = rotation * (vec3<f32>(local[0], local[1], local[2]) * scale) + position;
        let radius = max(local[3] * largest, 0.0);
        for (var s = 0u; s < draws.count; s = s + 1u) {
            let slot = draws.offset + s;
            if slot >= arrayLength(&objects_out) || slot >= u.output_rows {
                break;
            }
            objects_out[slot].transform = model;
            objects_out[slot].prev_transform = model;
            objects_out[slot].normal_mat = normal_mat;
            objects_out[slot].bounds = array<f32, 4>(center.x, center.y, center.z, radius);
            stamp[slot] = u.run;
            push_slot(slot);
        }
    }
}

@compute @workgroup_size(1)
fn cs_moved_finish() {
    atomicStore(&moved_new[1], atomicLoad(&moved_new[0]));
    carry_args[0] = (moved_prev[1] + 63u) / 64u;
    carry_args[1] = 1u;
    carry_args[2] = 1u;
}

@compute @workgroup_size(64)
fn cs_moved_carry(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= moved_prev[1] {
        return;
    }
    let slot = moved_prev[2u + gid.x];
    if slot >= arrayLength(&stamp) || stamp[slot] == u.run {
        return;
    }
    push_slot(slot);
}
