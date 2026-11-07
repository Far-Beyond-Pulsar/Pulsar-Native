//! The shipped Radiant templates: each a material class's surface function
//! and its helpers (`radiant::RadiantTemplate`), which a drawer composes
//! into its own program (`RadiantTemplate::compose`). The opaque registry's
//! built-ins are [`DEFAULT_SURFACE`] (class 0) and the classes 1-4; the
//! transparent registry's class 0 is [`TRANSPARENT_DEFAULT`]; hosts register
//! the others as custom templates (`radiant_demo`).

/// The default material class (`MATERIAL_CLASS_DEFAULT`, `default_pbr`):
/// `default_pbr_surface` with the material graph's override section.
pub const DEFAULT_SURFACE: &str = include_str!("../templates/default_surface.wgsl");
pub const CLEAR_COAT: &str = include_str!("../templates/clear_coat.wgsl");
pub const SUBSURFACE: &str = include_str!("../templates/subsurface.wgsl");
pub const ANISOTROPIC: &str = include_str!("../templates/anisotropic.wgsl");
pub const SKIN: &str = include_str!("../templates/skin.wgsl");
pub const FOLIAGE: &str = include_str!("../templates/foliage.wgsl");
/// The transparent drawers' default surface
/// (`radiant_eval_transparent(material, input, ambient)`).
pub const TRANSPARENT_DEFAULT: &str = include_str!("../templates/transparent_default.wgsl");
pub const OPAL: &str = include_str!("../templates/opal.wgsl");
pub const GLASS: &str = include_str!("../templates/glass.wgsl");
pub const GLASS_TRANSPARENT: &str = include_str!("../templates/glass_transparent.wgsl");
pub const WATER: &str = include_str!("../templates/water.wgsl");
pub const WATER_TRANSPARENT: &str = include_str!("../templates/water_transparent.wgsl");
