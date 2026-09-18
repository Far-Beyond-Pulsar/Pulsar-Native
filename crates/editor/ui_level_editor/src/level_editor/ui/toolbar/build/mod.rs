//! Build & Run family.
//!
//! Everything that compiles and runs the game from the toolbar lives here:
//! the Build Core split-button, the build config/target dropdowns, and the
//! cargo progress cell / runner they all share.

pub mod build_core;
pub mod build_dropdowns;
pub mod cargo_progress;