//! Primitive type implementations module
//!
//! Primitive registration is organized by feature-gated submodules.
//!
//! - `core`: language/core primitives and fixed-size numeric arrays
//! - `std`: standard library primitives (currently `String`)
//! - `serde`: serde-focused primitives (currently `serde_json::Value`)
//!
//! Primitive modules use `#[pulsar_type(primitive)]` aliases to generate
//! `Reflectable` implementations and inventory registration.

#[cfg(feature = "prims-core")]
pub mod core;

#[cfg(feature = "prims-std")]
pub mod std;

#[cfg(feature = "prims-serde")]
pub mod serde;

#[cfg(feature = "prims-glam")]
pub mod glam;

#[cfg(feature = "prims-helio")]
pub mod helio;
