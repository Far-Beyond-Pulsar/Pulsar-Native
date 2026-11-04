//! The sky: the authored atmosphere (Hillaire 2020) with its clouds, and the
//! passes that render it.
//!
//! [`AtmospherePass`] resolves the first enabled [`AtmosphereRow`] row
//! and the first directional light into the atmosphere LUTs and frame that
//! lighting reads, and the clouds' noise and shadow; [`CloudTracePass`]
//! marches the clouds seen from the camera; [`AtmosphereCompositePass`]
//! draws the sky where nothing was drawn, aerial perspective over geometry
//! and the clouds in front of both. Without an atmosphere row none draws
//! anything, and the sky stays the graph's black background.
//!
//! A scene has one sky, its atmosphere; its clouds are the row's cloud layer
//! ([`CloudLayer`], the [`clouds`] module).

pub mod atmosphere;
pub mod clouds;
mod feature;
#[cfg(feature = "host-rows")]
pub mod host_rows;
pub mod scene_rows;
pub use atmosphere::ATMOSPHERES;
pub use atmosphere::{
    AtmosphereCompositePass, AtmospherePass, AtmosphereRow, CloudLayer, CloudRepresentation,
    PlanetWeather,
};
pub use clouds::CloudTracePass;
pub use feature::SkyFeature;
