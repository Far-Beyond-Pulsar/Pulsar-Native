// Mesh join: one thread per mesh-instance row. A placed instance (attached,
// enabled, owner alive and not hidden) writes one object row and one
// material row per section, at the section's slot in the sections pool;
// every other slot stays zero (the object-batch pass skips rows whose
// `mesh_generation` is 0).

struct Handle {
    offset: u32,
    count: u32,
}

/// `helio_mats::GpuMaterial` (96 bytes).
struct Material {
    base_color: array<f32, 4>,
    emissive: array<f32, 4>,
    roughness_metallic: array<f32, 4>,
    tex_base_color: u32,
    tex_normal: u32,
    tex_roughness: u32,
    tex_emissive: u32,
    tex_occlusion: u32,
    workflow: u32,
    flags: u32,
    material_class: u32,
    class_params: array<f32, 4>,
}

/// `helio_component::MeshSectionDraw` (128 bytes).
struct Section {
    first_index: u32,
    index_count: u32,
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    material: Material,
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

struct MeshJoinUniforms {
    instance_rows: u32,
    output_rows: u32,
    // Where the per-instance list links start in `link` (after the per-owner heads).
    link_base: u32,
    run: u32,
    // The frame's world origin (hi + lo), zero outside camera-relative frames.
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
}

@group(0) @binding(0) var<uniform> u: MeshJoinUniforms;
@group(0) @binding(1) var<storage, read> owners: array<Owner>;
@group(0) @binding(2) var<storage, read> generations: array<u32>;
@group(0) @binding(3) var<storage, read> hidden: array<u32>;
@group(0) @binding(4) var<storage, read> transforms: array<ObjectTransform>;
@group(0) @binding(5) var<storage, read> vertex_handles: array<Handle>;
@group(0) @binding(6) var<storage, read> index_handles: array<Handle>;
@group(0) @binding(7) var<storage, read> mesh_bounds: array<array<f32, 4>>;
@group(0) @binding(8) var<storage, read> mesh_flags: array<u32>;
@group(0) @binding(9) var<storage, read> section_handles: array<Handle>;
@group(0) @binding(10) var<storage, read> skin_handles: array<Handle>;
@group(0) @binding(11) var<storage, read> palette_handles: array<Handle>;
@group(0) @binding(12) var<storage, read> sections: array<Section>;
@group(0) @binding(13) var<storage, read_write> objects_out: array<StaticObjectRow>;
@group(0) @binding(14) var<storage, read_write> materials_out: array<Material>;
// Per-owner heads then per-instance links of a linked list of the placed
// instances each owner carries (`head + 1`, 0 ends the list), rebuilt by every
// full join so a later moved-owner join can find an owner's instances.
@group(0) @binding(15) var<storage, read_write> link: array<atomic<u32>>;
// Per object: the space (coordinate-space slot of its nearest rigid frame) it is
// drawn in, zero for the world. Absent (a zero-length buffer) outside a hierarchy.
@group(0) @binding(16) var<storage, read> spaces: array<u32>;

/// The owner entity of instance `row` when the instance should draw, else
/// `NO_OWNER`: attached and enabled, its recorded owner generation matches
/// the owner's current one, the owner is not hidden and has a transform.
fn placed_owner(row: u32) -> u32 {
    if row >= arrayLength(&owners) {
        return NO_OWNER;
    }
    let owner = owners[row];
    if owner.enabled == 0u {
        return NO_OWNER;
    }
    let index = owner.owner_index;
    if index >= arrayLength(&generations) || generations[index] != owner.owner_generation {
        return NO_OWNER;
    }
    if index < arrayLength(&hidden) && hidden[index] != 0u {
        return NO_OWNER;
    }
    if index >= arrayLength(&transforms) {
        return NO_OWNER;
    }
    return index;
}

fn safe_inverse(v: f32) -> f32 {
    if v == 0.0 {
        return 0.0;
    }
    return 1.0 / v;
}

@compute @workgroup_size(64)
fn cs_join_meshes(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    if row >= u.instance_rows {
        return;
    }
    let owner = placed_owner(row);
    if owner == NO_OWNER {
        return;
    }
    if row >= arrayLength(&section_handles) || row >= arrayLength(&vertex_handles)
        || row >= arrayLength(&index_handles) {
        return;
    }
    let draws = section_handles[row];
    let vertices = vertex_handles[row];
    let indices = index_handles[row];
    var skin = Handle(0u, 0u);
    var palette = Handle(0u, 0u);
    if row < arrayLength(&skin_handles) { skin = skin_handles[row]; }
    if row < arrayLength(&palette_handles) { palette = palette_handles[row]; }
    if draws.count == 0u || vertices.count == 0u || indices.count == 0u {
        return;
    }
    if owner < u.link_base && u.link_base + row < arrayLength(&link) {
        let previous = atomicExchange(&link[owner], row + 1u);
        atomicStore(&link[u.link_base + row], previous);
    }
    // The instance's own generation, offset by one so a live row's
    // identity is never zero.
    var generation = 1u;
    if row < arrayLength(&generations) {
        generation = generations[row] + 1u;
    }

    let t = transforms[owner];
    let rotation = object_rotation(t);
    let scale = object_scale(t);
    // Relative to the frame's world origin, in two steps: an object near the
    // camera is near the origin, so `object - hi` is exact, and what remains
    // is small. At Earth radius one f32 step is 0.5 m; a world-space model
    // matrix would snap every vertex to it.
    let position = object_relative(t, u.origin_hi, u.origin_lo);
    let c0 = rotation[0] * scale.x;
    let c1 = rotation[1] * scale.y;
    let c2 = rotation[2] * scale.z;
    // Inverse-transpose of rotation * scale: rotation * scale^-1.
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

    // World bounding sphere: the local center through the model matrix, the
    // local radius scaled by the largest axis scale (conservative under
    // non-uniform scale).
    var local = array<f32, 4>(0.0, 0.0, 0.0, 0.5);
    if row < arrayLength(&mesh_bounds) {
        local = mesh_bounds[row];
    }
    let center = rotation * (vec3<f32>(local[0], local[1], local[2]) * scale) + position;
    let abs_scale = abs(scale);
    let radius = max(local[3] * max(abs_scale.x, max(abs_scale.y, abs_scale.z)), 0.0);
    var flags = 0u;
    if row < arrayLength(&mesh_flags) {
        flags = mesh_flags[row];
    }
    // The space this object is drawn in (instance flags bits 8-15, see
    // `helio_pass_object_batch::set_coordinate_space`): its nearest rigid frame.
    if owner < arrayLength(&spaces) {
        flags = (flags & ~0xFF00u) | ((spaces[owner] & 0xFFu) << 8u);
        // Static means "never moves": inside a rigid frame an object is fixed only
        // relative to the frame, and the frame moves, so caches that key on a
        // fixed world transform (static shadow casters, ray-tracing instances)
        // must treat it as movable.
        if spaces[owner] != 0u {
            flags = flags | 8u; // INSTANCE_FLAG_MOVABLE
        }
    }

    for (var s = 0u; s < draws.count; s = s + 1u) {
        let slot = draws.offset + s;
        if slot >= arrayLength(&sections) || slot >= u.output_rows {
            break;
        }
        let section = sections[slot];
        materials_out[slot] = section.material;
        objects_out[slot] = StaticObjectRow(
            row,
            generation,
            slot,
            generation,
            model,
            model,
            normal_mat,
            array<f32, 4>(center.x, center.y, center.z, radius),
            section.index_count,
            indices.offset + section.first_index,
            i32(vertices.offset),
            section.material_class,
            section.graph_hash_lo,
            section.graph_hash_hi,
            flags,
            skin.offset - vertices.offset,
            palette.offset,
            palette.count / 2u,
            s,
        );
    }
}
