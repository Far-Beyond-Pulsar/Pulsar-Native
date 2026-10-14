//! Regression evals for the agent runtime.
//!
//! Run with `cargo test -p agent_runtime --test evals`. Every eval drives the
//! real loop or projector with `agent_runtime::testing::MockProvider`; see
//! README.md in this directory for how to add one.

mod budget;
mod common;
mod determinism;
mod pins;
mod report_roles;
mod steward_off;
mod tainted_writes;
mod time_travel;
