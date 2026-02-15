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

/// Trait for composite types that expose indexed sub-fields
pub trait CompositeFieldAccessor {
    /// Get the value at the given index
    fn get_component(&self, index: usize) -> Option<f32>;
    
    /// Set the value at the given index
    fn set_component(&mut self, index: usize, value: f32);
    
    /// Get the number of components
    fn component_count(&self) -> usize;
}

/// Implement CompositeFieldAccessor for Vec3
impl CompositeFieldAccessor for Vec3 {
    fn get_component(&self, index: usize) -> Option<f32> {
        self.get(index).copied()
    }
    
    fn set_component(&mut self, index: usize, value: f32) {
        if index < 3 {
            self[index] = value;
        }
    }
    
    fn component_count(&self) -> usize {
        3
    }
}

/// Implement CompositeFieldAccessor for Color
impl CompositeFieldAccessor for Color {
    fn get_component(&self, index: usize) -> Option<f32> {
        self.get(index).copied()
    }
    
    fn set_component(&mut self, index: usize, value: f32) {
        if index < 4 {
            self[index] = value;
        }
    }
    
    fn component_count(&self) -> usize {
        4
    }
}

/// Descriptor for a sub-field in a composite type
#[derive(Clone, Debug)]
pub struct SubFieldDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub color_hint: Option<[f32; 3]>, // RGB color hint for styling (e.g., X=red, Y=green, Z=blue)
    pub index: usize, // Index into the composite type (e.g., 0 for X, 1 for Y, 2 for Z)
}

impl SubFieldDescriptor {
    /// Create a new sub-field descriptor
    pub fn new(name: &'static str, label: &'static str, index: usize) -> Self {
        Self {
            name,
            label,
            color_hint: None,
            index,
        }
    }
    
    /// Set the color hint for this sub-field
    pub fn with_color(mut self, color: [f32; 3]) -> Self {
        self.color_hint = Some(color);
        self
    }
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
            SubFieldDescriptor::new("x", "X", 0)
                .with_color([1.0, 0.3, 0.3]),
            SubFieldDescriptor::new("y", "Y", 1)
                .with_color([0.3, 1.0, 0.3]),
            SubFieldDescriptor::new("z", "Z", 2)
                .with_color([0.3, 0.5, 1.0]),
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
            SubFieldDescriptor::new("r", "R", 0)
                .with_color([1.0, 0.3, 0.3]),
            SubFieldDescriptor::new("g", "G", 1)
                .with_color([0.3, 1.0, 0.3]),
            SubFieldDescriptor::new("b", "B", 2)
                .with_color([0.3, 0.5, 1.0]),
            SubFieldDescriptor::new("a", "A", 3),
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
