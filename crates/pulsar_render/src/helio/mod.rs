//! Helio / D3D12 backend utilities.
//!
//! This module contains Windows-specific helpers (DXGI shared texture creation,
//! HANDLE ↔ usize conversions) that are shared between `engine_backend` and
//! any future standalone Helio backend.
//!
//! # Migration status
//!
//! The main `HelioRenderer` struct lives in `engine_backend::subsystems::render::helio_renderer`
//! because its constructors take engine-internal types (`SceneDb`, `GameState`,
//! `PhysicsQueryService`).  Once those types are extracted to their own crates the
//! full implementation will be moved here.

pub mod handle_utils;

#[cfg(target_os = "windows")]
pub mod dxgi_shared_texture;
