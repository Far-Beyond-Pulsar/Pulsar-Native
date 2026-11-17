//! Contracts for Pulsar's Cables plugin.
//!
//! This crate owns contracts only. It has no registry or runtime state.
//!
//! Two things cross from the cable component to the cable feature:
//!
//! - the authored cable definitions, as GPU rows ([`CABLE_DEFINITIONS`]): the
//!   appearance and simulation parameters, and the scene texture slot;
//! - the frame's spans ([`CABLE_SPANS`]): which two connector positions each
//!   cable joins. Connectors sharing an ID are paired on the CPU each frame,
//!   as portals pair by Link ID, and published as one payload.

use bytemuck::{Pod, Zeroable};

/// Authored cable configuration mirrored for the GPU. `mode` is 0 for 2D
/// ribbons and 1 for 3D cable geometry.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct CableDefinitionRow {
    pub id: u32,
    pub enabled: u32,
    pub mode: u32,
    pub bundle_count: u32,
    pub radius: f32,
    pub bundle_spacing: f32,
    pub droop: f32,
    pub wind_strength: f32,
    pub wind_direction_speed: [f32; 4],
    pub reset_revision: u32,
    pub texture_slot: u32,
    pub color_rgba8: u32,
    pub _padding: u32,
}

/// GPU buffer contract for the enabled cable definitions, packed into the
/// leading rows.
pub const CABLE_DEFINITIONS: helio_core::BufferContract<CableDefinitionRow> =
    helio_core::BufferContract::new("cable_definitions");

/// The most bundle lanes one tube cable draws.
pub const MAX_BUNDLE_LANES: u32 = 64;

/// One cable: the two connectors sharing `id`, in the frame's space (the
/// world position minus the frame's origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CableSpan {
    pub id: u32,
    pub from: [f32; 3],
    pub to: [f32; 3],
    /// The definition's render mode: a 3D tube (with `lanes` strands) or a
    /// camera-facing ribbon (one lane).
    pub tube: bool,
    /// Parallel strands, `1..=MAX_BUNDLE_LANES`.
    pub lanes: u32,
}

/// The frame's cables ([`CABLE_SPANS`]): every pair of connectors that share
/// the ID of an enabled definition.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CableSpans(pub Vec<CableSpan>);

impl CableSpans {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The spans, published by the host's cable producer every frame. Without
/// them nothing is drawn.
pub const CABLE_SPANS: helio_core::PayloadContract<CableSpans> =
    helio_core::PayloadContract::new("cable_spans");
