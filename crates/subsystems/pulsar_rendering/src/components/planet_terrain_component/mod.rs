//! Scene component and render bridge for production planetary terrain.
//!
//! `pulsar_terrain` owns canonical terrain state and long-lived workers. This
//! component owns the scene-facing configuration and the only translation
//! boundary into Helio, matching the ownership pattern used by the other
//! rendering components.

mod component;
mod render_adapter;
mod runtime;

pub use component::{ComponentError, PLANET_TERRAIN_CLASS_NAME, PlanetTerrainComponent};
pub use render_adapter::*;
