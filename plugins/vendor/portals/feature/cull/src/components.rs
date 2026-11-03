//! SceneDB-owned portal GPU projections.

/// SceneDB-authored peer reference for a portal.
///
/// `Entity` is a packed generational handle, but it is not itself a GPU/POD
/// type. Keeping its raw bits in the mirrored row preserves the authoritative
/// peer relationship and lets the future resolver reconstruct the validated
/// SceneDB handle before generating portal-view and chain projections.
pub const NO_PORTAL_PEER: u64 = u64::MAX;

/// One derived portal chain: indices into the portal-view rows, outermost
/// first. Variable length: engines store it in the
/// [`crate::PORTAL_CHAIN_PORTAL_POOL_BUFFER`] pool and its
/// [`crate::PORTAL_CHAIN_HANDLE_BUFFER`] handle table (Helio's standalone
/// hosts use `helio_components::PortalChainComponent`); the passes bind both.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortalChain {
    pub portals: Vec<u32>,
}

/// Authored portal relationship and local aperture data.
///
/// This is intentionally distinct from [`PortalViewRow`] and
/// [`PortalChain`]. Those rows are derived GPU projections consumed by
/// the current render passes; this row is the persistent SceneDB-facing source
/// of truth. A portal can point to a peer in the same sublevel, another
/// sublevel, or a nested actor context without storing a sublevel index of its
/// own.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct PortalRow {
    pub peer: u64,
    pub flags: u32,
    pub _pad: u32,
    pub transform: [[f32; 4]; 4],
    pub half_extent: [f32; 2],
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for PortalRow {}

/// The buffer the renderer reads [`PortalRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const PORTALS: helio_core::BufferContract<PortalRow> =
    helio_core::BufferContract::new("portals");

impl PortalRow {
    /// Bit in [`Self::flags`] that enables this authored portal.
    pub const FLAG_ENABLED: u32 = 1 << 0;

    pub fn new(
        peer: Option<pulsar_scenedb::Entity>,
        transform: glam::Mat4,
        half_extent: [f32; 2],
    ) -> Self {
        Self {
            peer: peer.map_or(NO_PORTAL_PEER, pulsar_scenedb::Entity::bits),
            flags: Self::FLAG_ENABLED,
            _pad: 0,
            transform: transform.to_cols_array_2d(),
            half_extent,
        }
    }

    pub fn peer_entity(&self) -> Option<pulsar_scenedb::Entity> {
        (self.peer != NO_PORTAL_PEER).then(|| pulsar_scenedb::Entity::from_bits(self.peer))
    }

    pub fn transform(&self) -> glam::Mat4 {
        glam::Mat4::from_cols_array_2d(&self.transform)
    }

    pub fn is_enabled(&self) -> bool {
        self.flags & Self::FLAG_ENABLED != 0
    }
}

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct PortalViewRow {
    pub transform: [f32; 16],
    pub inverse_transform: [f32; 16],
    pub half_extent: [f32; 2],
    pub coordinate_space: u32,
    /// Reserved compatibility word. Zero means the row is a physical portal
    /// for legacy manually-authored rows. The projection bridge sets
    /// [`Self::FLAG_MASK_HIDDEN`] on virtual nested rows so they can still be
    /// referenced by a chain without being stamped as another opening.
    pub _pad: u32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for PortalViewRow {}

/// The buffer the renderer reads [`PortalViewRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const PORTAL_VIEWS: helio_core::BufferContract<PortalViewRow> =
    helio_core::BufferContract::new("portal_views");

impl PortalViewRow {
    /// The row is a nested projection-only portal surface, not a physical
    /// opening that should contribute to the screen-space portal mask.
    pub const FLAG_MASK_HIDDEN: u32 = 1 << 0;
}

/// Derived frame counts for the portal projection tables.
///
/// The authored [`PortalRow`] remains the source of truth. This row is
/// published by the projection bridge alongside the dense derived view and
/// chain rows so a renderer integration can distinguish active rows from a
/// growable SceneDB buffer's reserved capacity without changing the authored
/// data model.
#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct PortalProjectionCountsRow {
    pub portal_view_count: u32,
    pub portal_chain_count: u32,
    pub coordinate_space_count: u32,
    pub _pad: u32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for PortalProjectionCountsRow {}

/// The buffer the renderer reads [`PortalProjectionCountsRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const PORTAL_PROJECTION_COUNTS: helio_core::BufferContract<PortalProjectionCountsRow> =
    helio_core::BufferContract::new("portal_projection_counts");

impl From<crate::GpuPortalView> for PortalViewRow {
    fn from(v: crate::GpuPortalView) -> Self {
        bytemuck::cast(v)
    }
}
impl From<PortalViewRow> for crate::GpuPortalView {
    fn from(v: PortalViewRow) -> Self {
        bytemuck::cast(v)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;
    #[test]
    fn scene_records_preserve_gpu_abi() {
        assert_eq!(std::mem::size_of::<PortalRow>(), 88);
        assert_eq!(std::mem::size_of::<PortalViewRow>(), 144);
        assert_eq!(std::mem::size_of::<PortalProjectionCountsRow>(), 16);
    }

    #[test]
    fn authored_portal_round_trips_peer_handle() {
        let mut world = pulsar_scenedb::World::new();
        let peer = world.spawn();
        let authored = PortalRow::new(Some(peer), glam::Mat4::IDENTITY, [1.0, 2.0]);
        assert_eq!(authored.peer_entity(), Some(peer));
        assert_eq!(authored.half_extent, [1.0, 2.0]);
    }
}
