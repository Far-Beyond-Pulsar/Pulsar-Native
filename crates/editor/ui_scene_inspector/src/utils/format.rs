//! How names, sizes and entities are written.

use pulsar_scenedb::Entity;

pub fn bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KIB * KIB * KIB {
        format!("{:.2} GiB", b / (KIB * KIB * KIB))
    } else if b >= KIB * KIB {
        format!("{:.1} MiB", b / (KIB * KIB))
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

/// A count with thousands separators.
pub use pulsar_reflection::value_tree::grouped as count;

/// `12v1`: slot index and generation.
pub fn entity(entity: Entity) -> String {
    format!("{}v{}", entity.index(), entity.generation())
}

/// A type name without module paths, generic arguments kept:
/// `alloc::vec::Vec<a::b::C>` is `Vec<C>`.
pub fn short_type(full: &str) -> String {
    let mut out = String::with_capacity(full.len());
    let mut segment = String::new();
    let mut chars = full.chars().peekable();
    while let Some(c) = chars.next() {
        if c == ':' && chars.peek() == Some(&':') {
            chars.next();
            segment.clear();
        } else if c.is_alphanumeric() || c == '_' {
            segment.push(c);
        } else {
            out.push_str(&segment);
            segment.clear();
            out.push(c);
        }
    }
    out.push_str(&segment);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_lose_their_paths_and_keep_their_generics() {
        assert_eq!(short_type("a::b::Transform"), "Transform");
        assert_eq!(short_type("alloc::vec::Vec<a::b::C>"), "Vec<C>");
        assert_eq!(
            short_type("std::collections::HashMap<alloc::string::String, [f32; 3]>"),
            "HashMap<String, [f32; 3]>"
        );
        assert_eq!(short_type("(a::A, b::B)"), "(A, B)");
    }

    #[test]
    fn counts_are_grouped() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(12_345_678), "12,345,678");
    }
}
