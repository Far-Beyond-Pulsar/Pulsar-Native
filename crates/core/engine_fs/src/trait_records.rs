//! Trait declarations as the project index sees them.
//!
//! A trait asset (`*.trait.json`) is parsed into a [`TraitRecord`] without
//! `ui_types_common`, so the index and [`TraitDb`](crate::TraitDb) stay
//! available to a runtime that has no editor. Parsing is deliberately
//! tolerant: it accepts both the canonical envelope (`schemaVersion`,
//! `typeKind`, `displayName`) and the older `display_name` template, exactly
//! the shapes the index accepted before.
//!
//! Identity: a trait's [`id`](TraitRecord::id) is what Blueprints reference.
//! The path is where the trait currently lives and may change. See
//! `.agents/TRAITS.md` for how ids are assigned and kept.

use std::path::Component;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One parameter of a trait method; `type_name` is the Rust-style spelling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitParamRecord {
    pub name: String,
    pub type_name: String,
}

/// One method of a trait.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitMethodRecord {
    pub name: String,
    #[serde(default)]
    pub params: Vec<TraitParamRecord>,
    /// `()` when the method returns nothing.
    pub return_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// An implementer may omit a method with a default body.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_default: bool,
}

/// A parsed trait asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitRecord {
    /// Stable identity (see module docs).
    pub id: String,
    /// Normalized project-relative path of the asset right now.
    pub path: String,
    pub name: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Engine-owned (`meta.builtin`): the method contract is fixed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub builtin: bool,
    /// Version of a built-in's method contract (`meta.contractVersion`).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub contract_version: u32,
    /// Traits this one extends, as authored: ids or project-relative paths.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supertraits: Vec<String>,
    pub methods: Vec<TraitMethodRecord>,
    /// The `trait:Name` entry of the project's `type-index/index.json`, when
    /// the type system also lists this trait.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_index_id: Option<String>,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl TraitRecord {
    /// Whether `trait` is the same contract, ignoring documentation.
    pub fn same_contract(&self, other: &TraitRecord) -> bool {
        self.methods.len() == other.methods.len()
            && self.methods.iter().zip(&other.methods).all(|(a, b)| {
                a.name == b.name
                    && a.return_type == b.return_type
                    && a.has_default == b.has_default
                    && a.params == b.params
            })
    }
}

/// `path` as a normalized, project-relative, forward-slash path, or `None`
/// if it is absolute or escapes the project.
pub(crate) fn normalize_relative_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let path = std::path::Path::new(&normalized);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return None;
    }
    let parts = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Trait definitions are project assets and may be stored anywhere in the
/// project. `types/traits/` is the default creation location, not a validity
/// constraint; older projects may already have root-level trait files.
pub(crate) fn is_trait_asset(path: &str) -> bool {
    path.ends_with(".trait.json")
}

pub(crate) fn is_blueprint_asset(path: &str) -> bool {
    path.ends_with(".blueprint.json")
        || std::path::Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            == Some("graph_save.json")
}

/// Whether `value` is a recognizable trait declaration. The canonical
/// envelope must be complete; the older template needs `display_name`.
pub(crate) fn valid_trait_declaration(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    let Some(name) = object.get("name").and_then(Value::as_str) else {
        return false;
    };
    if name.trim().is_empty() || !object.get("methods").is_some_and(Value::is_array) {
        return false;
    }

    // Require one complete, recognizable shape rather than accepting any JSON
    // object that happens to contain a name and methods array.
    let has_canonical_marker =
        object.contains_key("schemaVersion") || object.contains_key("typeKind");
    if has_canonical_marker {
        return object.get("schemaVersion").and_then(Value::as_u64) == Some(1)
            && object.get("typeKind").and_then(Value::as_str) == Some("trait")
            && object
                .get("displayName")
                .and_then(Value::as_str)
                .is_some_and(|display_name| !display_name.trim().is_empty())
            && object.contains_key("meta");
    }

    object
        .get("display_name")
        .and_then(Value::as_str)
        .is_some_and(|display_name| !display_name.trim().is_empty())
}

/// Parse a Blueprint asset. The editor writes `//` comment lines before the
/// JSON (graphy-blueprint `codec::strip_header_comments`); they are skipped
/// the same way. Anything else that is not JSON is `None`.
pub(crate) fn parse_blueprint_json(bytes: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start_matches(['\r', '\n', ' ', '\t']);
        if let Some(comment) = trimmed.strip_prefix("//") {
            rest = comment.split_once('\n').map_or("", |(_, after)| after);
        } else {
            rest = trimmed;
            break;
        }
    }
    serde_json::from_str(rest).ok()
}

/// The id embedded in a trait declaration, if it has a usable one.
pub(crate) fn embedded_id(value: &Value) -> Option<String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

/// Spell a `TypeRef` JSON value (`{"kind":"Primitive","name":"f32"}` ...) the
/// way the trait editor shows it.
pub(crate) fn type_ref_text(value: &Value) -> String {
    match value.get("kind").and_then(Value::as_str) {
        Some("Primitive") => value.get("name").and_then(Value::as_str),
        Some("Path") => value.get("path").and_then(Value::as_str),
        Some("AliasRef") => {
            return format!(
                "alias:{}",
                value.get("alias").and_then(Value::as_str).unwrap_or("?")
            )
        }
        _ => None,
    }
    .unwrap_or("()")
    .to_owned()
}

/// Parse a validated declaration. `path` is where it was read from and `id`
/// the identity the caller settled on (see the index's identity pass).
pub(crate) fn parse_trait_record(value: &Value, path: &str, id: String) -> Option<TraitRecord> {
    if !valid_trait_declaration(value) {
        return None;
    }
    let object = value.as_object()?;
    let name = object.get("name")?.as_str()?.to_owned();
    let display_name = object
        .get("displayName")
        .or_else(|| object.get("display_name"))
        .and_then(Value::as_str)
        .unwrap_or(&name)
        .to_owned();
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|text| !text.is_empty());
    let meta = object.get("meta");
    let builtin = meta
        .and_then(|meta| meta.get("builtin"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let contract_version = meta
        .and_then(|meta| meta.get("contractVersion"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;
    let supertraits = object
        .get("supertraits")
        .and_then(Value::as_array)
        .map(|list| {
            let mut refs: Vec<String> = list
                .iter()
                .filter_map(Value::as_str)
                .map(|text| {
                    // A path reference is stored normalized; an id as written.
                    normalize_relative_path(text)
                        .filter(|normalized| is_trait_asset(normalized))
                        .unwrap_or_else(|| text.trim().to_owned())
                })
                .filter(|text| !text.is_empty())
                .collect();
            refs.sort();
            refs.dedup();
            refs
        })
        .unwrap_or_default();

    let mut methods = Vec::new();
    for method in object.get("methods")?.as_array()? {
        let method_name = method.get("name").and_then(Value::as_str)?.to_owned();
        let signature = method.get("signature");
        let params = signature
            .and_then(|signature| signature.get("params"))
            .and_then(Value::as_array)
            .map(|params| {
                params
                    .iter()
                    .map(|param| TraitParamRecord {
                        name: param
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                        type_name: param
                            .get("type")
                            .map(type_ref_text)
                            .unwrap_or_else(|| "()".to_owned()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let return_type = signature
            .and_then(|signature| signature.get("return"))
            .map(type_ref_text)
            .unwrap_or_else(|| "()".to_owned());
        methods.push(TraitMethodRecord {
            name: method_name,
            params,
            return_type,
            doc: method
                .get("doc")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|text| !text.is_empty()),
            has_default: method
                .get("defaultBody")
                .and_then(Value::as_str)
                .is_some_and(|body| !body.trim().is_empty()),
        });
    }

    Some(TraitRecord {
        id,
        path: path.to_owned(),
        name,
        display_name,
        description,
        builtin,
        contract_version,
        supertraits,
        methods,
        type_index_id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn canonical() -> Value {
        json!({
            "schemaVersion": 1,
            "typeKind": "trait",
            "name": "Movable",
            "displayName": "Movable",
            "supertraits": ["types/traits/base.trait.json", "abc"],
            "methods": [{
                "name": "move_by",
                "signature": {
                    "params": [{"name": "dx", "type": {"kind": "Primitive", "name": "f32"}}],
                    "return": {"kind": "Primitive", "name": "()"}
                },
                "defaultBody": "{}"
            }],
            "meta": {"builtin": true, "contractVersion": 3}
        })
    }

    #[test]
    fn parses_methods_supertraits_and_builtin_stamp() {
        let record =
            parse_trait_record(&canonical(), "types/traits/movable.trait.json", "id1".into())
                .unwrap();
        assert_eq!(record.methods[0].params[0].type_name, "f32");
        assert_eq!(record.methods[0].return_type, "()");
        assert!(record.methods[0].has_default);
        assert!(record.builtin);
        assert_eq!(record.contract_version, 3);
        assert_eq!(record.supertraits, ["abc", "types/traits/base.trait.json"]);
    }

    #[test]
    fn accepts_the_legacy_template_and_rejects_partial_envelopes() {
        let legacy = json!({"name": "T", "display_name": "T", "methods": []});
        assert!(valid_trait_declaration(&legacy));
        let partial = json!({"schemaVersion": 1, "name": "T", "methods": []});
        assert!(!valid_trait_declaration(&partial));
        assert!(parse_trait_record(&partial, "t.trait.json", "x".into()).is_none());
    }

    #[test]
    fn paths_normalize_and_reject_escapes() {
        assert_eq!(
            normalize_relative_path("types\\traits\\a.trait.json").as_deref(),
            Some("types/traits/a.trait.json")
        );
        assert!(normalize_relative_path("../a.trait.json").is_none());
        assert!(normalize_relative_path("/a.trait.json").is_none());
    }

    #[test]
    fn contract_comparison_ignores_documentation() {
        let a = parse_trait_record(&canonical(), "a.trait.json", "1".into()).unwrap();
        let mut b = a.clone();
        b.methods[0].doc = Some("changed".into());
        b.description = Some("changed".into());
        assert!(a.same_contract(&b));
        b.methods[0].return_type = "bool".into();
        assert!(!a.same_contract(&b));
    }

    #[test]
    fn blueprint_files_with_editor_header_comments_parse() {
        let saved = "// Auto Generated by the Pulsar Blueprint Editor\n// DO NOT EDIT\n//\n\n{\"format_version\": 2}";
        assert_eq!(parse_blueprint_json(saved.as_bytes()).unwrap()["format_version"], 2);
        assert!(parse_blueprint_json(b"{\"a\":1}").is_some());
        assert!(parse_blueprint_json(b"// only a comment").is_none());
        assert!(parse_blueprint_json(b"not json").is_none());
    }
}
