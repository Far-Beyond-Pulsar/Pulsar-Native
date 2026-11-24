//! The roots of the shared-library SDK: nothing here, the dependencies are the point.
//!
//! A library release cuts its SDK with `cargo sdk pack --package sdk_roots -- --release -p sdk_roots`, and a
//! consumer's build then finds every crate inside the libraries already compiled (`.agents/CI.md`).
