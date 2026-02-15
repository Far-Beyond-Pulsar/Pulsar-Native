//! UI Field Registry - Trait-based type-to-UI mapping system
//!
//! This crate provides a trait-based architecture for mapping Rust types to UI components.
//! Any type can implement `FieldRenderer` to define how it should be rendered in the properties panel.
//!
//! **Type-Safe Field Access**: Uses named fields instead of array indices for compile-time verification.

use std::any::Any;
use serde::{Deserialize, Serialize};

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
    
    /// For Custom representation types, return an identifier that the UI layer can use
    /// to look up the custom renderer. Defaults to type_name().
    fn custom_ui_key(&self) -> &'static str {
        self.type_name()
    }
}

/// Trait for composite types that expose named sub-fields (type-safe field access by name)
pub trait CompositeField {
    /// Get an f32 field by name
    fn get_field_f32(&self, _field_name: &str) -> Option<f32> {
        None
    }
    
    /// Set an f32 field by name
    fn set_field_f32(&mut self, _field_name: &str, _value: f32) {
        // Default no-op
    }
    
    /// Get a bool field by name
    fn get_field_bool(&self, _field_name: &str) -> Option<bool> {
        None
    }
    
    /// Set a bool field by name
    fn set_field_bool(&mut self, _field_name: &str, _value: bool) {
        // Default no-op
    }
    
    /// Get a String field by name
    fn get_field_string(&self, _field_name: &str) -> Option<String> {
        None
    }
    
    /// Set a String field by name
    fn set_field_string(&mut self, _field_name: &str, _value: String) {
        // Default no-op
    }
}

/// Descriptor for a sub-field in a composite type
#[derive(Clone, Debug)]
pub struct SubFieldDescriptor {
    pub name: &'static str,
    pub label: &'static str,
    pub color_hint: Option<[f32; 3]>, // RGB color hint for styling (e.g., X=red, Y=green, Z=blue)
    pub field_type: SubFieldType,
}

/// The type of a sub-field
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubFieldType {
    F32,
    Bool,
    String,
}

impl SubFieldDescriptor {
    /// Create a new f32 sub-field descriptor
    pub fn f32(name: &'static str, label: &'static str) -> Self {
        Self {
            name,
            label,
            color_hint: None,
            field_type: SubFieldType::F32,
        }
    }
    
    /// Create a new bool sub-field descriptor
    pub fn bool(name: &'static str, label: &'static str) -> Self {
        Self {
            name,
            label,
            color_hint: None,
            field_type: SubFieldType::Bool,
        }
    }
    
    /// Create a new String sub-field descriptor
    pub fn string(name: &'static str, label: &'static str) -> Self {
        Self {
            name,
            label,
            color_hint: None,
            field_type: SubFieldType::String,
        }
    }
    
    /// Set the color hint for this sub-field
    pub fn with_color(mut self, color: [f32; 3]) -> Self {
        self.color_hint = Some(color);
        self
    }
    
    /// Set the color hint for this sub-field (takes Option for convenience)
    pub fn with_color_opt(mut self, color: Option<[f32; 3]>) -> Self {
        self.color_hint = color;
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

// ─── Composite struct types ──────────────────────────────────────────────────

/// 3D Vector - Position, rotation, scale, direction
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "derive", derive(ui_gen_macros::CompositeField))]
pub struct Vec3 {
    #[cfg_attr(feature = "derive", field(label = "X", color = "red"))]
    pub x: f32,
    #[cfg_attr(feature = "derive", field(label = "Y", color = "green"))]
    pub y: f32,
    #[cfg_attr(feature = "derive", field(label = "Z", color = "blue"))]
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    pub const ONE: Self = Self::new(1.0, 1.0, 1.0);
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);
}

// Manual implementation when derive feature is disabled
#[cfg(not(feature = "derive"))]
impl FieldRenderer for Vec3 {
    fn type_name(&self) -> &'static str {
        "Vec3"
    }
    
    fn representation(&self) -> FieldRepresentation {
        FieldRepresentation::Composite(vec![
            SubFieldDescriptor::f32("x", "X")
                .with_color([1.0, 0.3, 0.3]),
            SubFieldDescriptor::f32("y", "Y")
                .with_color([0.3, 1.0, 0.3]),
            SubFieldDescriptor::f32("z", "Z")
                .with_color([0.3, 0.5, 1.0]),
        ])
    }
}

#[cfg(not(feature = "derive"))]
impl CompositeField for Vec3 {
    fn get_field_f32(&self, field_name: &str) -> Option<f32> {
        match field_name {
            "x" => Some(self.x),
            "y" => Some(self.y),
            "z" => Some(self.z),
            _ => None,
        }
    }
    
    fn set_field_f32(&mut self, field_name: &str, value: f32) {
        match field_name {
            "x" => self.x = value,
            "y" => self.y = value,
            "z" => self.z = value,
            _ => {},
        }
    }
}

/// RGBA Color - Red, Green, Blue, Alpha channels
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "derive", derive(ui_gen_macros::CompositeField))]
pub struct Color {
    #[cfg_attr(feature = "derive", field(label = "R", color = "red"))]
    pub r: f32,
    #[cfg_attr(feature = "derive", field(label = "G", color = "green"))]
    pub g: f32,
    #[cfg_attr(feature = "derive", field(label = "B", color = "blue"))]
    pub b: f32,
    #[cfg_attr(feature = "derive", field(label = "A"))]
    pub a: f32,
}

impl Color {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self::new(r, g, b, 1.0)
    }
    
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);
    pub const RED: Self = Self::new(1.0, 0.0, 0.0, 1.0);
    pub const GREEN: Self = Self::new(0.0, 1.0, 0.0, 1.0);
    pub const BLUE: Self = Self::new(0.0, 0.0, 1.0, 1.0);
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);
}

#[cfg(not(feature = "derive"))]
impl FieldRenderer for Color {
    fn type_name(&self) -> &'static str {
        "Color"
    }
    
    fn representation(&self) -> FieldRepresentation {
        FieldRepresentation::Composite(vec![
            SubFieldDescriptor::f32("r", "R")
                .with_color([1.0, 0.3, 0.3]),
            SubFieldDescriptor::f32("g", "G")
                .with_color([0.3, 1.0, 0.3]),
            SubFieldDescriptor::f32("b", "B")
                .with_color([0.3, 0.5, 1.0]),
            SubFieldDescriptor::f32("a", "A"),
        ])
    }
}

#[cfg(not(feature = "derive"))]
impl CompositeField for Color {
    fn get_field_f32(&self, field_name: &str) -> Option<f32> {
        match field_name {
            "r" => Some(self.r),
            "g" => Some(self.g),
            "b" => Some(self.b),
            "a" => Some(self.a),
            _ => None,
        }
    }
    
    fn set_field_f32(&mut self, field_name: &str, value: f32) {
        match field_name {
            "r" => self.r = value,
            "g" => self.g = value,
            "b" => self.b = value,
            "a" => self.a = value,
            _ => {},
        }
    }
}

// ─── Conversion traits for backward compatibility ────────────────────────────

impl From<[f32; 3]> for Vec3 {
    fn from(arr: [f32; 3]) -> Self {
        Self::new(arr[0], arr[1], arr[2])
    }
}

impl From<Vec3> for [f32; 3] {
    fn from(v: Vec3) -> Self {
        [v.x, v.y, v.z]
    }
}

impl From<[f32; 4]> for Color {
    fn from(arr: [f32; 4]) -> Self {
        Self::new(arr[0], arr[1], arr[2], arr[3])
    }
}

impl From<Color> for [f32; 4] {
    fn from(c: Color) -> Self {
        [c.r, c.g, c.b, c.a]
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
