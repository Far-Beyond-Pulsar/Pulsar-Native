//! GPU-facing row ABI for Pulsar's Cables plugin.
//!
//! This crate owns contracts only. It has no registry or runtime state.

use bytemuck::{Pod, Zeroable};

/// Authored cable configuration mirrored for the GPU matcher/simulator.
/// `mode` is 0 for 2D ribbons and 1 for 3D cable geometry.
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

/// Connector authoring row. The owner transform is joined separately from
/// the engine's shared transform/owner contracts.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct CableConnectorRow {
    /// Replaced by the owner placement join before the feature consumes it.
    pub transform: [f32; 16],
    pub id: u32,
    pub enabled: u32,
    pub _padding: [u32; 2],
}

/// Byte size of the placed connector row. Kept beside the row ABI so the
/// feature and its source join cannot silently disagree about the stride.
pub const CABLE_CONNECTOR_ROW_BYTES: u64 = std::mem::size_of::<CableConnectorRow>() as u64;

/// GPU buffer contract for cable definitions.
pub const CABLE_DEFINITIONS: helio_core::BufferContract<CableDefinitionRow> =
    helio_core::BufferContract::new("cable_definitions");

/// GPU buffer contract for cable endpoints.
pub const CABLE_CONNECTORS: helio_core::BufferContract<CableConnectorRow> =
    helio_core::BufferContract::new("cable_connectors");

/// GPU buffer contract for expanded cable pairs/strands.
pub const CABLE_STRANDS: helio_core::BufferContract<CableStrandRow> =
    helio_core::BufferContract::new("cable_strands");

/// Matched endpoint pair and resolved GPU rendering parameters. Produced by
/// the feature's GPU pair expansion pass, never uploaded from a CPU pairing
/// loop.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct CableStrandRow {
    pub endpoint_a: u32,
    pub endpoint_b: u32,
    pub definition: u32,
    pub reset_revision: u32,
    pub config: [f32; 4],
    pub wind: [f32; 4],
}
