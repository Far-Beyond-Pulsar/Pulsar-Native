// The sun's occlusion by terrain that does not draw into the shadow maps
// (`sun_occlusion` module docs): a virtual clipmap of orthographic depth
// pages along the sun. Level k's texels are 2^k finest texels wide; a
// reader takes the level whose texel is about its footprint (or the
// nearest coarser one resident). Each page holds the distance from its
// near plane to the first occluder (0: none). Bind `sun_occlusion_frame`
// (a `SunOcclusionFrame`), `sun_occlusion_maps` (texture_2d<f32>, R32Float:
// the physical pages) and `sun_occlusion_pages` (array<u32>: the page
// tables, then each physical page's near plane).

const SUN_OCCLUSION_LEVEL_COUNT: u32 = 20u;
const SUN_OCCLUSION_EPOCH_COUNT: u32 = 2u;
// Texels along a page's side (a power of two: `SUN_OCCLUSION_PAGE_SHIFT`).
const SUN_OCCLUSION_PAGE_SHIFT: u32 = 6u;
// Pages along a level's window side, around the eye (a power of two).
const SUN_OCCLUSION_WINDOW_SHIFT: u32 = 7u;
const SUN_OCCLUSION_NONE: u32 = 0xffffffffu;

// Level k of an epoch: the eye's texel (integer part; the level's texels
// counted from the epoch's anchor along right and down) and its fraction.
struct SunOcclusionLevel {
    texel: vec2<i32>,
    frac: vec2<f32>,
}

// An epoch: the sun its pages were traced for, as the axes of their grid
// (right and down across, forward from the sun; right.w its page table's
// slot + 1, 0 when there is no such epoch), and the eye in each level's
// texels.
struct SunOcclusionEpoch {
    right: vec4<f32>,
    down: vec4<f32>,
    forward: vec4<f32>,
    levels: array<SunOcclusionLevel, 20>,
}

struct SunOcclusionFrame {
    // x: the finest texel (m; 0: no occlusion published), y: the levels,
    // z: texels along a page's side, w: pages along a window's side.
    info: vec4<f32>,
    // x: physical pages along the atlas's side, y: where the physical
    // pages' near planes start in `sun_occlusion_pages` (words).
    atlas: vec4<u32>,
    // The current epoch, then the one before (its pages read until the
    // current one has its own).
    epochs: array<SunOcclusionEpoch, 2>,
}

// The level whose texel is the largest no wider than `footprint` (m).
fn sun_occlusion_level(footprint: f32) -> u32 {
    let finest = sun_occlusion_frame.info.x;
    let levels = u32(sun_occlusion_frame.info.y);
    if finest <= 0.0 || levels == 0u { return 0u; }
    return u32(clamp(floor(log2(max(footprint, 1e-6) / finest)), 0.0, f32(levels - 1u)));
}

// The texel of camera-relative point `p` in level `level` of epoch `e`, as
// a whole texel and the fraction past its corner.
struct SunOcclusionTexel {
    texel: vec2<i32>,
    frac: vec2<f32>,
}

// (Fields are read one by one: a whole epoch copied out of the uniform in
// a loop costs a reader like fog's several times its work.)
fn sun_occlusion_texel(p: vec3<f32>, e: u32, level: u32) -> SunOcclusionTexel {
    let l = sun_occlusion_frame.epochs[e].levels[level];
    let texel = sun_occlusion_frame.info.x * exp2(f32(level));
    let across = vec2<f32>(dot(p, sun_occlusion_frame.epochs[e].right.xyz), dot(p, sun_occlusion_frame.epochs[e].down.xyz));
    let c = across / texel + l.frac;
    let whole = floor(c);
    return SunOcclusionTexel(l.texel + vec2<i32>(whole), c - whole);
}

// Page `page` of level `level` in table `slot`, from the eye's page `eye`:
// its physical page, or none (not resident, or outside the level's window).
fn sun_occlusion_physical(slot: u32, level: u32, page: vec2<i32>, eye: vec2<i32>) -> u32 {
    let half = 1 << (SUN_OCCLUSION_WINDOW_SHIFT - 1u);
    if any(abs(page - eye) >= vec2<i32>(half)) { return SUN_OCCLUSION_NONE; }
    let side = 1u << SUN_OCCLUSION_WINDOW_SHIFT;
    let wrap = vec2<u32>(page) & vec2<u32>(side - 1u);
    let entry = sun_occlusion_pages[((slot * SUN_OCCLUSION_LEVEL_COUNT + level) * side + wrap.y) * side + wrap.x];
    let tag = vec2<u32>(page >> vec2<u32>(SUN_OCCLUSION_WINDOW_SHIFT)) & vec2<u32>(511u);
    let physical = entry & 0x1fffu;
    if physical == 0u || ((entry >> 13u) & 511u) != tag.x || ((entry >> 22u) & 511u) != tag.y {
        return SUN_OCCLUSION_NONE;
    }
    return physical - 1u;
}

// The sun's share at camera-relative `p` by level `level` of epoch `e`, a
// 2x2 percentage filter: x the share, y 1 when its pages are resident (else
// 0, the share 1). `n`, the receiver's normal (zero: none), compares each
// tap against the receiver's plane through it, so lit slopes keep no acne
// at any texel size; the bias is a texel and half its slope.
fn sun_occlusion_read_level(p: vec3<f32>, n: vec3<f32>, e: u32, level: u32) -> vec2<f32> {
    let texel = sun_occlusion_frame.info.x * exp2(f32(level));
    let at = sun_occlusion_texel(p, e, level);
    // The taps around the point: from the texel left of and above it when
    // it lies in its texel's first half.
    let shift = select(vec2<i32>(0), vec2<i32>(-1), at.frac < vec2<f32>(0.5));
    let base = at.texel + shift;
    let eye = sun_occlusion_frame.epochs[e].levels[level].texel >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT);
    let slot = u32(sun_occlusion_frame.epochs[e].right.w) - 1u;
    // Not resident here: nothing more to work out.
    var last = base >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT);
    var physical = sun_occlusion_physical(slot, level, last, eye);
    if physical == SUN_OCCLUSION_NONE { return vec2<f32>(1.0, 0.0); }
    let f = at.frac - 0.5 - vec2<f32>(shift);
    let right = sun_occlusion_frame.epochs[e].right.xyz;
    let down = sun_occlusion_frame.epochs[e].down.xyz;
    let forward = sun_occlusion_frame.epochs[e].forward.xyz;
    let toward = dot(n, forward);
    var slope = vec2<f32>(0.0);
    if toward < -0.05 {
        slope = clamp(-vec2<f32>(dot(n, right), dot(n, down)) / toward, vec2<f32>(-4.0), vec2<f32>(4.0));
    }
    let bias = texel * (1.0 + 0.5 * (abs(slope.x) + abs(slope.y)));
    let depth = dot(p, forward);
    let side = sun_occlusion_frame.atlas.x;
    let page_side = 1 << SUN_OCCLUSION_PAGE_SHIFT;
    var lit = vec4<f32>(0.0);
    var near = bitcast<f32>(sun_occlusion_pages[sun_occlusion_frame.atlas.y + physical]);
    for (var k = 0; k < 4; k++) {
        let tap = vec2<i32>(k & 1, k >> 1);
        let t = base + tap;
        let page = t >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT);
        if any(page != last) {
            physical = sun_occlusion_physical(slot, level, page, eye);
            last = page;
            if physical == SUN_OCCLUSION_NONE { return vec2<f32>(1.0, 0.0); }
            near = bitcast<f32>(sun_occlusion_pages[sun_occlusion_frame.atlas.y + physical]);
        }
        let corner = vec2<i32>(i32(physical % side), i32(physical / side)) * page_side;
        let stored = textureLoad(sun_occlusion_maps, corner + (t & vec2<i32>(page_side - 1)), 0).r;
        // The receiver's plane at the tap's centre.
        let receiver = depth - near + dot((vec2<f32>(tap) - f) * texel, slope);
        lit[k] = select(0.0, 1.0, stored <= 0.0 || receiver <= stored + bias);
    }
    return vec2<f32>(mix(mix(lit.x, lit.y, f.x), mix(lit.z, lit.w, f.x), f.y), 1.0);
}

// The sun's share at camera-relative `p` (receiver normal `n`, zero for
// none; `footprint`, m: what the reader resolves there): from the level
// whose texel is about the footprint, else the nearest coarser level
// resident (at most `climb` levels coarser), the current epoch's pages
// before the last one's. x: the share; y: 1 + how many levels coarser it
// came from (0: none resident, the share 1).
fn sun_occlusion_at(p: vec3<f32>, n: vec3<f32>, footprint: f32, climb: u32) -> vec2<f32> {
    let levels = u32(sun_occlusion_frame.info.y);
    if sun_occlusion_frame.info.x <= 0.0 || levels == 0u { return vec2<f32>(1.0, 0.0); }
    let want = sun_occlusion_level(footprint);
    let last = min(min(levels, SUN_OCCLUSION_LEVEL_COUNT), want + climb + 1u);
    for (var level = want; level < last; level++) {
        for (var e = 0u; e < SUN_OCCLUSION_EPOCH_COUNT; e++) {
            if sun_occlusion_frame.epochs[e].right.w == 0.0 { continue; }
            let r = sun_occlusion_read_level(p, n, e, level);
            if r.y > 0.0 { return vec2<f32>(r.x, f32(level - want + 1u)); }
        }
    }
    return vec2<f32>(1.0, 0.0);
}

// The share of the sun reaching camera-relative point `p` (receiver normal
// `n`, zero for none, `footprint` m): 0 behind terrain, else 1, filtered
// across the texel; 1 where no page holds it.
fn sun_occlusion(p: vec3<f32>, n: vec3<f32>, footprint: f32) -> f32 {
    return sun_occlusion_at(p, n, footprint, SUN_OCCLUSION_LEVEL_COUNT).x;
}

// One tap of level `level` of epoch `e` at camera-relative `p`, against a
// texel's bias: x the share (0 or 1), y 1 when its page is resident (else
// 0, the share 1).
fn sun_occlusion_read_point(p: vec3<f32>, e: u32, level: u32) -> vec2<f32> {
    let at = sun_occlusion_texel(p, e, level);
    let eye = sun_occlusion_frame.epochs[e].levels[level].texel >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT);
    let slot = u32(sun_occlusion_frame.epochs[e].right.w) - 1u;
    let physical = sun_occlusion_physical(slot, level, at.texel >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT), eye);
    if physical == SUN_OCCLUSION_NONE { return vec2<f32>(1.0, 0.0); }
    let side = sun_occlusion_frame.atlas.x;
    let page_side = 1 << SUN_OCCLUSION_PAGE_SHIFT;
    let corner = vec2<i32>(i32(physical % side), i32(physical / side)) * page_side;
    let stored = textureLoad(sun_occlusion_maps, corner + (at.texel & vec2<i32>(page_side - 1)), 0).r;
    let near = bitcast<f32>(sun_occlusion_pages[sun_occlusion_frame.atlas.y + physical]);
    let receiver = dot(p, sun_occlusion_frame.epochs[e].forward.xyz) - near;
    let texel = sun_occlusion_frame.info.x * exp2(f32(level));
    return vec2<f32>(select(0.0, 1.0, stored <= 0.0 || receiver <= stored + texel), 1.0);
}

// `sun_occlusion` without a receiver or filter, a tap: for readers that
// average many samples themselves (fog), a quarter of the reads.
fn sun_occlusion_point(p: vec3<f32>, footprint: f32) -> f32 {
    let levels = u32(sun_occlusion_frame.info.y);
    if sun_occlusion_frame.info.x <= 0.0 || levels == 0u { return 1.0; }
    for (var level = sun_occlusion_level(footprint); level < min(levels, SUN_OCCLUSION_LEVEL_COUNT); level++) {
        for (var e = 0u; e < SUN_OCCLUSION_EPOCH_COUNT; e++) {
            if sun_occlusion_frame.epochs[e].right.w == 0.0 { continue; }
            let r = sun_occlusion_read_point(p, e, level);
            if r.y > 0.0 { return r.x; }
        }
    }
    return 1.0;
}
