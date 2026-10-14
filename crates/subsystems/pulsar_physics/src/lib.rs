//! Physics components for Pulsar Engine
//!
//! This crate provides physics-related components that integrate with the
//! engine's reflection system for automatic UI generation.

pub mod components;
pub mod skeletal;

pub use components::*;
pub use skeletal::*;

#[cfg(test)]
mod golden_metadata;
