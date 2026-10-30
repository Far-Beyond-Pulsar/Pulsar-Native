//! Which object each local player's view follows.
//!
//! A **view target** is the entity a player looks through: when it (or an
//! object beneath it) carries an enabled `CameraComponent`, that camera is the
//! frame's camera ([`camera_selection::select_world_camera`](crate::camera_selection::select_world_camera)).
//! Possession sets it to the possessed pawn (`ScriptDriver::advance_possession`)
//! and clears it when the possession ends; gameplay can retarget it later
//! (`ScriptDriver::set_view_target`, or the `world::set_view_target` script
//! function), for example to a cutscene camera or back to the pawn.
//!
//! The state is a small shared handle owned by the [`TickLoop`](crate::tick::TickLoop)
//! (`view_targets()`), written by the script driver on the tick thread and read
//! by the hosts' camera selection on theirs. It is keyed by player so a second
//! local player is a later addition, not a rewrite; today only
//! [`LOCAL_PLAYER`] exists and the hosts read that one.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use pulsar_scenedb::Entity;

/// A player's id: the one the session assigns when it starts them.
pub type PlayerId = i64;

/// The one local player a session starts today.
pub const LOCAL_PLAYER: PlayerId = 0;

/// The entity each player's view follows. Clones share one map.
#[derive(Clone, Debug, Default)]
pub struct ViewTargets(Arc<Mutex<BTreeMap<PlayerId, Entity>>>);

impl ViewTargets {
    pub fn new() -> Self {
        Self::default()
    }

    fn map(&self) -> std::sync::MutexGuard<'_, BTreeMap<PlayerId, Entity>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The entity `player` looks through, if one is set.
    pub fn get(&self, player: PlayerId) -> Option<Entity> {
        self.map().get(&player).copied()
    }

    /// The local player's view target.
    pub fn local(&self) -> Option<Entity> {
        self.get(LOCAL_PLAYER)
    }

    pub(crate) fn set(&self, player: PlayerId, target: Entity) {
        self.map().insert(player, target);
    }

    pub(crate) fn clear(&self, player: PlayerId) {
        self.map().remove(&player);
    }

    /// Nobody has a target: the session ended.
    pub(crate) fn clear_all(&self) {
        self.map().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_the_map_and_players_are_separate() {
        let targets = ViewTargets::new();
        let seen_elsewhere = targets.clone();
        let (a, b) = (Entity::from_bits(1), Entity::from_bits(2));
        assert_eq!(seen_elsewhere.local(), None);
        targets.set(LOCAL_PLAYER, a);
        targets.set(1, b);
        assert_eq!(seen_elsewhere.local(), Some(a));
        assert_eq!(seen_elsewhere.get(1), Some(b));
        targets.clear(LOCAL_PLAYER);
        assert_eq!(seen_elsewhere.local(), None);
        assert_eq!(seen_elsewhere.get(1), Some(b));
        targets.clear_all();
        assert_eq!(seen_elsewhere.get(1), None);
    }
}
