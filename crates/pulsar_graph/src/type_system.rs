use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeInfo {
    pub base_type: String,
    pub wrappers: Vec<WrapperType>,
    pub is_wildcard: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WrapperType {
    Vec,
    HashMap,
    HashSet,
    Arc,
    Box,
    Ref,
    RefMut,
    Option,
    Result,
}

#[derive(Debug, Clone)]
pub struct PinColor {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

#[derive(Debug, Clone)]
pub struct PinStyle {
    pub color: PinColor,
    pub icon: PinIcon,
    pub is_rainbow: bool,
}

#[derive(Debug, Clone)]
pub enum PinIcon {
    Circle,        // Default for primitives
    Square,        // Vec
    Diamond,       // HashMap/HashSet
    Triangle,      // Arc
    Hexagon,       // Box
    HollowCircle,  // Ref/RefMut
    Star,          // Option
    Cross,         // Result
    Rainbow,       // Wildcard
}

impl TypeInfo {
    /// Parse a Rust type string into TypeInfo
    pub fn parse(type_str: &str) -> Self {
        if type_str == "?" || type_str == "_" || type_str == "T" {
            return Self {
                base_type: "wildcard".to_string(),
                wrappers: Vec::new(),
                is_wildcard: true,
            };
        }

        let mut remaining = type_str;
        let mut wrappers = Vec::new();

        // Parse wrapper types from outside in
        loop {
            remaining = remaining.trim();

            if let Some(inner) = Self::extract_wrapper(remaining, "Vec<", ">") {
                wrappers.push(WrapperType::Vec);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "HashMap<", ">") {
                wrappers.push(WrapperType::HashMap);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "HashSet<", ">") {
                wrappers.push(WrapperType::HashSet);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "Arc<", ">") {
                wrappers.push(WrapperType::Arc);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "Box<", ">") {
                wrappers.push(WrapperType::Box);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "&mut ", "") {
                wrappers.push(WrapperType::RefMut);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "&", "") {
                wrappers.push(WrapperType::Ref);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "Option<", ">") {
                wrappers.push(WrapperType::Option);
                remaining = inner;
            } else if let Some(inner) = Self::extract_wrapper(remaining, "Result<", ">") {
                wrappers.push(WrapperType::Result);
                remaining = inner;
            } else {
                break;
            }
        }

        // Check if the inner type is a wildcard
        let is_wildcard = remaining == "?" || remaining == "_" || remaining == "T" ||
                         remaining.len() == 1 && remaining.chars().next().unwrap().is_uppercase();

        Self {
            base_type: if is_wildcard { "wildcard".to_string() } else { remaining.to_string() },
            wrappers,
            is_wildcard,
        }
    }

    fn extract_wrapper<'a>(type_str: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
        if type_str.starts_with(prefix) {
            if suffix.is_empty() {
                Some(&type_str[prefix.len()..])
            } else if type_str.ends_with(suffix) {
                Some(&type_str[prefix.len()..type_str.len() - suffix.len()])
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Generate a deterministic color for this type
    pub fn generate_color(&self) -> PinColor {
        if self.is_wildcard {
            // Rainbow color will be handled specially in rendering
            return PinColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
        }

        // Create a deterministic hash of the base type
        let mut hasher = DefaultHasher::new();
        self.base_type.hash(&mut hasher);
        let hash = hasher.finish();

        // Convert hash to HSV color space for better distribution
        let hue = (hash % 360) as f32;
        let saturation = 0.7; // Fixed saturation for consistent look
        let value = 0.8;      // Fixed value for consistent brightness

        // Convert HSV to RGB
        let (r, g, b) = Self::hsv_to_rgb(hue, saturation, value);

        PinColor { r, g, b, a: 1.0 }
    }

    /// Generate the complete pin style including icon
    pub fn generate_pin_style(&self) -> PinStyle {
        let color = self.generate_color();
        let icon = self.get_icon();
        let is_rainbow = self.is_wildcard;

        PinStyle {
            color,
            icon,
            is_rainbow,
        }
    }

    fn get_icon(&self) -> PinIcon {
        if self.is_wildcard {
            return PinIcon::Rainbow;
        }

        // Use the outermost wrapper type for the icon
        if let Some(wrapper) = self.wrappers.first() {
            match wrapper {
                WrapperType::Vec => PinIcon::Square,
                WrapperType::HashMap | WrapperType::HashSet => PinIcon::Diamond,
                WrapperType::Arc => PinIcon::Triangle,
                WrapperType::Box => PinIcon::Hexagon,
                WrapperType::Ref | WrapperType::RefMut => PinIcon::HollowCircle,
                WrapperType::Option => PinIcon::Star,
                WrapperType::Result => PinIcon::Cross,
            }
        } else {
            PinIcon::Circle
        }
    }

    /// Check if two types are compatible for connection
    pub fn is_compatible_with(&self, other: &TypeInfo) -> bool {
        // Wildcard types can connect to anything
        if self.is_wildcard || other.is_wildcard {
            return true;
        }

        // For now, exact type match (can be extended later)
        self.base_type == other.base_type && self.wrappers == other.wrappers
    }

    /// Check if this type can be converted to another type
    pub fn can_convert_to(&self, other: &TypeInfo) -> bool {
        // Wildcard types can convert to anything
        if self.is_wildcard || other.is_wildcard {
            return true;
        }

        // Same base type with compatible wrappers
        if self.base_type == other.base_type {
            // Allow some wrapper conversions (e.g., T -> &T, T -> Box<T>)
            return true;
        }

        // Built-in conversions
        match (self.base_type.as_str(), other.base_type.as_str()) {
            ("i32", "f32") | ("i32", "f64") | ("f32", "f64") => true,
            ("&str", "String") | ("String", "&str") => true,
            _ => false,
        }
    }

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
}

impl std::fmt::Display for TypeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_wildcard {
            return write!(f, "?");
        }

        let mut result = self.base_type.clone();

        // Apply wrappers from innermost to outermost (reverse order)
        for wrapper in self.wrappers.iter().rev() {
            result = match wrapper {
                WrapperType::Vec => format!("Vec<{}>", result),
                WrapperType::HashMap => format!("HashMap<{}>", result),
                WrapperType::HashSet => format!("HashSet<{}>", result),
                WrapperType::Arc => format!("Arc<{}>", result),
                WrapperType::Box => format!("Box<{}>", result),
                WrapperType::Ref => format!("&{}", result),
                WrapperType::RefMut => format!("&mut {}", result),
                WrapperType::Option => format!("Option<{}>", result),
                WrapperType::Result => format!("Result<{}>", result),
            };
        }

        write!(f, "{}", result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_parsing() {
        // Basic types
        assert_eq!(TypeInfo::parse("i32").base_type, "i32");
        assert_eq!(TypeInfo::parse("String").base_type, "String");

        // Wrapper types
        let vec_type = TypeInfo::parse("Vec<i32>");
        assert_eq!(vec_type.base_type, "i32");
        assert_eq!(vec_type.wrappers, vec![WrapperType::Vec]);

        // Nested wrappers
        let complex_type = TypeInfo::parse("Arc<Vec<String>>");
        assert_eq!(complex_type.base_type, "String");
        assert_eq!(complex_type.wrappers, vec![WrapperType::Arc, WrapperType::Vec]);

        // Wildcard types
        let wildcard = TypeInfo::parse("?");
        assert!(wildcard.is_wildcard);
        assert_eq!(wildcard.base_type, "wildcard");
    }

    #[test]
    fn test_type_compatibility() {
        let i32_type = TypeInfo::parse("i32");
        let f32_type = TypeInfo::parse("f32");
        let wildcard = TypeInfo::parse("?");

        // Wildcard compatibility
        assert!(wildcard.is_compatible_with(&i32_type));
        assert!(i32_type.is_compatible_with(&wildcard));

        // Same type compatibility
        assert!(i32_type.is_compatible_with(&i32_type));

        // Different type incompatibility (for now)
        assert!(!i32_type.is_compatible_with(&f32_type));
    }

    #[test]
    fn test_color_generation() {
        let type1 = TypeInfo::parse("i32");
        let type2 = TypeInfo::parse("i32");
        let type3 = TypeInfo::parse("String");

        let color1 = type1.generate_color();
        let color2 = type2.generate_color();
        let color3 = type3.generate_color();

        // Same type should generate same color
        assert_eq!((color1.r, color1.g, color1.b), (color2.r, color2.g, color2.b));

        // Different types should generate different colors
        assert_ne!((color1.r, color1.g, color1.b), (color3.r, color3.g, color3.b));
    }

    #[test]
    fn test_display() {
        assert_eq!(TypeInfo::parse("i32").to_string(), "i32");
        assert_eq!(TypeInfo::parse("Vec<i32>").to_string(), "Vec<i32>");
        assert_eq!(TypeInfo::parse("Arc<Vec<String>>").to_string(), "Arc<Vec<String>>");
        assert_eq!(TypeInfo::parse("?").to_string(), "?");
    }
}