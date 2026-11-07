//! Radiant — Helio's hybrid material template system.
//!
//! Radiant combines hand-authored template shaders with graph-generated WGSL
//! snippets to give artists flexibility without forcing PSO permutations for
//! every material.

mod graph_registry;
mod material_flags;
mod shader_cache;
pub mod template;

pub use graph_registry::{graph_source, register_graph_source, RadiantGraphRegistry};
pub use material_flags::*;
pub use shader_cache::*;
pub use template::*;
