// Virtual-geometry instance derivation (see `virtual_geometry_instances.rs`).
// `helio_vg_format::WGSL` is prepended: VgInstance and the VG_INST_* flags.
//
// Three kernels, run in order on a change of any input:
//   cs_scatter_overrides  per VG component row: by_owner[owner] = its override row
//   cs_classify           per static-mesh instance row: candidate VgInstance
//                         (flags == 0 when not VG-active) + lod_suppressed flag
//   cs_compact            one workgroup: ordered compaction of the active
//                         candidates into vg_instances, count in vg_instance_count

struct Owner {
    owner_index: u32,
    owner_generation: u32,
    enabled: u32,
}

struct Handle {
    offset: u32,
    count: u32,
}

/// `helio_component::VgOverrideGpu`.
struct Override {
    flags: u32,
    lod_error_scale: f32,
}

/// `helio_component::virtual_geometry_instances::VgAssetReadyRow`.
struct AssetReady {
    vertex_offset: u32,
    vertex_count: u32,
    asset_slot: u32,
    flags: u32,
}

struct Uniforms {
    instance_rows: u32,
    override_rows: u32,
    // bit 0: virtual geometry globally enabled.
    flags: u32,
    _pad: u32,
}

const F32_MAX: f32 = 3.4028235e38;
// `static_objects` rows are 252 bytes = 63 words:
// word 0 mesh_slot, word 1 mesh_generation, words 4..20 the model matrix
// (column-major, four columns of four).
const OBJECT_WORDS: u32 = 63u;
const NO_ASSET: u32 = 0xffffffffu;
const ASSET_READY_BIT: u32 = 1u;
const OVERRIDE_PRESENT: u32 = 0x80000000u;
// Axis lengths of the model matrix may differ by this fraction and still count
// as a uniform (angle-preserving) scale.
const UNIFORM_SCALE_TOLERANCE: f32 = 1.0e-3;

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var<storage, read> owners: array<Owner>;
@group(0) @binding(2) var<storage, read> generations: array<u32>;
@group(0) @binding(3) var<storage, read> override_handles: array<Handle>;
@group(0) @binding(4) var<storage, read> override_pool: array<Override>;
@group(0) @binding(5) var<storage, read> section_handles: array<Handle>;
@group(0) @binding(6) var<storage, read> vertex_handles: array<Handle>;
@group(0) @binding(7) var<storage, read> objects: array<u32>;
@group(0) @binding(8) var<storage, read> asset_ready: array<AssetReady>;
// Per object entity: (override flags | OVERRIDE_PRESENT, bitcast scale), or
// zero when the object has no VirtualGeometryComponent. Copying the row here
// keeps cs_classify within 8 storage buffers.
@group(0) @binding(9) var<storage, read_write> by_owner: array<vec2<u32>>;
@group(0) @binding(10) var<storage, read_write> candidates: array<VgInstance>;
@group(0) @binding(11) var<storage, read_write> suppressed: array<atomic<u32>>;
@group(0) @binding(12) var<storage, read_write> out_instances: array<VgInstance>;
@group(0) @binding(13) var<storage, read_write> out_count: array<u32>;
// Per static_objects row: set for every section row of a VG-active instance.
@group(0) @binding(14) var<storage, read_write> classic_suppressed: array<atomic<u32>>;

fn object_f32(word: u32) -> f32 {
    return bitcast<f32>(objects[word]);
}

/// Rows of `asset_ready` are sorted ascending by `vertex_offset`; padding rows
/// carry `vertex_offset == 0xffffffff` and sort last.
fn find_asset(vertex_offset: u32) -> u32 {
    let n = arrayLength(&asset_ready);
    var low = 0u;
    var high = n;
    loop {
        if low >= high { break; }
        let mid = low + (high - low) / 2u;
        if asset_ready[mid].vertex_offset < vertex_offset {
            low = mid + 1u;
        } else {
            high = mid;
        }
    }
    if low < n && asset_ready[low].vertex_offset == vertex_offset
        && asset_ready[low].vertex_count != 0u {
        return low;
    }
    return NO_ASSET;
}

/// Cone (backface) culling in asset space is valid only when the instance
/// transform preserves angles and orientation: finite non-zero uniform scale
/// and no mirroring.
fn cone_cull_allowed(base: u32) -> bool {
    let c0 = vec3<f32>(object_f32(base + 4u), object_f32(base + 5u), object_f32(base + 6u));
    let c1 = vec3<f32>(object_f32(base + 8u), object_f32(base + 9u), object_f32(base + 10u));
    let c2 = vec3<f32>(object_f32(base + 12u), object_f32(base + 13u), object_f32(base + 14u));
    let l0 = length(c0);
    let l1 = length(c1);
    let l2 = length(c2);
    let low = min(l0, min(l1, l2));
    let high = max(l0, max(l1, l2));
    if !(low > 0.0) || !(high <= F32_MAX) {
        return false;
    }
    if high - low > UNIFORM_SCALE_TOLERANCE * high {
        return false;
    }
    return dot(cross(c0, c1), c2) > 0.0;
}

@compute @workgroup_size(64)
fn cs_scatter_overrides(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    if row >= u.override_rows || row >= arrayLength(&owners)
        || row >= arrayLength(&override_handles) {
        return;
    }
    let handle = override_handles[row];
    if handle.count == 0u || handle.offset >= arrayLength(&override_pool) {
        return;
    }
    let owner = owners[row];
    if owner.enabled == 0u || owner.owner_index >= arrayLength(&generations)
        || generations[owner.owner_index] != owner.owner_generation
        || owner.owner_index >= arrayLength(&by_owner) {
        return;
    }
    let authored = override_pool[handle.offset];
    by_owner[owner.owner_index] =
        vec2<u32>(authored.flags | OVERRIDE_PRESENT, bitcast<u32>(authored.lod_error_scale));
}

@compute @workgroup_size(64)
fn cs_classify(@builtin(global_invocation_id) gid: vec3<u32>) {
    let row = gid.x;
    if row >= u.instance_rows || row >= arrayLength(&candidates) {
        return;
    }
    candidates[row] = VgInstance(0u, 0u, 0.0, 0u);
    if (u.flags & 1u) == 0u {
        return;
    }
    if row >= arrayLength(&section_handles) || row >= arrayLength(&vertex_handles)
        || row >= arrayLength(&owners) {
        return;
    }
    // Placed by the scene join: its first section's object row is live and
    // belongs to this instance.
    let draws = section_handles[row];
    if draws.count == 0u {
        return;
    }
    let base = draws.offset * OBJECT_WORDS;
    if base + OBJECT_WORDS > arrayLength(&objects) {
        return;
    }
    if objects[base] != row || objects[base + 1u] == 0u {
        return;
    }
    // Asset readiness (root pages resident), found by the instance's interned
    // vertex range.
    let vertices = vertex_handles[row];
    if vertices.count == 0u {
        return;
    }
    let asset = find_asset(vertices.offset);
    if asset == NO_ASSET || (asset_ready[asset].flags & ASSET_READY_BIT) == 0u {
        return;
    }

    // Authored overrides of the owner object; none means global defaults.
    var flags = VG_INST_ENABLED | VG_INST_SHADOW_CASTER;
    var scale = 1.0;
    let owner = owners[row].owner_index;
    if owner < arrayLength(&by_owner) {
        let authored = by_owner[owner];
        if (authored.x & OVERRIDE_PRESENT) != 0u {
            if (authored.x & VG_INST_ENABLED) == 0u {
                return;
            }
            // Only authored bits; derived ones are set below.
            flags = authored.x
                & ~(OVERRIDE_PRESENT | VG_INST_LOD_SUPPRESS | VG_INST_CONE_CULL);
            scale = bitcast<f32>(authored.y);
        }
    }
    if !(scale > 0.0) || !(scale <= F32_MAX) {
        scale = 1.0;
    }
    flags = flags | VG_INST_LOD_SUPPRESS;
    if cone_cull_allowed(base) {
        flags = flags | VG_INST_CONE_CULL;
    }
    candidates[row] = VgInstance(asset_ready[asset].asset_slot, flags, scale, draws.offset);
    if owner < arrayLength(&suppressed) {
        atomicStore(&suppressed[owner], 1u);
    }
    for (var section = 0u; section < draws.count; section = section + 1u) {
        let object_row = draws.offset + section;
        if object_row < arrayLength(&classic_suppressed) {
            atomicStore(&classic_suppressed[object_row], 1u);
        }
    }
}

var<workgroup> scan: array<u32, 256>;

@compute @workgroup_size(256)
fn cs_compact(@builtin(local_invocation_index) li: u32) {
    let n = min(u.instance_rows, arrayLength(&candidates));
    var running = 0u;
    var start = 0u;
    loop {
        if start >= n { break; }
        let i = start + li;
        var live = 0u;
        var instance = VgInstance(0u, 0u, 0.0, 0u);
        if i < n {
            instance = candidates[i];
            if (instance.flags & VG_INST_ENABLED) != 0u {
                live = 1u;
            }
        }
        scan[li] = live;
        workgroupBarrier();
        for (var offset = 1u; offset < 256u; offset = offset << 1u) {
            var add = 0u;
            if li >= offset {
                add = scan[li - offset];
            }
            workgroupBarrier();
            scan[li] = scan[li] + add;
            workgroupBarrier();
        }
        if live == 1u {
            let dst = running + scan[li] - 1u;
            if dst < arrayLength(&out_instances) {
                out_instances[dst] = instance;
            }
        }
        let total = scan[255u];
        workgroupBarrier();
        running = running + total;
        start = start + 256u;
    }
    if li == 0u {
        out_count[0] = running;
    }
}
