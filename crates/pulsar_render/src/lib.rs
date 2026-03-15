//! Pulsar Render — renderer abstraction and shared types.
//!
//! This crate provides:
//! - [`Renderer`] trait: the engine-facing interface every backend must implement
//! - Shared data types used by all backends ([`types`])
//! - Optional Helio/D3D12 backend utilities ([`helio`], Windows-only, `helio` feature)

pub mod traits;
pub mod types;

#[cfg(feature = "helio")]
pub mod helio;

pub use traits::{Renderer, HeadlessRenderer};
pub use types::{
    RenderScene, RenderOutput, RenderMetrics, GpuTextureHandle,
    CameraInput, GpuProfilerData, DiagnosticMetric, SharedGpuTextures,
    Framebuffer,
};
