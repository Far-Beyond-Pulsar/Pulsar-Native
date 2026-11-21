//! The mocap editor (`pulsar.mocap`): a live capture session and the review of
//! `.mocap` takes in one dock workspace. Design: `.agents/MOCAP.md`.
//!
//! Panels: camera viewport (the video on a WGPU surface with HUD cards on a
//! panel canvas), the fused scene, sources, timeline (virtualized lanes) and inspector,
//! all over one [`SessionModel`].

mod blit;
mod curves;
mod diagrams;
mod editor;
mod model;
mod paint;
mod panels;
mod provider;

pub use blit::{fit_rect, grid, tile_rect, FrameBlit};
pub use curves::{curve_polylines, tick_step, value_range, CurveSpec};
pub use editor::MocapEditor;
pub use model::{sample_take, CameraChoice, NewSource, SessionModel};
pub use provider::{MocapBuiltinProvider, PROVIDER_ID};
