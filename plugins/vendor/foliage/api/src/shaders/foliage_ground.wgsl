// The ground foliage grows on (`helio_foliage_api::ground`): placement writes
// one `GroundQuery` per candidate and a ground answers each with a
// `GroundHit` (`GROUND_QUERIES`, `GROUND_HITS`, `GROUND_ARGS`). Placement and
// every ground (the built-in plane, a voxel terrain) read these, never a copy.

/// `GpuGroundQuery` (32 bytes): a segment down the tile's up.
struct GroundQuery {
    start:  vec3<f32>,
    length: f32,
    up:     vec3<f32>,
    pad:    u32,
}

/// `GpuGroundHit` (16 bytes): where the segment met the ground (distance as
/// f32 bits, negative: no hit), its normal (packed), its ground id
/// (`GROUND_NONE`: no ground) and its linear colour (RGBA8, 0: none).
struct GroundHit {
    distance: u32,
    normal:   u32,
    ground:   u32,
    colour:   u32,
}

/// `GpuGroundArgs` (16 bytes): the answering dispatch's workgroups and the
/// query count.
struct GroundArgs {
    groups: vec3<u32>,
    count:  u32,
}

/// Ground id of a hit that met no ground (`GROUND_NONE`).
const GROUND_NONE: u32 = 0xffffffffu;

/// A hit that met no ground.
fn ground_miss() -> GroundHit {
    return GroundHit(bitcast<u32>(-1.0), 0u, GROUND_NONE, 0u);
}
