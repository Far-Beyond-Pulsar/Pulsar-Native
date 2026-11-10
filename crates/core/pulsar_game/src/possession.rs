//! Whether the local player possesses a pawn.
//!
//! [`PossessionGate`] is the one piece of possession state the rest of the
//! runtime reads: whether a possessed pawn exists, which is what keeps the
//! built-in free camera off the keys the pawn is listening to. What a
//! possession puts on the pawn (the input plugin's mapping) is the plugins'
//! (`engine_backend::scene::game_hooks::GameSessionRegistration`), run by
//! the script driver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Whether the local player's controller has a possessed pawn.
///
/// Owned by the [`TickLoop`](crate::tick::TickLoop), set by its script driver
/// as possession begins and ends, and cloned to whoever must act differently
/// while a game owns the player's input (the built-in [`FreeCam`](crate::freecam::FreeCam)
/// in the windowed and Play-in-Editor hosts, which run on other threads than
/// the tick loop). Clones share one flag.
#[derive(Clone, Debug, Default)]
pub struct PossessionGate(Arc<AtomicBool>);

impl PossessionGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// A pawn is possessed: the game owns the player's input.
    pub fn is_possessed(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn set(&self, possessed: bool) {
        self.0.store(possessed, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_of_the_gate_share_one_flag() {
        let gate = PossessionGate::new();
        let seen_elsewhere = gate.clone();
        assert!(!gate.is_possessed());
        gate.set(true);
        assert!(seen_elsewhere.is_possessed());
        gate.set(false);
        assert!(!seen_elsewhere.is_possessed());
    }
}
