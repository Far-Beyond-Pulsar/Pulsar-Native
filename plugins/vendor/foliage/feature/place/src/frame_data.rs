//! Views into the top-down foliage terrain capture.
//!
//! Produced by `FoliageTerrainPass` over the active foliage ring, consumed by
//! `FoliagePlacePass`, `FoliageInteractionPass` and the far-ring terrain-shading fallback.
//!
//! Both textures cover the same camera-relative, texel-snapped ring extent (default 256 m
//! at 4 texels/m) and must be sampled with the same transform. They are re-rendered only
//! for tiles whose residency or generation changed; the snap is what stops the capture
//! swimming under camera motion, so any consumer that derives its own unsnapped UVs
//! reintroduces exactly the shimmer the snapping exists to remove.
#[derive(Clone, Copy)]
pub struct FoliageTerrainViews<'a> {
    /// Terrain height (R) + slope (G) — `Rg16Float`.
    ///
    /// Slope is stored as `cos(angle)` so the placement shader tests a foliage type's
    /// `slope_range` acceptance band with two compares and no trig.
    pub height_slope: &'a wgpu::TextureView,
    /// Packed world normal (RGB) + material id (A) — `Rgba8Unorm`.
    ///
    /// The material id is what lets procedural density rules key off the surface
    /// (e.g. the voxel `MAT_GRASS` palette entry) without the placement shader knowing
    /// which terrain representation produced it.
    pub normal_material: &'a wgpu::TextureView,
}
