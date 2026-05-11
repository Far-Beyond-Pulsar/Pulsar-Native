//! # blueprint_compiler
//!
//! Thin re-export wrapper around PBGC.  All logic lives in the `pbgc` crate.
//! Import from here for engine-side code; the public API is identical.

pub use pbgc::*;
