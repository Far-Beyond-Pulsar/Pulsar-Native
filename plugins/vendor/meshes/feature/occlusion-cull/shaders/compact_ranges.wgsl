// Compact visible indirect draw records within each material/shading range.
// One workgroup owns one range, so its atomic allocator is deterministic with
// respect to range boundaries and never races another workgroup's output.
//
// The survivors are also appended to their material key's draw segment: a
// fixed region of `segment_indirect` the CPU assigned to that key (see
// `helio_pass_gbuffer::DrawSegments`). Draw passes record one draw per
// segment at its fixed offset, so they never need this frame's range layout.

struct Range {
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    start: u32,
    count: u32,
}

struct RangeCapacity {
    slots: u32,
    bucket: u32,
    segment_count: u32,
    _pad: u32,
}

struct Segment {
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    bucket: u32,
    first: u32,
    capacity: u32,
    _pad0: u32,
    _pad1: u32,
}

const NO_SEGMENT: u32 = 0xFFFFFFFFu;

@group(0) @binding(0) var<storage, read> source_indirect: array<u32>;
@group(0) @binding(1) var<storage, read_write> compacted_indirect: array<u32>;
@group(0) @binding(2) var<storage, read> range_counts: array<u32>;
@group(0) @binding(3) var<storage, read> opaque_ranges: array<Range>;
@group(0) @binding(4) var<storage, read> transparent_ranges: array<Range>;
@group(0) @binding(5) var<storage, read> forward_ranges: array<Range>;
@group(0) @binding(6) var<storage, read_write> draw_counts: array<u32>;
@group(0) @binding(7) var<uniform> capacity: RangeCapacity;
@group(0) @binding(8) var<storage, read> segments: array<Segment>;
@group(0) @binding(9) var<storage, read_write> segment_indirect: array<u32>;
@group(0) @binding(10) var<storage, read_write> segment_counts: array<atomic<u32>>;

var<workgroup> survivor_count: atomic<u32>;
var<workgroup> segment_slot: u32;
var<workgroup> segment_base: u32;

@compute @workgroup_size(64, 1, 1)
fn compact_ranges(
    @builtin(workgroup_id) workgroup: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
) {
    let bucket = capacity.bucket;
    let range_index = workgroup.x + workgroup.y * 65535u;
    let bucket_count = range_counts[bucket];
    if range_index >= bucket_count {
        return;
    }

    var range: Range;
    if bucket == 0u {
        range = opaque_ranges[range_index];
    } else if bucket == 1u {
        range = transparent_ranges[range_index];
    } else {
        range = forward_ranges[range_index];
    }

    if local.x == 0u {
        atomicStore(&survivor_count, 0u);
    }
    workgroupBarrier();

    // Order within one material range has no semantic meaning. Atomic
    // allocation packs live records without a serial per-draw CPU loop.
    for (var i = local.x; i < range.count; i += 64u) {
        let source_slot = range.start + i;
        if source_indirect[source_slot * 5u + 1u] != 0u {
            let destination = range.start + atomicAdd(&survivor_count, 1u);
            for (var word = 0u; word < 5u; word += 1u) {
                compacted_indirect[destination * 5u + word] =
                    source_indirect[source_slot * 5u + word];
            }
        }
    }

    workgroupBarrier();
    let count = atomicLoad(&survivor_count);

    // Clear the unused tail. This keeps the non-count multi-draw fallback
    // correct while count-capable adapters skip the same empty slots.
    for (var i = count + local.x; i < range.count; i += 64u) {
        compacted_indirect[(range.start + i) * 5u + 1u] = 0u;
    }

    if local.x == 0u {
        let slot = 4u + bucket * capacity.slots + range_index;
        draw_counts[slot] = count;

        // Reserve room in this key's segment. Several ranges can share a key
        // (the sort interleaves them by mesh), so they append atomically.
        segment_slot = NO_SEGMENT;
        for (var s = 0u; s < capacity.segment_count; s += 1u) {
            let segment = segments[s];
            if segment.bucket == bucket
                && segment.material_class == range.material_class
                && segment.graph_hash_lo == range.graph_hash_lo
                && segment.graph_hash_hi == range.graph_hash_hi {
                segment_slot = s;
                break;
            }
        }
        if segment_slot != NO_SEGMENT && count > 0u {
            segment_base = atomicAdd(&segment_counts[segment_slot], count);
        }
    }

    // The compacted records above are read back below by other lanes.
    storageBarrier();
    workgroupBarrier();

    // A key the CPU has not added yet is skipped until it is: it is never
    // drawn under another key's pipeline. Records past the segment's
    // capacity are dropped; draws read at most `capacity` records.
    let segment_index = segment_slot;
    if segment_index == NO_SEGMENT {
        return;
    }
    let segment = segments[segment_index];
    for (var i = local.x; i < count; i += 64u) {
        let destination = segment_base + i;
        if destination < segment.capacity {
            for (var word = 0u; word < 5u; word += 1u) {
                segment_indirect[(segment.first + destination) * 5u + word] =
                    compacted_indirect[(range.start + i) * 5u + word];
            }
        }
    }
}
