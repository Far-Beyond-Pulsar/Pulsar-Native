// The sprite sort's prepare step: the cull's GPU-written visible count as
// the sort's `(count, num_blocks)` and its indirect dispatch size
// (`helio_core::radix_sort`), so the sort follows what is visible rather
// than `max_visible`.

const WG: u32 = 256u;

struct FrameUniform {
    count: u32,
    num_blocks: u32,
}

// ── cs_prepare ──────────────────────────────────────────────────────────────

@group(0) @binding(0) var<storage, read> indirect_in: array<u32>;
@group(0) @binding(1) var<storage, read_write> frame_uniform_rw: FrameUniform;
@group(0) @binding(2) var<storage, read_write> dispatch_args: array<u32>;

@compute @workgroup_size(1)
fn cs_prepare() {
    let count = indirect_in[1]; // DrawIndexedIndirectArgs.instance_count
    let num_blocks = max((count + WG - 1u) / WG, 1u);
    frame_uniform_rw.count = count;
    frame_uniform_rw.num_blocks = num_blocks;
    dispatch_args[0] = num_blocks;
    // dispatch_args[1]/[2] (y/z workgroup counts) are always 1, written once
    // at buffer creation — this kernel only ever updates the x count.
}
