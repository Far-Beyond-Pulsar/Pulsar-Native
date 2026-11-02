use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum IntensityUnits {
    Unitless,
    Lumens,
    Candelas,
    Lux,
    Nits,
}

impl Default for IntensityUnits {
    fn default() -> Self {
        Self::Lumens
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum MobileQualityLevel {
    Low,
    Medium,
    High,
    Epic,
}

impl Default for MobileQualityLevel {
    fn default() -> Self {
        Self::High
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum ShadowCacheMode {
    Auto,
    StaticOnly,
    DynamicOnly,
    Disabled,
}

impl Default for ShadowCacheMode {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum LightType {
    Directional,
    Point,
    Spot,
    Area,
}

impl Default for LightType {
    fn default() -> Self {
        Self::Point
    }
}

/// `#[gpu(as = u32, with = ...)]` target for `general.rs`'s `light_type`
/// field -- computed once at GPU-mirror-build time (`LightComponentGpuMirror
/// ::to_gpu_mirror`), not by a hand-written post-mirror translation
/// function. Deliberately NOT a raw discriminant cast: `helio::LightType`
/// has no `Area` variant (Helio doesn't support area lights), so this picks
/// the nearest equivalent (`Point`) -- a genuine semantic decision that has
/// to live in code somewhere regardless of where the bytes are stored.
///
/// This mapping is intentionally kept in Rust, not pushed into WGSL: the
/// shader files that branch on `light_type` (`forward_lit.wgsl`,
/// `deferred_lighting.wgsl`, and others) do exact-value/range checks
/// (`light_type > 0u && light_type < 2u`) that assume the value is 0, 1, or
/// 2 -- a raw `Area` discriminant reaching the GPU unmapped would silently
/// fall through every one of those checks. Remapping here, before upload,
/// is the safe place for it.
pub fn light_type_to_gpu_u32(kind: LightType) -> u32 {
    let helio_type = match kind {
        LightType::Directional => helio::LightType::Directional,
        LightType::Point => helio::LightType::Point,
        LightType::Spot => helio::LightType::Spot,
        LightType::Area => helio::LightType::Point, // helio has no Area; nearest equivalent
    };
    helio_type as u32
}
