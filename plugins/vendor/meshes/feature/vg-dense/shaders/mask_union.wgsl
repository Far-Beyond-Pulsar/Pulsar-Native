// Tiles Tier 3 must resolve this frame: the union of the dense mask the raster
// classified this frame (`cur`) and the mask the cull routed clusters with
// (`last`, last frame's). Clusters routed to the dense bin are not drawn by
// Tier 1/2, and routing a tile's clusters away starves the raster counters that
// keep the tile dense, so `cur` alone alternates ON/OFF frame to frame and
// leaves holes in the OFF frames; the union is stable.
struct UnionParams {
    tiles: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
}
@group(0) @binding(0) var<uniform> up: UnionParams;
@group(0) @binding(1) var<storage, read> cur_mask: array<u32>;
@group(0) @binding(2) var<storage, read> last_mask: array<u32>;
@group(0) @binding(3) var<storage, read_write> out_mask: array<u32>;

@compute @workgroup_size(64)
fn union_masks(@builtin(global_invocation_id) gid: vec3<u32>) {
    let t = gid.x;
    if t >= up.tiles || t >= arrayLength(&out_mask) { return; }
    var on = false;
    if t < arrayLength(&cur_mask) && cur_mask[t] != 0u { on = true; }
    if t < arrayLength(&last_mask) && last_mask[t] != 0u { on = true; }
    out_mask[t] = select(0u, 1u, on);
}
