//! Foliage quality presets.
//!
//! Mirrors the shape of the removed virtual-geometry pass's `LodQuality`: a four-variant enum
//! whose variants are the only platform difference in the whole foliage stack. Nothing
//! in the plan's platform matrix is `#[cfg]`-ed out — every pass compiles and runs
//! everywhere, and a mobile tier is a `FoliageQuality::Low` rather than a second code
//! path. That is a deliberate constraint: the moment a quality tier becomes a `cfg`, the
//! low tier stops being tested by anyone's desktop build.

/// Quality preset scaling the foliage's reach, density, LOD ladder and cull granularity.
///
/// The knobs trade against each other for a roughly fixed frame cost, but they are
/// separate methods so a caller with an unusual budget (a wide vista at low density, say)
/// can mix them. Reach costs little: the bands ([`crate::plan_bands`]) double it per band
/// at a constant price per band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FoliageQuality {
    /// Mobile and integrated tier: a short reach at a third of the density, in 8x8
    /// clusters.
    Low,
    /// The reference tier.
    #[default]
    Medium,
    High,
    /// Everything widened.
    Ultra,
}

impl FoliageQuality {
    /// Metres the foliage reaches by default (the last band ends here).
    pub fn range(self) -> f32 {
        match self {
            FoliageQuality::Low => 100.0,
            FoliageQuality::Medium => 200.0,
            FoliageQuality::High => 300.0,
            FoliageQuality::Ultra => 400.0,
        }
    }

    /// Multiplier applied to every foliage type's authored `density`.
    pub fn density_multiplier(self) -> f32 {
        match self {
            FoliageQuality::Low => 0.35,
            FoliageQuality::Medium => 1.0,
            FoliageQuality::High => 1.35,
            FoliageQuality::Ultra => 2.0,
        }
    }

    /// Multiplier applied to the near LOD distances (L0 → L1): pushing them out buys
    /// silhouette quality with raster cost.
    pub fn lod_distance_scale(self) -> f32 {
        match self {
            FoliageQuality::Low => 0.35,
            FoliageQuality::Medium => 1.0,
            FoliageQuality::High => 1.3,
            FoliageQuality::Ultra => 1.75,
        }
    }

    /// Records per cull cluster — the granularity of the second cull dispatch.
    ///
    /// A square power of two, never finer than 16 (halving the cluster doubles the cull
    /// dispatch to buy precision the Hi-Z test cannot resolve at that footprint) and at
    /// most 32 (the cull keeps a cluster's records in one 32-bit mask): 16 (4×4) at
    /// every quality.
    pub fn cluster_granularity(self) -> u32 {
        16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [FoliageQuality; 4] = [
        FoliageQuality::Low,
        FoliageQuality::Medium,
        FoliageQuality::High,
        FoliageQuality::Ultra,
    ];

    #[test]
    fn medium_is_the_default_reference_tier() {
        assert_eq!(FoliageQuality::default(), FoliageQuality::Medium);
        assert_eq!(FoliageQuality::Medium.density_multiplier(), 1.0);
        assert_eq!(FoliageQuality::Medium.lod_distance_scale(), 1.0);
    }

    #[test]
    fn presets_are_monotonic() {
        for pair in ALL.windows(2) {
            let (lower, higher) = (pair[0], pair[1]);
            assert!(
                higher.range() > lower.range(),
                "{higher:?} must reach further than {lower:?}"
            );
            assert!(higher.density_multiplier() > lower.density_multiplier());
            assert!(higher.lod_distance_scale() > lower.lod_distance_scale());
            assert!(higher.cluster_granularity() <= lower.cluster_granularity());
        }
    }

    #[test]
    fn clusters_are_square_and_fit_the_culls_mask() {
        for quality in ALL {
            let granularity = quality.cluster_granularity();
            let edge = (granularity as f64).sqrt() as u32;
            assert_eq!(
                edge * edge,
                granularity,
                "{quality:?} clusters are not square"
            );
            assert!(
                (16..=32).contains(&granularity),
                "{quality:?} granularity {granularity}"
            );
        }
    }
}
