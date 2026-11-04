//! Feature-flag helper functions for material flags.
//!
//! These are thin wrappers over the bit-flag constants defined in this crate.

use crate::material::{
    FLAG_ALPHA_BLEND, FLAG_ALPHA_TEST, FLAG_DOUBLE_SIDED, FLAG_HAS_ANISOTROPY, FLAG_HAS_CLEAR_COAT,
    FLAG_HAS_CUSTOM_SHADER, FLAG_HAS_NORMAL_MAP, FLAG_HAS_SUBSURFACE,
};

/// Check if a material has double-sided rendering enabled.
pub fn is_double_sided(flags: u32) -> bool {
    flags & FLAG_DOUBLE_SIDED != 0
}

/// Check if a material has alpha blending enabled.
pub fn has_alpha_blend(flags: u32) -> bool {
    flags & FLAG_ALPHA_BLEND != 0
}

/// Check if a material has alpha testing enabled.
pub fn has_alpha_test(flags: u32) -> bool {
    flags & FLAG_ALPHA_TEST != 0
}

/// Check if a material has a normal map bound.
pub fn has_normal_map(flags: u32) -> bool {
    flags & FLAG_HAS_NORMAL_MAP != 0
}

/// Check if a material has clear-coat rendering enabled.
pub fn has_clear_coat(flags: u32) -> bool {
    flags & FLAG_HAS_CLEAR_COAT != 0
}

/// Check if a material has subsurface scattering enabled.
pub fn has_subsurface(flags: u32) -> bool {
    flags & FLAG_HAS_SUBSURFACE != 0
}

/// Check if a material has anisotropy enabled.
pub fn has_anisotropy(flags: u32) -> bool {
    flags & FLAG_HAS_ANISOTROPY != 0
}

/// Check if a material uses a custom shader.
pub fn has_custom_shader(flags: u32) -> bool {
    flags & FLAG_HAS_CUSTOM_SHADER != 0
}
