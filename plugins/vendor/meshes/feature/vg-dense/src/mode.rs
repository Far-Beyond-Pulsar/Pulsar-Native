//! Tier-3 backend selection: hardware gating and the `VgMode` policy.

use helio_vg_format::VgMode;

/// Whether `device` can run the 3-RT path.
///
/// `EXPERIMENTAL_RAY_QUERY` is only ever present on a device created through
/// `helio::renderer::config::required_wgpu_features(usable_adapter_features(..))`,
/// and `usable_adapter_features` strips it on adapters whose shader compiler
/// crashes on ray-query pipelines (software Vulkan, Pulsar-Native#840). Checking
/// the *device's* features therefore inherits that filter without this crate
/// depending on the `helio` crate. The acceleration-structure limits must also
/// be nonzero (they are zero unless the application requested them).
pub fn rt_available(device: &wgpu::Device) -> bool {
    let limits = device.limits();
    device
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY)
        && limits.max_tlas_instance_count > 0
        && limits.max_blas_geometry_count > 0
        && limits.max_blas_primitive_count > 0
}

/// Which implementation resolves dense tiles this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier3Backend {
    /// Tier 3 is not used (mode `Off` / `ForceHwRaster`).
    Off,
    /// Tiled depth peel. Runs everywhere.
    NonRt,
    /// Ray-query over proxy-LOD BLASes.
    Rt,
}

/// Tunables of the `Auto` policy.
#[derive(Clone, Copy, Debug)]
pub struct AutoPolicy {
    /// Ray tracing needs at least this many dense tiles to pay for the TLAS
    /// refresh and proxy BLAS maintenance.
    pub min_dense_tiles_for_rt: u32,
    /// ...or at least this fraction of all tiles.
    pub min_dense_fraction_for_rt: f32,
    /// Once on RT, stay until the count falls below `threshold * hysteresis`
    /// (stops flicker around the threshold).
    pub hysteresis: f32,
}

impl AutoPolicy {
    /// Policy of the render graph: RT traces only the dense-tile list (the rest
    /// of the frame stays on Tier 1/2), so it pays off as soon as a few clusters
    /// of dense tiles exist; tuned on a forest view with dozens of dense tiles
    /// per tree (hundreds of thousands of dense clusters), where the depth peel
    /// overflows its per-tile lists.
    pub fn dense_regions() -> Self {
        Self {
            min_dense_tiles_for_rt: 16,
            min_dense_fraction_for_rt: 0.004,
            hysteresis: 0.5,
        }
    }
}

impl Default for AutoPolicy {
    fn default() -> Self {
        Self {
            min_dense_tiles_for_rt: 256,
            min_dense_fraction_for_rt: 0.05,
            hysteresis: 0.5,
        }
    }
}

/// Stateful selector (the state is only the hysteresis bit).
#[derive(Debug, Default)]
pub struct Tier3Selector {
    pub policy: AutoPolicy,
    on_rt: bool,
}

impl Tier3Selector {
    pub fn new(policy: AutoPolicy) -> Self {
        Self {
            policy,
            on_rt: false,
        }
    }

    /// Choose the backend for a frame.
    ///
    /// * `Off`, `ForceHwRaster` -> `Off` (those modes bypass Tier 3).
    /// * `ForceNonRtFallback` -> `NonRt`, never RT even if available.
    /// * `Auto` -> `Rt` only when `rt_available`, the proxy BLASes are ready
    ///   (`rt_ready`) and the dense tile count justifies it; otherwise `NonRt`.
    pub fn select(
        &mut self,
        mode: VgMode,
        rt_available: bool,
        rt_ready: bool,
        dense_tiles: u32,
        total_tiles: u32,
    ) -> Tier3Backend {
        match mode {
            VgMode::Off | VgMode::ForceHwRaster => {
                self.on_rt = false;
                Tier3Backend::Off
            }
            VgMode::ForceNonRtFallback => {
                self.on_rt = false;
                Tier3Backend::NonRt
            }
            VgMode::Auto => {
                if !(rt_available && rt_ready) {
                    self.on_rt = false;
                    return Tier3Backend::NonRt;
                }
                let p = &self.policy;
                let by_frac = (total_tiles as f32 * p.min_dense_fraction_for_rt).ceil() as u32;
                let threshold = p.min_dense_tiles_for_rt.min(by_frac.max(1)).max(1);
                let threshold = if self.on_rt {
                    ((threshold as f32) * p.hysteresis).ceil() as u32
                } else {
                    threshold
                };
                self.on_rt = dense_tiles >= threshold.max(1);
                if self.on_rt {
                    Tier3Backend::Rt
                } else {
                    Tier3Backend::NonRt
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_modes_never_use_rt() {
        let mut s = Tier3Selector::default();
        assert_eq!(
            s.select(VgMode::ForceNonRtFallback, true, true, 10_000, 32_000),
            Tier3Backend::NonRt
        );
        assert_eq!(
            s.select(VgMode::ForceHwRaster, true, true, 10_000, 32_000),
            Tier3Backend::Off
        );
        assert_eq!(
            s.select(VgMode::Off, true, true, 10_000, 32_000),
            Tier3Backend::Off
        );
    }

    #[test]
    fn auto_requires_hardware_readiness_and_enough_tiles() {
        let mut s = Tier3Selector::default();
        assert_eq!(
            s.select(VgMode::Auto, false, true, 10_000, 32_000),
            Tier3Backend::NonRt
        );
        assert_eq!(
            s.select(VgMode::Auto, true, false, 10_000, 32_000),
            Tier3Backend::NonRt
        );
        assert_eq!(
            s.select(VgMode::Auto, true, true, 3, 32_000),
            Tier3Backend::NonRt
        );
        assert_eq!(
            s.select(VgMode::Auto, true, true, 256, 32_000),
            Tier3Backend::Rt
        );
    }

    #[test]
    fn auto_has_hysteresis() {
        let mut s = Tier3Selector::default();
        assert_eq!(
            s.select(VgMode::Auto, true, true, 300, 32_000),
            Tier3Backend::Rt
        );
        // Below the entry threshold but above the exit one: stays on RT.
        assert_eq!(
            s.select(VgMode::Auto, true, true, 200, 32_000),
            Tier3Backend::Rt
        );
        assert_eq!(
            s.select(VgMode::Auto, true, true, 100, 32_000),
            Tier3Backend::NonRt
        );
        // And needs the full threshold to come back.
        assert_eq!(
            s.select(VgMode::Auto, true, true, 200, 32_000),
            Tier3Backend::NonRt
        );
    }

    #[test]
    fn small_screens_scale_the_threshold() {
        let mut s = Tier3Selector::default();
        // 400 tiles total: 5% = 20 dense tiles is enough.
        assert_eq!(
            s.select(VgMode::Auto, true, true, 20, 400),
            Tier3Backend::Rt
        );
    }
}
