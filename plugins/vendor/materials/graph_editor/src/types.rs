//! Shader pin types: the WGSL type universe, kept apart from reflected Pulsar types.
//!
//! Pins carry WGSL spellings (`f32`, `vec3<f32>`, `texture_2d<f32>`, the editor's
//! `TextureSrc` asset handle). The shader editor judges connections with
//! [`ShaderTypeResolver`], composed behind Graphy's structural rules by
//! [`resolver`]; it never asks the reflection registry, so a WGSL `f32` is not
//! confused with a reflected Rust one.

use graphy::{ChainResolver, Compatibility, DataType, StructuralResolver, TypeResolver};
use graphy_render::{geom::rgba, Color};

/// Connection rules between WGSL pin types: the same type connects, a
/// single-uppercase-letter generic takes any type, everything else does not.
#[derive(Debug, Default, Clone, Copy)]
pub struct ShaderTypeResolver;

impl TypeResolver for ShaderTypeResolver {
    fn compatibility(&self, from: &DataType, to: &DataType) -> Compatibility {
        let (DataType::Data(a), DataType::Data(b)) = (from, to) else {
            return Compatibility::Unknown;
        };
        let (a, b) = (a.type_string.as_str(), b.type_string.as_str());
        if a == b {
            Compatibility::Exact
        } else if is_generic(a) || is_generic(b) {
            Compatibility::Wildcard
        } else {
            Compatibility::Incompatible(format!("'{a}' does not connect to '{b}'"))
        }
    }
}

/// `T`, `U`...: a generic placeholder the pin takes its type from.
fn is_generic(type_string: &str) -> bool {
    let mut chars = type_string.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_uppercase())
}

/// The resolver the shader editor connects with: Graphy's structural rules
/// (execution, identity, wildcard) first, then the WGSL rules.
pub fn resolver() -> ChainResolver {
    ChainResolver::new()
        .with(StructuralResolver)
        .with(ShaderTypeResolver)
}

/// The type string of a data pin, or `""` for execution.
pub fn type_name(data_type: &DataType) -> &str {
    data_type.type_string().unwrap_or("")
}

/// Whether the value of a pin of this type can be shown as a colour swatch.
pub fn is_texture_previewable(data_type: &DataType) -> bool {
    matches!(type_name(data_type), "vec4<f32>" | "vec3<f32>")
}

/// Whether the pin is a texture or an asset handle for one.
pub fn is_texture(data_type: &DataType) -> bool {
    let name = type_name(data_type);
    name == "TextureSrc" || name.starts_with("texture_")
}

/// Pin colour: textures purple, every other type from Graphy's stable palette.
pub fn pin_color(data_type: &DataType) -> Color {
    if is_texture(data_type) {
        rgba(0x9e, 0x57, 0xeb, 1.0)
    } else {
        graphy_render::default_pin_style(data_type).color
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str) -> DataType {
        DataType::typed(name)
    }

    #[test]
    fn wgsl_types_connect_by_name() {
        let r = resolver();
        assert_eq!(r.compatibility(&t("vec3<f32>"), &t("vec3<f32>")), Compatibility::Exact);
        assert!(matches!(
            r.compatibility(&t("vec3<f32>"), &t("vec4<f32>")),
            Compatibility::Incompatible(_)
        ));
        assert!(!r.compatibility(&t("f32"), &t("TextureSrc")).is_connectable());
    }

    #[test]
    fn wildcards_and_generics_take_any_type() {
        let r = resolver();
        assert!(r.compatibility(&t("?"), &t("vec2<f32>")).is_connectable());
        assert!(r.compatibility(&t("f32"), &t("T")).is_connectable());
        assert!(r.compatibility(&t("U"), &t("mat4x4<f32>")).is_connectable());
    }

    #[test]
    fn execution_never_meets_data() {
        let r = resolver();
        assert!(!r.compatibility(&DataType::Exec, &t("f32")).is_connectable());
        assert!(r.compatibility(&DataType::Exec, &DataType::Exec).is_connectable());
    }

    #[test]
    fn colour_pins_preview_and_textures_are_purple() {
        assert!(is_texture_previewable(&t("vec4<f32>")));
        assert!(!is_texture_previewable(&t("f32")));
        assert!(is_texture(&t("TextureSrc")));
        assert!(is_texture(&t("texture_2d<f32>")));
        assert_eq!(pin_color(&t("TextureSrc")), pin_color(&t("texture_2d<f32>")));
    }
}
