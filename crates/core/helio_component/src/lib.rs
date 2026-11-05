//! Rendering components for Pulsar Engine
//!
//! This crate provides rendering-related components that integrate with the
//! engine's reflection system for automatic UI generation.

pub mod anim_source;
pub mod asset_component;
pub mod components;
pub mod cooked_texture;
pub mod mesh_cache;
pub mod mesh_materials;
pub mod mesh_spatial;
pub mod mesh_thumbnail;
mod motion_gate;
mod static_move_watch;
pub mod texture_assets;
pub mod texture_detail;
pub use static_move_watch::StaticMoveWatch;
pub mod subsystems;
pub mod virtual_geometry_host;
pub mod virtual_geometry_instances;

pub use asset_component::*;
pub use components::*;
