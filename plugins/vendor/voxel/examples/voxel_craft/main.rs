//! Voxel Craft: a sandbox on a voxel planet of 10 cm voxels.
//!
//! Walk, jump, fly, dig and build on a 16 km cube-sphere planet drawn by
//! Helio's voxel planet pass (`VoxelTerrainComponent`'s renderer): meadows,
//! deserts, mountains and caves; procedurally grown trees of shapes (oak,
//! birch, spruce, pine and jungle giants), bushes, boulders and cacti;
//! cottages, towers, ruins, wells and pyramids; a day-night cycle. Every
//! voxel's colour is procedural (its block's palette, varied voxel by voxel
//! and patch by patch): nothing is textured.
//!
//! ```text
//! cargo run -p helio-voxel-examples --release --bin voxel_craft
//! ```
//!
//! Controls: WASD move, Space jump (tap twice, or F, to fly), Ctrl sprint,
//! left mouse dig, right mouse build a half-metre block, middle mouse pick,
//! mouse wheel or 1-9 pick from the hotbar, V third-person camera, T speed
//! up time, R respawn, Esc release the mouse.
//! A connected OpenXR headset selects VR automatically: left stick moves,
//! right stick snap-turns/cycles blocks, triggers dig/build, A jumps, B flies,
//! X picks a block, Y recentres. See the README for all VR controls.
//!
//! Environment: `VOXEL_CRAFT_QUALITY` (performance|balanced|quality|native,
//! or fxaa: no TSR),
//! `VOXEL_CRAFT_LOD` (cells per pixel, 0.6), `VOXEL_CRAFT_DISTANCE` (view
//! distance in metres); `VOXEL_CRAFT_SHOTS=<dir>` runs the screenshot tour
//! (see the README).
//! `VOXEL_CRAFT_VR=off` forces desktop; `VOXEL_CRAFT_FRAMES=<n>` bounds a smoke run.
//!
//! Layout: `terrain/` the planet's recipe and biomes, `world/` edits, the
//! worker thread and decoration (trees and rocks grown by the engine's
//! `flora`), `structures/` the buildings,
//! `palette.rs` the blocks' look, `player/` movement, interaction and the
//! character, `gfx.rs` the renderer, `game.rs` one frame.

#[path = "../common.rs"]
mod common;

mod app;
mod blocks;
mod config;
mod game;
mod gfx;
mod input;
mod palette;
mod player;
mod strokes;
mod structures;
mod terrain;
mod world;
#[cfg(not(target_arch = "wasm32"))]
mod xr;

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,voxel_craft=info"),
    )
    .init();
    let event_loop = winit::event_loop::EventLoop::new().expect("event loop");
    let mut app = app::App::default();
    event_loop.run_app(&mut app).expect("run");
}
