//! The look the frame applies late.
//!
//! The camera shows what the last simulation tick wrote, so a mouse movement
//! that arrives after that tick (up to a tick, a few milliseconds) is not in the
//! picture. The standard remedy (Unreal's late camera update, the VR runtimes'
//! late-latched head pose) is to read the newest input at the last moment before
//! the camera goes into the frame's uniforms and turn the view by it, as a
//! rotation over the simulated pose.
//!
//! Here the frame never writes the world. It asks the plugins what the motion
//! no tick has consumed yet will turn the view by
//! ([`engine_backend::scene::game_hooks::pending_view_turn`]: the input
//! plugin says what value the look action takes for that motion, the character
//! plugin what that value turns its control rotation by) and gives the
//! camera selection that rotation ([`crate::camera_selection::turned_rig`]).
//! The next tick takes the same motion from the sampler and applies it through
//! its own look path, by the same rules, so the world reaches the view the frame
//! showed and the motion is counted once: [`PointerSampler`]'s totals and the
//! tick's sequence number make "not consumed yet" exact (see its docs).

use crate::pointer::PointerSampler;
use std::time::Duration;

/// How long the frame waits for a running tick to end before it shows the
/// tick's view as it is. The tick costs a fraction of a millisecond.
const TICK_WAIT: Duration = Duration::from_millis(2);
/// Reads of the world that a tick interrupted before the frame gives up.
const ATTEMPTS: usize = 3;

/// Read the world for a frame with the pointer motion no finished tick has
/// applied: `read` gets that motion (device counts, x right and y down; `None`
/// when no tick loop consumes the sampler, a tick would not end in time or kept
/// interrupting) and is run again if a tick ran while it read, so what it
/// returns pairs a world with exactly the motion that world lacks.
pub fn read_with_pending_motion<T>(
    pointer: &PointerSampler,
    mut read: impl FnMut(Option<[f32; 2]>) -> T,
) -> T {
    for _ in 0..ATTEMPTS {
        let Some(before) = pointer.wait_for_idle_tick(TICK_WAIT) else {
            break;
        };
        let motion = pointer.unapplied_motion();
        let value = read(motion);
        if pointer.tick_sequence() == before {
            return value;
        }
    }
    read(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured() -> std::sync::Arc<PointerSampler> {
        let sampler = PointerSampler::new();
        sampler.set_captured(true);
        sampler.attach_tick_consumer();
        sampler
    }

    fn add(sampler: &PointerSampler, dx: f64, dy: f64) {
        sampler.add_motion(dx, dy, crate::latency::stamp());
    }

    /// Motion is unapplied from the moment it arrives until the tick that took it
    /// ends, and counted by no frame after that: once, never twice, never lost.
    #[test]
    fn motion_is_pending_until_the_tick_that_took_it_ends() {
        let sampler = captured();
        assert_eq!(sampler.unapplied_motion(), Some([0.0, 0.0]));
        add(&sampler, 5.0, -2.0);
        assert_eq!(sampler.unapplied_motion(), Some([5.0, -2.0]));

        sampler.begin_tick();
        let sample = sampler.drain().expect("the tick takes it");
        assert_eq!((sample.dx, sample.dy), (5.0, -2.0));
        // Taken but not yet in the world: still pending, and a tick is running.
        assert_eq!(sampler.unapplied_motion(), Some([5.0, -2.0]));
        assert!(sampler.wait_for_idle_tick(Duration::from_micros(200)).is_none());
        add(&sampler, 1.0, 1.0);
        sampler.end_tick();

        // What arrived during the tick is pending, what the tick took is not.
        assert_eq!(sampler.unapplied_motion(), Some([1.0, 1.0]));
        assert_eq!(sampler.wait_for_idle_tick(Duration::ZERO), Some(sampler.tick_sequence()));
        sampler.begin_tick();
        let next = sampler.drain().unwrap();
        assert_eq!((next.dx, next.dy), (1.0, 1.0));
        sampler.end_tick();
        assert_eq!(sampler.unapplied_motion(), Some([0.0, 0.0]));
    }

    #[test]
    fn nothing_is_pending_without_a_tick_that_consumes_it() {
        let sampler = PointerSampler::new();
        sampler.set_captured(true);
        add(&sampler, 4.0, 4.0);
        assert_eq!(sampler.unapplied_motion(), None);
    }

    #[test]
    fn releasing_the_cursor_forgets_the_pending_motion() {
        let sampler = captured();
        add(&sampler, 4.0, 4.0);
        sampler.set_captured(false);
        sampler.set_captured(true);
        assert_eq!(sampler.unapplied_motion(), Some([0.0, 0.0]));
        assert!(sampler.drain().is_none());
    }

    /// A tick that runs while the frame reads the world makes the read void: the
    /// frame reads again, so the world and the pending motion it pairs are from
    /// the same side of the tick.
    #[test]
    fn a_read_a_tick_interrupted_is_done_again() {
        let sampler = captured();
        add(&sampler, 2.0, 0.0);
        let mut reads = Vec::new();
        let seen = read_with_pending_motion(&sampler, |motion| {
            reads.push(motion);
            if reads.len() == 1 {
                // The tick takes the motion and ends while the read is going on.
                sampler.begin_tick();
                sampler.drain();
                sampler.end_tick();
            }
            motion
        });
        assert_eq!(reads, [Some([2.0, 0.0]), Some([0.0, 0.0])]);
        assert_eq!(seen, Some([0.0, 0.0]));
    }

    /// A tick that never ends in time, or keeps interrupting, leaves the frame
    /// showing the tick's view with no late motion rather than a wrong one.
    #[test]
    fn a_frame_that_cannot_pair_a_read_shows_the_ticks_view() {
        let sampler = captured();
        add(&sampler, 2.0, 0.0);
        sampler.begin_tick();
        let seen = read_with_pending_motion(&sampler, |motion| motion);
        assert_eq!(seen, None);
        sampler.end_tick();

        let mut calls = 0;
        let seen = read_with_pending_motion(&sampler, |motion| {
            calls += 1;
            sampler.begin_tick();
            sampler.end_tick();
            motion
        });
        assert_eq!(calls, ATTEMPTS + 1);
        assert_eq!(seen, None);
    }
}
