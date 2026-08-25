//! The single bridge between the two `GameTime` types.
//!
//! `pulsar_core::GameTime` and [`pulsar_scenedb::GameTime`] are
//! structurally-identical but distinct types by design, not an accident to
//! paper over: SceneDB lives in its own repo and deliberately does not depend
//! on pulsar_core (it is a standalone storage layer), while pulsar_core
//! predates the extraction and owns the gameplay-facing type. Neither crate
//! can implement `From` across the boundary (orphan rule).
//!
//! THIS module is the one deliberate seam between them
//! (Pulsar-Native#652). New call sites must convert here — never re-inline a
//! field copy, and never widen this module with anything beyond that one
//! conversion. If a third spelling of frame time ever appears, extend this
//! module rather than inventing a second seam.

use pulsar_core::GameTime;

/// Convert a gameplay-facing [`pulsar_core::GameTime`] snapshot into the
/// data-layer equivalent expected by `pulsar_scenedb`'s `Schedule`/systems.
///
/// Field-by-field identity: `elapsed`, `delta`, and `tick` carry over
/// unchanged; no scaling or clamping happens at the boundary.
pub fn to_scenedb_time(time: GameTime) -> pulsar_scenedb::GameTime {
    pulsar_scenedb::GameTime {
        elapsed: time.elapsed,
        delta: time.delta,
        tick: time.tick,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// #652: the bridge maps every field verbatim — it is a type-system
    /// seam, not a transformation.
    #[test]
    fn converts_every_field_verbatim() {
        let core = GameTime {
            elapsed: Duration::from_secs_f32(12.5),
            delta: Duration::from_millis(16),
            tick: 781,
        };
        let scenedb = to_scenedb_time(core);
        assert_eq!(scenedb.elapsed, core.elapsed);
        assert_eq!(scenedb.delta, core.delta);
        assert_eq!(scenedb.tick, core.tick);
    }

    /// Zero-time edge case stays zero in all fields.
    #[test]
    fn converts_zero_time() {
        let scenedb = to_scenedb_time(GameTime {
            elapsed: Duration::ZERO,
            delta: Duration::ZERO,
            tick: 0,
        });
        assert_eq!(scenedb.elapsed, Duration::ZERO);
        assert_eq!(scenedb.delta, Duration::ZERO);
        assert_eq!(scenedb.tick, 0);
    }
}
