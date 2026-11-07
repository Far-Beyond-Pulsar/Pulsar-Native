//! Particle emitters' scene rows (a row join, see
//! `helio_scene_join::RowJoin`): placed at the owner's transform, packed into
//! [`crate::CORONA_MAX_EMITTERS`] rows, each with the next range of the
//! particle pool ([`crate::CORONA_MAX_PARTICLES`]) of the size it requests
//! (rounded up to [`crate::CORONA_RANGE_ALIGNMENT`]), a prefix sum on the
//! GPU. A range that does not fit is clamped to what is left of the pool.
//! Each row carries its emitter's identity (its source row + 1, in the
//! `spawn_cursor` word): the pass restarts a row's particles when another
//! emitter takes it.
use helio_scene_join::{placement::*, BufferKey, Placement, RowJoin, RowJoinKeys, RowTable};

/// The owner's transform and the next range of the particle pool
/// (`rows_emitters.wgsl`).
const EMITTER_PLACEMENT: Placement =
    Placement::new("corona_emitters", include_str!("rows_emitters.wgsl")).from_owner();

/// `CoronaEmitterSourceRow`: the emitter row (60 words) with its transform
/// and particle offset left to the join and the requested particles in
/// `particle_count`.
pub const CORONA_EMITTER_SOURCE_ROW_BYTES: u64 = 60 * 4;
/// `particle_count`: the requested range in the source, the allocated one
/// out (`row_join.wgsl`'s `EMITTER_COUNT_WORD`).
const CORONA_COUNT_WORD: u32 = 46;

pub const CORONA_EMITTERS_KEY: BufferKey = BufferKey::of("corona_emitters");

/// Particle emitters' row join, from the emitter rows at `emitters`.
pub fn scene_rows(device: &wgpu::Device, keys: RowJoinKeys, emitters: BufferKey) -> RowJoin {
    RowJoin::new(
        device,
        "CoronaRows",
        keys,
        vec![RowTable::new(
            "Corona Rows Emitters",
            CORONA_EMITTERS_KEY,
            emitters,
            CORONA_EMITTER_SOURCE_ROW_BYTES,
            60,
            GATE_HIDDEN,
        )
        .placed_by(EMITTER_PLACEMENT)
        .packed(crate::CORONA_MAX_EMITTERS)
        .gated_on(CORONA_COUNT_WORD)
        .pool(crate::CORONA_MAX_PARTICLES)],
    )
}
