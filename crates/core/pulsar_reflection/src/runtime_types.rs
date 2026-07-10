//! Runtime type information system
//!
//! Provides compile-time captured type metadata available at runtime for
//! reflection, serialization, and UI generation without enum pattern matching.

use std::any::TypeId;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

/// Runtime type descriptor captured at compile time
///
/// Contains all necessary metadata about a type for runtime introspection,
/// serialization, and UI rendering. Registered automatically via the
/// `#[derive(Reflectable)]` macro.
#[derive(Clone)]
pub struct RuntimeTypeInfo {
    /// Unique type identifier
    pub type_id: TypeId,

    /// Full type name including module path
    pub type_name: &'static str,

    /// Size in bytes
    pub size: usize,

    /// Alignment in bytes
    pub align: usize,

    /// Structural information about the type
    pub structure: TypeStructure,

    /// Optional declared display color for this type, as a hex string
    /// (e.g. `"#58A6FF"`). Set via `#[reflect(color = "#RRGGBB")]`.
    ///
    /// Consumers that need *some* color for a type (e.g. a node-graph pin
    /// renderer) should go through [`RuntimeTypeInfo::resolve_color`], which
    /// falls back to a deterministic hash-based color when this is `None` —
    /// this is the canonical, single source of truth for type display color;
    /// UI frameworks must not derive their own.
    pub color: Option<&'static str>,
}

impl RuntimeTypeInfo {
    /// Get the base type name without module path
    ///
    /// Example: "pulsar::math::Vec3" -> "Vec3"
    pub fn base_name(&self) -> &str {
        self.type_name.split("::").last().unwrap_or(self.type_name)
    }

    /// Check if this is a primitive type
    pub fn is_primitive(&self) -> bool {
        matches!(self.structure, TypeStructure::Primitive)
    }

    /// Check if this is a string type
    pub fn is_string(&self) -> bool {
        matches!(self.structure, TypeStructure::String)
    }

    /// Check if this is a struct type
    pub fn is_struct(&self) -> bool {
        matches!(self.structure, TypeStructure::Struct { .. })
    }

    /// Check if this is an enum type
    pub fn is_enum(&self) -> bool {
        matches!(self.structure, TypeStructure::Enum { .. })
    }

    /// Check if this is a wrapper type (Vec, Arc, etc.)
    pub fn is_wrapper(&self) -> bool {
        matches!(self.structure, TypeStructure::Wrapper { .. })
    }

    /// Get field information if this is a struct
    pub fn fields(&self) -> Option<&[FieldInfo]> {
        match &self.structure {
            TypeStructure::Struct { fields } => Some(fields),
            _ => None,
        }
    }

    /// Get enum variants if this is an enum
    pub fn enum_variants(&self) -> Option<&[&'static str]> {
        match &self.structure {
            TypeStructure::Enum { variants } => Some(variants),
            _ => None,
        }
    }

    /// Get inner type if this is a wrapper
    pub fn inner_type(&self) -> Option<&'static RuntimeTypeInfo> {
        match &self.structure {
            TypeStructure::Wrapper { inner, .. } => Some(inner),
            _ => None,
        }
    }

    /// Check if this is the wildcard "matches anything" type
    pub fn is_wildcard(&self) -> bool {
        matches!(self.structure, TypeStructure::Wildcard)
    }

    /// The shared wildcard type instance — represents "any type" for generic
    /// blueprint nodes (e.g. `Make Array<T>`) and reroute-node type inference.
    pub fn wildcard() -> &'static RuntimeTypeInfo {
        &WILDCARD_TYPE_INFO
    }

    /// Resolve a display color for this type as RGBA in `[0.0, 1.0]`.
    ///
    /// This is the single canonical way to get a type's display color: it
    /// uses the declared [`color`](Self::color) when present, and otherwise
    /// derives a deterministic color from a hash of the type name. Consumers
    /// (e.g. node-graph pin renderers) should always go through this rather
    /// than computing their own type-to-color mapping.
    pub fn resolve_color(&self) -> [f32; 4] {
        if self.is_wildcard() {
            // Rendered specially (rainbow) by consumers; white is the neutral base.
            return [1.0, 1.0, 1.0, 1.0];
        }

        if let Some(hex) = self.color {
            if let Some(rgba) = parse_hex_color(hex) {
                return rgba;
            }
        }

        let mut hasher = DefaultHasher::new();
        self.base_name().hash(&mut hasher);
        let hash = hasher.finish();

        let hue = (hash % 360) as f32;
        let (r, g, b) = hsv_to_rgb(hue, 0.7, 0.8);
        [r, g, b, 1.0]
    }
}

/// Parse a `#RRGGBB` or `#RRGGBBAA` hex color string into RGBA `[0.0, 1.0]`.
fn parse_hex_color(hex: &str) -> Option<[f32; 4]> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    let channel = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| v as f32 / 255.0);

    match hex.len() {
        6 => Some([
            channel(&hex[0..2])?,
            channel(&hex[2..4])?,
            channel(&hex[4..6])?,
            1.0,
        ]),
        8 => Some([
            channel(&hex[0..2])?,
            channel(&hex[2..4])?,
            channel(&hex[4..6])?,
            channel(&hex[6..8])?,
        ]),
        _ => None,
    }
}

/// Convert an HSV color (hue in degrees, saturation/value in `[0,1]`) to RGB.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let c = v * s;
    let h_prime = h / 60.0;
    let x = c * (1.0 - ((h_prime % 2.0) - 1.0).abs());
    let m = v - c;

    let (r_prime, g_prime, b_prime) = match h_prime as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        5 => (c, 0.0, x),
        _ => (0.0, 0.0, 0.0),
    };

    (r_prime + m, g_prime + m, b_prime + m)
}

static WILDCARD_TYPE_INFO: RuntimeTypeInfo = RuntimeTypeInfo {
    type_id: TypeId::of::<()>(),
    type_name: "?",
    size: 0,
    align: 1,
    structure: TypeStructure::Wildcard,
    color: None,
};

impl fmt::Debug for RuntimeTypeInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RuntimeTypeInfo")
            .field("type_id", &self.type_id)
            .field("type_name", &self.type_name)
            .field("size", &self.size)
            .field("align", &self.align)
            .field("structure", &self.structure)
            .field("color", &self.color)
            .finish()
    }
}

/// Structural information about a type
#[derive(Clone, Debug)]
pub enum TypeStructure {
    /// Primitive numeric or boolean type (f32, i32, u64, bool, etc.)
    Primitive,

    /// String type (String, &str)
    String,

    /// Wrapper type containing another type
    Wrapper {
        /// Kind of wrapper (Vec, Arc, HashMap, etc.)
        wrapper_kind: WrapperType,

        /// Inner type being wrapped
        inner: &'static RuntimeTypeInfo,
    },

    /// Struct with named fields
    Struct {
        /// Field metadata array
        fields: &'static [FieldInfo],
    },

    /// Enum with variants
    Enum {
        /// Variant names
        variants: &'static [&'static str],
    },

    /// Wildcard "matches anything" placeholder, used by generic blueprint
    /// nodes (e.g. `Make Array<T>`) and reroute-node type inference.
    Wildcard,
}

/// Types of wrapper containers
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WrapperType {
    /// Vec<T>
    Vec,

    /// Box<T>
    Box,

    /// Arc<T>
    Arc,

    /// Rc<T>
    Rc,

    /// Option<T>
    Option,

    /// Result<T, E> (only captures T)
    Result,

    /// HashMap<K, V> (only captures V for simplicity)
    HashMap,

    /// HashSet<T>
    HashSet,

    /// Custom wrapper type
    Custom(&'static str),
}

/// Information about a struct field
#[derive(Clone, Debug)]
pub struct FieldInfo {
    /// Field name
    pub name: &'static str,

    /// Type information for this field
    pub type_info: &'static RuntimeTypeInfo,

    /// Offset from start of struct in bytes
    pub offset: usize,
}

impl FieldInfo {
    /// Create a new field info
    pub const fn new(
        name: &'static str,
        type_info: &'static RuntimeTypeInfo,
        offset: usize,
    ) -> Self {
        Self {
            name,
            type_info,
            offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_type_info_primitives() {
        let type_info = RuntimeTypeInfo {
            type_id: TypeId::of::<f32>(),
            type_name: "f32",
            size: 4,
            align: 4,
            structure: TypeStructure::Primitive,
            color: None,
        };

        assert_eq!(type_info.base_name(), "f32");
        assert!(type_info.is_primitive());
        assert!(!type_info.is_string());
        assert!(!type_info.is_struct());
    }

    #[test]
    fn test_runtime_type_info_string() {
        let type_info = RuntimeTypeInfo {
            type_id: TypeId::of::<String>(),
            type_name: "alloc::string::String",
            size: std::mem::size_of::<String>(),
            align: std::mem::align_of::<String>(),
            structure: TypeStructure::String,
            color: None,
        };

        assert_eq!(type_info.base_name(), "String");
        assert!(!type_info.is_primitive());
        assert!(type_info.is_string());
    }

    #[test]
    fn test_wrapper_type() {
        static INNER: RuntimeTypeInfo = RuntimeTypeInfo {
            type_id: TypeId::of::<f32>(),
            type_name: "f32",
            size: 4,
            align: 4,
            structure: TypeStructure::Primitive,
            color: None,
        };

        let vec_type = RuntimeTypeInfo {
            type_id: TypeId::of::<Vec<f32>>(),
            type_name: "alloc::vec::Vec<f32>",
            size: std::mem::size_of::<Vec<f32>>(),
            align: std::mem::align_of::<Vec<f32>>(),
            structure: TypeStructure::Wrapper {
                wrapper_kind: WrapperType::Vec,
                inner: &INNER,
            },
            color: None,
        };

        assert!(vec_type.is_wrapper());
        assert_eq!(vec_type.inner_type().unwrap().type_name, "f32");
    }
}
