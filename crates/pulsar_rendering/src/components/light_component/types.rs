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
