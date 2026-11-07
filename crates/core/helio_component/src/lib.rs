//! Pulsar's shared rendering-side engine pieces: the generic asset seams
//! (which component an asset becomes, who loads an asset kind, what a
//! changed file reloads), images (texture assets, cooking, residency in a
//! scene's texture store), cameras, movability and its motion gate, ground
//! material names, reflected lists and sublevel displays.
//!
//! No object kind's component lives here: every kind is its plugin's
//! (meshes: `meshes_component`, materials: `materials_component`, ...).

pub mod asset_component;
pub mod components;
pub mod cooked_texture;
mod motion_gate;
pub mod skinning;
mod static_move_watch;
pub mod subsystems;
pub mod texture_assets;
pub mod texture_detail;
pub use static_move_watch::StaticMoveWatch;

pub use asset_component::*;
pub use components::*;
