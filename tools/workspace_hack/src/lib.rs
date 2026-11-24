//! Pins the feature set of the crates inside the shared libraries. Generated crate: see `tools/hack_gen`.
//!
//! A crate that depends on this one gets, whatever else is in its build, the union of the features every
//! consumer (the editor and every plugin) asks of those crates. Cargo unifies features per build, and a
//! crate compiled with other features is another crate to rustc, so without this a build of the editor
//! alone yields shared libraries the plugins cannot link against. Nothing in this crate is used.
