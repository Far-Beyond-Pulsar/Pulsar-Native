//!use helio_prelude
//!use helio_foliage_common
// Foliage tile cull, cluster cull + compaction, and indirect finalize — stages 2 and 3
// of `FoliagePlacePass`.
//
// Three entry points, dispatched in order on the *main render encoder* with an implicit
// barrier between them:
//
//   cs_tile_cull     one lane per ring slot   -> tile_visible[slot]
//   cs_cluster_cull  one lane per cluster     -> visible_blades[lod region] + counters
//   cs_finalize      four lanes               -> foliage_indirect[0..4]
//
// ── Why the main encoder and not `chain_transparent` ────────────────────────────
//
// `chain_transparent` passes must record exclusively on the separate compute encoder,
// and the two encoders are submitted as `[compute_encoder, encoder]`, so *all* compute-
// encoder work runs before *all* render-encoder work. A `chain_transparent` foliage cull
// would therefore Hi-Z-test against the **previous** frame's pyramid. `HiZBuildPass` and
// `OcclusionCullPass` both use `ctx.encoder_ptr` for exactly this reason and neither opts
// in. See the plan's §6.2 (marked [audit] — the first draft had this wrong).

const WG_SIZE: u32 = 64u;

const FOLIAGE_LOD_COUNT: u32 = 4u;
const FOLIAGE_LOD_NONE: u32 = 4u;


// Mirrors crate::TileState. `Placing` is excluded on purpose: its slab
// contents are undefined until the placement dispatch's final barrier, so drawing it
// renders whatever the previous tenant left behind.
const TILE_STATE_RESIDENT: u32 = 2u;
const TILE_STATE_EVICTING: u32 = 3u;

// Mirrors the COUNTER_* constants in src/lib.rs.
const COUNTER_VISIBLE_OVERFLOW: u32 = 4u;

/// Mirrors `FoliageCullUniforms` (Rust, 64 bytes).
struct FoliageCullUniforms {
    tile_count:             u32,
    screen_width:           u32,
    screen_height:          u32,
    hiz_mip_count:          u32,

    // 0 on frame 0 and right after a resize rebuilds the graph: the Hi-Z pyramid has not
    // been built from any real depth yet, so its texels hold whatever the graph's
    // texture pool left there, and testing against them can cull the entire world for
    // that frame. Copied verbatim from `CullUniforms::hiz_valid` in vg_cull.wgsl.
    hiz_valid:              u32,
    cluster_size:           u32,
    clusters_per_tile:      u32,
    per_lod_capacity:       u32,

    tile_size:              f32,
    lod_quality_scale:      f32,
    type_count:             u32,
    cluster_dispatch_width: u32,

    max_foliage_height:     f32,
    wpo_extent:             f32,
    /// Width of the LOD cross-fade overlap band, in metres. Must match the consumer's
    /// `lod_fade_band`: this pass decides which clusters get emitted into *two* LODs,
    /// and the consumer decides their blend weights. A mismatch leaves instances that
    /// blend to less (gap) or more (double-density ring) than one blade's coverage.
    lod_fade_band:          f32,
    _pad0:                  u32,
}

/// Mirrors `wgpu::util::DrawIndirectArgs` (16 bytes).
struct DrawIndirect {
    vertex_count:   u32,
    instance_count: u32,
    first_vertex:   u32,
    first_instance: u32,
}

@group(0) @binding(0)  var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1)  var<uniform> cull: FoliageCullUniforms;
@group(0) @binding(2)  var<storage, read> tiles: array<FoliageTile>;
@group(0) @binding(3)  var<storage, read> blades: array<BladeInstance>;
@group(0) @binding(4)  var<storage, read> types: array<FoliageType>;
@group(0) @binding(5)  var<storage, read_write> visible_blades: array<u32>;
@group(0) @binding(6)  var<storage, read_write> counters: array<atomic<u32>>;
@group(0) @binding(7)  var<storage, read_write> indirect: array<DrawIndirect>;
@group(0) @binding(8)  var<storage, read_write> tile_visible: array<u32>;
@group(0) @binding(9)  var hiz_tex: texture_2d<f32>;
@group(0) @binding(10) var hiz_samp: sampler;
@group(0) @binding(11) var<storage, read> frames: array<TileFrame>;
@group(0) @binding(12) var<uniform> foliage_bands: FoliageBands;

var<workgroup> wg_planes: array<vec4<f32>, 5>;

// ── Shared helpers ──────────────────────────────────────────────────────────────

/// True for finite values only. WGSL has no `isnan`/`isinf`, and `x == x` is legal for a
/// compiler to fold away under fast-math, so this tests the exponent field directly.
fn is_finite_f32(value: f32) -> bool {
    return (bitcast<u32>(value) & 0x7f800000u) != 0x7f800000u;
}

/// Left, right, bottom, top and near planes (helio_core::depth::frustum_planes).
/// There is no far plane: the camera projection has none, and the textbook one
/// (row 2 alone) is `(0, 0, 0, near)` under it — normalizing that divides by zero
/// and the NaN culls everything.
fn publish_frustum_planes() {
    let vp = cameras[0].view_proj;
    let p0 = vec4<f32>(vp[0][3] + vp[0][0], vp[1][3] + vp[1][0], vp[2][3] + vp[2][0], vp[3][3] + vp[3][0]);
    let p1 = vec4<f32>(vp[0][3] - vp[0][0], vp[1][3] - vp[1][0], vp[2][3] - vp[2][0], vp[3][3] - vp[3][0]);
    let p2 = vec4<f32>(vp[0][3] + vp[0][1], vp[1][3] + vp[1][1], vp[2][3] + vp[2][1], vp[3][3] + vp[3][1]);
    let p3 = vec4<f32>(vp[0][3] - vp[0][1], vp[1][3] - vp[1][1], vp[2][3] - vp[2][1], vp[3][3] - vp[3][1]);
    let p4 = vec4<f32>(vp[0][3] - vp[0][2], vp[1][3] - vp[1][2], vp[2][3] - vp[2][2], vp[3][3] - vp[3][2]);
    wg_planes[0] = p0 / length(p0.xyz);
    wg_planes[1] = p1 / length(p1.xyz);
    wg_planes[2] = p2 / length(p2.xyz);
    wg_planes[3] = p3 / length(p3.xyz);
    wg_planes[4] = p4 / length(p4.xyz);
}

fn sphere_visible(center: vec3<f32>, radius: f32) -> bool {
    return (dot(wg_planes[0].xyz, center) + wg_planes[0].w >= -radius)
        && (dot(wg_planes[1].xyz, center) + wg_planes[1].w >= -radius)
        && (dot(wg_planes[2].xyz, center) + wg_planes[2].w >= -radius)
        && (dot(wg_planes[3].xyz, center) + wg_planes[3].w >= -radius)
        && (dot(wg_planes[4].xyz, center) + wg_planes[4].w >= -radius);
}

/// Conservative farthest-depth Hi-Z test, transcribed from `vg_cull.wgsl` (the block around
/// lines 199-252) including the `hiz_valid` frame-0 guard.
///
/// Rejects only when the *complete* projected sphere is on screen and all four corners of
/// its footprint are behind existing depth at the chosen mip. Every early `return false`
/// below is "cannot prove occlusion" — the conservative answer — not "visible".
fn hiz_occluded(center_ws: vec3<f32>, world_radius: f32) -> bool {
    if cull.hiz_valid == 0u {
        return false;
    }
    let cull_clip = cameras[0].view_proj * vec4<f32>(center_ws, 1.0);
    if cull_clip.w <= 0.0 {
        return false;
    }
    let cull_ndc = cull_clip.xyz / cull_clip.w;
    let cull_uv = vec2<f32>(cull_ndc.x * 0.5 + 0.5, cull_ndc.y * -0.5 + 0.5);
    let nearest_view_depth = cull_clip.w - world_radius;
    if nearest_view_depth <= cameras[0].position_near.w {
        return false;
    }

    let ndc_r = max(
        abs(world_radius * cameras[0].proj[0][0] / nearest_view_depth),
        abs(world_radius * cameras[0].proj[1][1] / nearest_view_depth),
    );
    let uv_radius = ndc_r * 0.5;
    let uv_min = cull_uv - vec2<f32>(uv_radius);
    let uv_max = cull_uv + vec2<f32>(uv_radius);
    if !(all(uv_min >= vec2<f32>(0.0)) && all(uv_max <= vec2<f32>(1.0))) {
        return false;
    }

    let cam_to_center = center_ws - cameras[0].position_near.xyz;
    let dist_sq = dot(cam_to_center, cam_to_center);
    // Reversed depth: the near plane (1) unless the sphere is wholly in front of the eye.
    var near_z = 1.0;
    if dist_sq > world_radius * world_radius {
        let direction = cam_to_center / sqrt(dist_sq);
        let near_ws = center_ws - direction * world_radius;
        let near_clip = cameras[0].view_proj * vec4<f32>(near_ws, 1.0);
        if near_clip.w > 0.0 {
            near_z = clamp(near_clip.z / near_clip.w, 0.0, 1.0);
        }
    }

    let half_height = f32(cull.screen_height) * 0.5;
    let diameter_px = max(ndc_r * half_height * 2.0, 1.0);
    let mip = clamp(u32(ceil(log2(diameter_px))), 0u, max(cull.hiz_mip_count, 1u) - 1u);
    let hiz_00 = textureSampleLevel(hiz_tex, hiz_samp, uv_min, f32(mip)).r;
    let hiz_01 = textureSampleLevel(hiz_tex, hiz_samp, vec2<f32>(uv_max.x, uv_min.y), f32(mip)).r;
    let hiz_10 = textureSampleLevel(hiz_tex, hiz_samp, vec2<f32>(uv_min.x, uv_max.y), f32(mip)).r;
    let hiz_11 = textureSampleLevel(hiz_tex, hiz_samp, uv_max, f32(mip)).r;
    let hiz_depth = helio_depth_farthest(helio_depth_farthest(hiz_00, hiz_01), helio_depth_farthest(hiz_10, hiz_11));
    return helio_hiz_occludes(hiz_depth, near_z);
}

// ── Stage 2a: tile cull ─────────────────────────────────────────────────────────

@compute @workgroup_size(64)
fn cs_tile_cull(
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    if lane == 0u {
        publish_frustum_planes();
    }
    workgroupBarrier();

    let slot = workgroup_id.x * WG_SIZE + lane;
    if slot >= cull.tile_count || slot >= arrayLength(&tiles) || slot >= arrayLength(&tile_visible) {
        return;
    }

    let tile = tiles[slot];
    var visible = 0u;
    if (tile.state == TILE_STATE_RESIDENT || tile.state == TILE_STATE_EVICTING)
        && tile.blade_count > 0u
    {
        let frame = frames[slot];
        let band = foliage_bands.bands[foliage_band_of_slot(slot)];
        let center = frame.origin + (frame.axis_u + frame.axis_v) * 0.5 + frame.up * tile.bounds_center;
        // Dilate by the tallest foliage plus its wind displacement before testing.
        // Under-dilating here is the classic wind-culling bug: blades blow outside the
        // tile bounds and the tile is culled while its geometry is still on screen.
        let radius = 0.5 * max(length(frame.axis_u + frame.axis_v), length(frame.axis_u - frame.axis_v))
            + tile.bounds_half + cull.max_foliage_height + cull.wpo_extent;
        // Only the tiles reaching into their band's distances draw: the inner tiles of a
        // far band's ring lie under the nearer bands.
        let d = length(center - cameras[0].position_near.xyz);
        let in_band = d + radius > band.inner - band.fade_in && d - radius < band.outer;
        if in_band && sphere_visible(center, radius) && !hiz_occluded(center, radius) {
            visible = 1u;
        }
    }
    tile_visible[slot] = visible;
}

// ── Stage 2b: cluster cull + per-LOD compaction ─────────────────────────────────

@compute @workgroup_size(64)
fn cs_cluster_cull(
    @builtin(workgroup_id) workgroup_id: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    if lane == 0u {
        publish_frustum_planes();
    }
    workgroupBarrier();

    let group = workgroup_id.x + workgroup_id.y * max(cull.cluster_dispatch_width, 1u);
    let index = group * WG_SIZE + lane;
    let clusters_per_tile = max(cull.clusters_per_tile, 1u);
    let slot = index / clusters_per_tile;
    let cluster = index - slot * clusters_per_tile;

    if slot >= cull.tile_count || slot >= arrayLength(&tiles) || slot >= arrayLength(&tile_visible) {
        return;
    }
    if tile_visible[slot] == 0u {
        return;
    }

    let tile = tiles[slot];
    let frame = frames[slot];
    let cluster_size = max(cull.cluster_size, 1u);
    let first = cluster * cluster_size;
    if first >= tile.blade_count {
        return;
    }
    let count = min(cluster_size, tile.blade_count - first);
    let band = foliage_bands.bands[foliage_band_of_slot(slot)];
    let eye = cameras[0].position_near.xyz;

    // Real per-cluster bounds, read from the records. The alternative — reusing the tile's
    // bounds — makes cluster culling exactly as precise as tile culling, i.e. useless.
    var bounds_min = vec3<f32>(1.0e30);
    var bounds_max = vec3<f32>(-1.0e30);
    var sampled = 0u;
    for (var i = 0u; i < count; i = i + 1u) {
        let arena_index = tile.blade_offset + first + i;
        if arena_index >= arrayLength(&blades) {
            break;
        }
        let position = foliage_blade_root(tile, frame, blades[arena_index]);
        bounds_min = min(bounds_min, position);
        bounds_max = max(bounds_max, position);
        sampled = sampled + 1u;
    }
    if sampled == 0u {
        return;
    }

    let center = (bounds_min + bounds_max) * 0.5;
    let dilate = cull.max_foliage_height + cull.wpo_extent;
    let radius = length(bounds_max - center) + dilate;

    if !sphere_visible(center, radius) {
        return;
    }
    if hiz_occluded(center, radius) {
        return;
    }

    // The band decides the LOD: the near band's blades at L0 or L1 (by the cluster's
    // first record's type, with the cluster in both inside the fade band), a tuft of
    // blades at L2, a tuft as one shape at L3.
    let distance = length(center - eye);
    let head = blades[tile.blade_offset + first];
    let type_id = min((head.packed_scale_type >> 16u) & 0xffu, max(min(cull.type_count, arrayLength(&types)), 1u) - 1u);
    let foliage = types[type_id];
    var lod = band.lod;
    var second = FOLIAGE_LOD_NONE;
    if band.lod == FOLIAGE_BAND_LOD_BLADES {
        let boundary = foliage.lod0 * select(1.0, cull.lod_quality_scale, is_finite_f32(cull.lod_quality_scale) && cull.lod_quality_scale > 0.0);
        lod = select(1u, 0u, distance < boundary);
        // Inside the switch band each tuft draws at one of L0 and L1 by its hash (the
        // vertex shader's switch), so a cluster there is emitted at both.
        if lod == 0u && distance > boundary - cull.lod_fade_band {
            second = 1u;
        }
    }

    // Which records draw: those whose tuft's scale (the band's fade, the kept share
    // against its rank, the type's reach) is above zero. Whole tufts drop out together:
    // a tuft's records share their rank and position.
    var keep_mask = 0u;
    for (var i = 0u; i < count; i = i + 1u) {
        let blade = blades[tile.blade_offset + first + i];
        let d = length(foliage_blade_root(tile, frame, blade) - eye);
        let ty = types[min((blade.packed_scale_type >> 16u) & 0xffu, max(min(cull.type_count, arrayLength(&types)), 1u) - 1u)];
        if foliage_tuft_scale(band, d, foliage_tuft_rank(blade)) * foliage_type_reach(ty, d) > 0.0 {
            keep_mask = keep_mask | (1u << i);
        }
    }
    if keep_mask == 0u {
        return;
    }
    emit_cluster(lod, keep_mask, slot, first);
    if second < FOLIAGE_LOD_COUNT {
        emit_cluster(second, keep_mask, slot, first);
    }
}

/// Append the kept records of one cluster (`mask`, a bit per record) into
/// `visible_blades[]` for a single LOD.
fn emit_cluster(lod: u32, mask: u32, slot: u32, first: u32) {
    let emit = countOneBits(mask);

    // Bounded append. The reservation can overshoot the region, so the finalize stage
    // clamps `instance_count` — and the partial write below guarantees every slot below
    // that clamp was actually written: a "reserve then drop" policy would leave a hole of
    // uninitialised indices inside the drawn range.
    let capacity = cull.per_lod_capacity;
    let base = atomicAdd(&counters[lod], emit);
    if base >= capacity {
        atomicAdd(&counters[COUNTER_VISIBLE_OVERFLOW], emit);
        return;
    }
    let writable = min(emit, capacity - base);
    if writable < emit {
        atomicAdd(&counters[COUNTER_VISIBLE_OVERFLOW], emit - writable);
    }

    let region = lod * capacity;
    var written = 0u;
    var bits = mask;
    loop {
        if bits == 0u || written >= writable {
            break;
        }
        let i = firstTrailingBit(bits);
        bits = bits & (bits - 1u);
        let out_index = region + base + written;
        if out_index < arrayLength(&visible_blades) {
            // Packed reference, NOT a flat arena index: tile slot in the high 16 bits,
            // tile-local record index in the low 16. The consumer mirrors this as
            // `VISIBLE_TILE_SHIFT` / `VISIBLE_LOCAL_MASK`.
            visible_blades[out_index] = (slot << 16u) | ((first + i) & 0xffffu);
        }
        written = written + 1u;
    }
}

// ── Stage 3: counters -> DrawIndirectArgs ───────────────────────────────────────

@compute @workgroup_size(4)
fn cs_finalize(@builtin(local_invocation_index) lane: u32) {
    if lane >= FOLIAGE_LOD_COUNT || lane >= arrayLength(&indirect) {
        return;
    }

    // Smooth foliage's strip at each LOD (FOLIAGE_LOD_VERTEX_COUNTS), a `TriangleStrip`
    // with no vertex or index buffer; none without a smooth type. Blocky foliage is drawn
    // as boxes the draw expands from these same instances. Written as a branch rather than
    // a const array because indexing a `const` array with a runtime value is not portable
    // across WGSL front-ends.
    var vertex_count = 13u;
    if lane == 0u {
        vertex_count = 11u;
    } else if lane == 1u {
        vertex_count = 7u;
    } else if lane == 2u {
        vertex_count = 88u;
    }
    var any_smooth = false;
    let rows = min(cull.type_count, arrayLength(&types));
    for (var t = 0u; t < rows; t++) {
        if (types[t].kind_and_flags & 0xffu) != FOLIAGE_KIND_BLOCK && types[t].density > 0.0 {
            any_smooth = true;
            break;
        }
    }
    if !any_smooth {
        vertex_count = 0u;
    }

    let raw = atomicLoad(&counters[lane]);
    let instance_count = min(raw, cull.per_lod_capacity);

    indirect[lane].vertex_count = vertex_count;
    indirect[lane].instance_count = instance_count;
    indirect[lane].first_vertex = 0u;
    // Always 0. The per-LOD region base is applied by the consumer through
    // `lod_region_offset`, not by the draw call, so that a single vertex shader can index
    // `visible_blades` with `instance_index` in every LOD.
    indirect[lane].first_instance = 0u;
}
