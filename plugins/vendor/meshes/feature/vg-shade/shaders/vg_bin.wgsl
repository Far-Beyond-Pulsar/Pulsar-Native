// Material binning: per-bin pixel counts, exclusive prefix sum, per-bin pixel
// lists and indirect dispatch arguments. A bin is a material class clamped to
// the last bin; each bin is shaded by its own pipeline.

struct Params {
    width: u32,
    height: u32,
    flags: u32,
    bin_count: u32,
    pixel_count: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
}

const INVALID: u32 = 0xffffffffu;
const MATERIAL_WORDS: u32 = 24u;
const MATERIAL_CLASS_WORD: u32 = 19u;
// Linear pixel index across a 2D dispatch of 4096 workgroups of 64 per row.
const ROW_THREADS: u32 = 262144u;
// Shade dispatches use rows of 32768 workgroups of 64 threads.
const SHADE_ROW_GROUPS: u32 = 32768u;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> pixel_material: array<u32>;
@group(0) @binding(2) var<storage, read> materials: array<u32>;
// Per bin: [offset, count, cursor, _].
@group(0) @binding(3) var<storage, read_write> bins: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> pixel_list: array<u32>;
// Per bin: indirect dispatch (x, y, z, _).
@group(0) @binding(5) var<storage, read_write> dispatch_args: array<u32>;

fn bin_of(material_row: u32) -> u32 {
    let mclass = materials[material_row * MATERIAL_WORDS + MATERIAL_CLASS_WORD];
    return min(mclass, params.bin_count - 1u);
}

@compute @workgroup_size(64)
fn bin_count(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.y * ROW_THREADS + gid.x;
    if idx >= params.pixel_count {
        return;
    }
    let m = pixel_material[idx];
    if m == INVALID {
        return;
    }
    atomicAdd(&bins[bin_of(m) * 4u + 1u], 1u);
}

@compute @workgroup_size(1)
fn bin_scan() {
    var offset = 0u;
    for (var b = 0u; b < params.bin_count; b = b + 1u) {
        let count = atomicLoad(&bins[b * 4u + 1u]);
        atomicStore(&bins[b * 4u], offset);
        atomicStore(&bins[b * 4u + 2u], 0u);
        offset = offset + count;
        let groups = (count + 63u) / 64u;
        let x = min(groups, SHADE_ROW_GROUPS);
        var y = 1u;
        if x > 0u {
            y = (groups + x - 1u) / x;
        }
        dispatch_args[b * 4u] = x;
        dispatch_args[b * 4u + 1u] = y;
        dispatch_args[b * 4u + 2u] = 1u;
        dispatch_args[b * 4u + 3u] = 0u;
    }
}

@compute @workgroup_size(64)
fn bin_scatter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.y * ROW_THREADS + gid.x;
    if idx >= params.pixel_count {
        return;
    }
    let m = pixel_material[idx];
    if m == INVALID {
        return;
    }
    let b = bin_of(m);
    let slot = atomicAdd(&bins[b * 4u + 2u], 1u);
    pixel_list[atomicLoad(&bins[b * 4u]) + slot] = idx;
}
