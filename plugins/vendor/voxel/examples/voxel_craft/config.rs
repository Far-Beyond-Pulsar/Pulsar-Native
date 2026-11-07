//! Tunables of the example: the planet, the player and what streams around them.

/// Planet radius (m). Small enough to circle in a few hours on foot, large
/// enough that the horizon visibly curves.
pub const PLANET_RADIUS_M: f64 = 16_000.0;
/// The world's voxels (m): small enough for grass, bark and pebbles.
pub const VOXEL_M: f64 = 0.1;
/// A built block's side (m): building lays cubes of this on a lattice of
/// it.
pub const BLOCK_M: f64 = 0.5;
/// The radius of what one dig takes out (m).
pub const DIG_RADIUS_M: f64 = 0.35;
/// The highest ledge walking steps up (m).
pub const STEP_M: f64 = 0.55;
/// Seed of the terrain and of every placed feature.
pub const WORLD_SEED: u64 = 7;

/// Walking, running and flying speeds (m/s).
pub const WALK_SPEED: f64 = 4.3;
pub const SPRINT_SPEED: f64 = 6.6;
pub const FLY_SPEED: f64 = 14.0;
pub const FLY_FAST_SPEED: f64 = 60.0;
/// Gravity (m/s^2) and the jump's launch speed (about 1.3 m high).
pub const GRAVITY: f64 = 30.0;
pub const JUMP_SPEED: f64 = 8.8;
/// The player's collision box: width and height (m), and the eye above feet.
pub const PLAYER_WIDTH: f64 = 0.6;
pub const PLAYER_HEIGHT: f64 = 1.8;
pub const EYE_HEIGHT: f64 = 1.62;
/// How far the player reaches to dig and build (m).
pub const REACH: f64 = 6.0;
/// Third person camera distance behind the character (m).
pub const THIRD_PERSON_DISTANCE: f64 = 4.5;

/// A day's length in seconds of play.
pub const DAY_SECONDS: f64 = 600.0;
