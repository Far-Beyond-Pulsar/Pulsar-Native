//! The world the game sees: a voxel planet with edits applied off-thread,
//! read through the planet's query service (`WorldQueries`): bricks of
//! cells sampled by workers around the player, edits shown on them a few
//! milliseconds after they are made. Its decoration (trees, structures) is
//! the planet's features (`decorate`), part of its recipe.

pub mod decorate;
pub mod edits;
mod worker;

use crate::blocks::Block;
use edits::Edit;
use glam::DVec3;
use helio_pass_voxel_planet::queries::{Resident, WorldQueries};
use helio_pass_voxel_planet::{Brush, Cell, Matter, Planet, RayHit};
use std::sync::Arc;
use worker::{Job, Worker};

/// The query focus that follows the player.
const PLAYER: u64 = 1;
/// How far around the player the world is kept resident: its collision,
/// reach and third-person camera, with room to move.
pub const PLAYER_FOCUS_M: f64 = 10.0;

pub struct World {
    snapshot: Arc<Planet>,
    worker: Worker,
    queries: WorldQueries,
}

impl World {
    pub fn new(planet: Planet) -> Self {
        let snapshot = Arc::new(planet.clone());
        let threads = std::thread::available_parallelism()
            .map_or(2, |n| n.get().saturating_sub(2).clamp(1, 4));
        Self {
            queries: WorldQueries::new(snapshot.clone(), threads),
            snapshot,
            worker: Worker::spawn(planet),
        }
    }

    /// The latest finished planet, for drawing.
    pub fn planet(&self) -> &Arc<Planet> {
        &self.snapshot
    }

    /// Takes finished snapshots from the worker. Call once a frame.
    pub fn update(&mut self) {
        if let Some(planet) = self.worker.latest() {
            self.snapshot = planet;
            self.queries.publish(self.snapshot.clone());
        }
    }

    /// Keep the world around the player (at `position`) resident.
    pub fn follow(&self, position: DVec3) {
        self.queries.focus(PLAYER, position, PLAYER_FOCUS_M);
    }

    /// Whether the world within `radius` of `position` is resident (it is
    /// asked for when not).
    pub fn loaded_around(&self, position: DVec3, radius: f64) -> bool {
        self.queries.resident_within(position, radius)
    }

    /// Whether the whole world around the player is resident, and shows
    /// every edit made.
    pub fn loaded(&self) -> bool {
        self.queries.loading() == 0 && self.queries.editing() == 0
    }

    /// Wait until the world around the player is resident and shows every
    /// edit made (loading a game, tests).
    pub fn settle(&self) {
        while !self.loaded() {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// Applies `edits` (off-thread); queries see them in a few milliseconds.
    pub fn edit(&mut self, edits: &[Edit]) {
        let grid = *self.snapshot.grid();
        let brushes: Vec<_> = edits.iter().map(|e| e.brush(&grid)).collect();
        self.stamp(&brushes);
    }

    /// Applies `brushes` (off-thread); queries see them in a few milliseconds.
    pub fn stamp(&mut self, brushes: &[Brush]) {
        if brushes.is_empty() {
            return;
        }
        self.queries.edit(brushes);
        self.worker.submit(Job::Edit(brushes.to_vec()));
    }

    /// Keep the world within `radius` of `center` resident too (focus `id`,
    /// not the player's).
    pub fn focus_on(&self, id: u64, center: DVec3, radius: f64) {
        debug_assert_ne!(id, PLAYER);
        self.queries.focus(id, center, radius);
    }

    /// Whether `cell` is solid. Space not resident yet is: the player
    /// waits at its edge rather than falling into it.
    pub fn is_solid(&self, cell: Cell) -> bool {
        self.queries
            .solid(cell, Matter::All)
            .ready()
            .unwrap_or(true)
    }

    /// The block at `cell` (`None`: air, ground the game has no block for,
    /// or not resident yet).
    pub fn block_at(&self, cell: Cell) -> Option<Block> {
        match self.queries.material(cell) {
            Resident::Ready(m) if self.is_solid(cell) => Block::from_material(m),
            _ => None,
        }
    }

    /// The cell containing a planet-centred point.
    pub fn cell_at(&self, p: DVec3) -> Cell {
        self.snapshot.grid().locate(p).0
    }

    pub fn cell_centre(&self, cell: Cell) -> DVec3 {
        self.snapshot.grid().cell_center(cell)
    }

    /// The first solid cell along a ray, as edited, within the resident
    /// world (`None` past it).
    pub fn raycast(&self, origin: DVec3, direction: DVec3, max: f64) -> Option<RayHit> {
        self.queries
            .raycast(origin, direction, max, Matter::All)
            .ready()
            .flatten()
    }
}
