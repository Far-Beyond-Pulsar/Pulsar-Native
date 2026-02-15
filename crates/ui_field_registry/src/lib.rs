//! UI Field Registry - Trait-based type-to-UI mapping system
//!
//! This crate provides a trait-based architecture for mapping Rust types to UI components.
//! Any type can implement `FieldRenderer` to define how it should be rendered in the properties panel.

use std::any::Any;

/// How a field should be rendered
pub enum FieldRepresentation {
    /// Primitive field - renders as a single input (f32, bool, String, etc.)
    Primitive,
    
    /// Composite field - renders as multiple sub-fields using primitives
    Composite(Vec<SubFieldDescriptor>),
    
    /// Custom rendering - type provides its own GPUI rendering logic
    /// (The actual rendering happens in the UI layer, this is just a marker)
    Custom,
}

/// Trait for types that can be rendered as fields in the properties panel
pub trait FieldRenderer: Any {
    /// Get the type name for debugging
    fn type_name(&self) -> &'static str;
    
    /// Get the field representation (primitive, composite, or custom)
    fn representation(&self) -> FieldRepresentation {
        FieldRepresentation::Primitive
    }
}

/// Descriptor for a sub-field in a composite type
#[derive(Clone, Debug)]
pub struct SubFieldDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub color_hint: Option<[f32; 3]>, // RGB color hint for styling (e.g., X=red, Y=green, Z=blue)
}

// ─── Primitive implementations ───────────────────────────────────────────────

impl FieldRenderer for f32 {
    fn type_name(&self) -> &'static str {
        "f32"
    }
}

impl FieldRenderer for bool {
    fn type_name(&self) -> &'static str {
        "bool"
    }
}

impl FieldRenderer for String {
    fn type_name(&self) -> &'static str {
        "String"
    }
}

// ─── Composite implementations ───────────────────────────────────────────────

/// Vec3 type alias for clarity
pub type Vec3 = [f32; 3];

impl FieldRenderer for Vec3 {
    fn type_name(&self) -> &'static str {
        "Vec3"
    }
    
    fn representation(&self) -> FieldRepresentation {
        FieldRepresentation::Composite(vec![
            SubFieldDescriptor {
                name: "x",
                label: "X",
                color_hint: Some([1.0, 0.3, 0.3]),
            },
            SubFieldDescriptor {
                name: "y",
                label: "Y",
                color_hint: Some([0.3, 1.0, 0.3]),
            },
            SubFieldDescriptor {
                name: "z",
                label: "Z",
                color_hint: Some([0.3, 0.5, 1.0]),
            },
        ])
    }
}

/// Color type alias (RGBA)
pub type Color = [f32; 4];

impl FieldRenderer for Color {
    fn type_name(&self) -> &'static str {
        "Color"
    }
    
    fn representation(&self) -> FieldRepresentation {
        FieldRepresentation::Composite(vec![
            SubFieldDescriptor {
                name: "r",
                label: "R",
                color_hint: Some([1.0, 0.3, 0.3]),
            },
            SubFieldDescriptor {
                name: "g",
                label: "G",
                color_hint: Some([0.3, 1.0, 0.3]),
            },
            SubFieldDescriptor {
                name: "b",
                label: "B",
                color_hint: Some([0.3, 0.5, 1.0]),
            },
            SubFieldDescriptor {
                name: "a",
                label: "A",
                color_hint: None,
            },
        ])
    }
}

// ─── Helper functions ────────────────────────────────────────────────────────

/// Check if a field representation is primitive
pub fn is_primitive_representation(repr: &FieldRepresentation) -> bool {
    matches!(repr, FieldRepresentation::Primitive)
}

/// Check if a field representation is composite
pub fn is_composite_representation(repr: &FieldRepresentation) -> bool {
    matches!(repr, FieldRepresentation::Composite(_))
}

/// Check if a field representation uses custom rendering
pub fn is_custom_representation(repr: &FieldRepresentation) -> bool {
    matches!(repr, FieldRepresentation::Custom)
}
