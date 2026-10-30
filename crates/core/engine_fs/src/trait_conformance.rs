//! Does an asset provide what the traits it declares require?
//!
//! Declaring a trait on a Blueprint used to be metadata only. This module
//! checks the declaration: every method of the trait (and of its
//! supertraits) without a default body must be provided, with a compatible
//! signature. It works on plain values so any editor can call it, the
//! Blueprint editor at compile time and project validation at preflight.
//!
//! A Blueprint provides a method with a *custom event* (`local_events`) or
//! a *macro* (`subgraphs`) of the same name. Names compare as snake case, so
//! `OnGameStart`, `On Game Start` and `on_game_start` are one name.
//!
//! The check is conservative, because a false error blocks a build:
//! parameter *names* are not compared, only their count and types; numeric
//! types are one class (width and signedness are not compared, since the
//! Blueprint type palette does not distinguish them reliably); a type that
//! cannot be compared (a generic, a tuple, a macro pin whose type is not
//! recorded, `any`) matches anything. A Blueprint that declares traits only
//! by path (saved before trait ids existed; see
//! [`BlueprintTraitEntry::path_only`](crate::BlueprintTraitEntry)) is held to the same
//! rules but its issues are warnings until it is saved again, so an old
//! project does not stop building on the day it gains the check.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::blueprint_trait_index::{declared_from_value, entry_for};
use crate::trait_db::TraitDb;
use crate::trait_records::TraitMethodRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    MissingMethod,
    ParamCount,
    ParamType,
    ReturnType,
    UnresolvedTrait,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceIssue {
    pub severity: Severity,
    pub kind: IssueKind,
    pub trait_name: String,
    pub method: Option<String>,
    pub message: String,
}

/// One parameter an asset's function takes. `type_name` is `None` when the
/// asset's format does not record it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvidedParam {
    pub name: String,
    pub type_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvidedReturn {
    Unit,
    /// Not recorded (an event node: the event system has no return value).
    /// Matches any trait return type.
    Unknown,
    /// Returns a value; the type is `None` when not recorded.
    Value(Option<String>),
}

/// A function an asset provides, in any language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvidedMethod {
    pub name: String,
    pub params: Vec<ProvidedParam>,
    pub returns: ProvidedReturn,
}

/// The functions a Blueprint (as its JSON asset) provides: custom events and
/// macros.
pub fn provided_methods(blueprint: &Value) -> Vec<ProvidedMethod> {
    let mut methods = Vec::new();
    let mut seen = BTreeSet::new();
    let mut push = |method: ProvidedMethod| {
        if seen.insert(normalize_name(&method.name)) {
            methods.push(method);
        }
    };

    let events = blueprint
        .get("local_events")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .chain(
            blueprint
                .pointer("/main_graph/custom_event_defs")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|defs| defs.values()),
        );
    for event in events {
        let Some(name) = event.get("name").and_then(Value::as_str) else {
            continue;
        };
        let params = event
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|field| ProvidedParam {
                name: field
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                type_name: field
                    .get("type_name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
            .collect();
        let return_type = event
            .get("return_type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let returns = if matches!(type_class(return_type), TypeClass::Unit) {
            ProvidedReturn::Unit
        } else {
            ProvidedReturn::Value(Some(return_type.to_owned()))
        };
        push(ProvidedMethod {
            name: name.to_owned(),
            params,
            returns,
        });
    }

    for graph in blueprint
        .get("subgraphs")
        .or_else(|| blueprint.get("local_macros"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if graph.get("kind").and_then(Value::as_str) == Some("collapsed") {
            continue;
        }
        let Some(name) = graph.get("name").and_then(Value::as_str) else {
            continue;
        };
        let pins = |side: &str| -> Vec<&Value> {
            graph
                .pointer(&format!("/interface/{side}"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|pin| pin.get("data_type").and_then(Value::as_str) != Some("Execution"))
                .collect()
        };
        let params = pins("inputs")
            .into_iter()
            .map(|pin| ProvidedParam {
                name: pin
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                type_name: None,
            })
            .collect();
        let returns = match pins("outputs").len() {
            0 => ProvidedReturn::Unit,
            _ => ProvidedReturn::Value(None),
        };
        push(ProvidedMethod {
            name: name.to_owned(),
            params,
            returns,
        });
    }

    // Placed entry-point nodes for a trait method (`event::on::<Class>.<name>`,
    // what the Blueprint editor's Add menu creates for a declared trait): the
    // node is the implementation, in the main graph or a macro.
    let graphs = std::iter::once(blueprint.get("main_graph")).chain(
        blueprint
            .get("subgraphs")
            .or_else(|| blueprint.get("local_macros"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|graph| graph.get("graph")),
    );
    for graph in graphs.flatten() {
        let nodes = graph.get("nodes").and_then(Value::as_object);
        for node in nodes.into_iter().flat_map(|nodes| nodes.values()) {
            let Some(event) = node
                .get("node_type")
                .and_then(Value::as_str)
                .and_then(|node_type| node_type.strip_prefix("event::on::"))
            else {
                continue;
            };
            let Some((_, method)) = event.rsplit_once('.') else {
                continue;
            };
            // Outputs other than the exec `Body` pin are the parameters.
            let outputs: Vec<&Value> = match node.get("outputs") {
                Some(Value::Array(pins)) => pins.iter().collect(),
                Some(Value::Object(pins)) => pins.values().collect(),
                _ => Vec::new(),
            };
            let params = outputs
                .into_iter()
                .filter_map(|pin| pin.get("id").and_then(Value::as_str))
                .filter(|id| *id != "Body")
                .map(|id| ProvidedParam {
                    name: id.to_owned(),
                    type_name: None,
                })
                .collect();
            push(ProvidedMethod {
                name: method.to_owned(),
                params,
                returns: ProvidedReturn::Unknown,
            });
        }
    }
    methods
}

/// Check the Blueprint JSON against the traits it declares. A Blueprint of
/// an unsupported format, or one declaring nothing, has no issues.
pub fn check_blueprint(db: &TraitDb, blueprint: &Value) -> Vec<ConformanceIssue> {
    let Some(declared) = declared_from_value(blueprint) else {
        return Vec::new();
    };
    if declared.ids.is_empty() && declared.paths.is_empty() {
        return Vec::new();
    }
    let entry = entry_for(db, "", &declared);
    let severity = if entry.path_only {
        Severity::Warning
    } else {
        Severity::Error
    };
    let mut issues: Vec<ConformanceIssue> = entry
        .unresolved
        .iter()
        .map(|reference| ConformanceIssue {
            severity,
            kind: IssueKind::UnresolvedTrait,
            trait_name: reference.clone(),
            method: None,
            message: format!(
                "Declared trait '{reference}' does not exist in this project. Remove it or restore the trait asset."
            ),
        })
        .collect();
    issues.extend(check(
        db,
        &entry.implemented_trait_ids,
        &provided_methods(blueprint),
        severity,
    ));
    issues
}

/// Check that `provided` satisfies every trait in `declared_ids`.
/// Issues carry `severity`.
pub fn check(
    db: &TraitDb,
    declared_ids: &[String],
    provided: &[ProvidedMethod],
    severity: Severity,
) -> Vec<ConformanceIssue> {
    let by_name: BTreeMap<String, &ProvidedMethod> = provided
        .iter()
        .map(|method| (normalize_name(&method.name), method))
        .collect();
    let mut issues = Vec::new();
    let mut checked = BTreeSet::new();
    for id in declared_ids {
        let Some(declared) = db.get(id) else {
            continue;
        };
        for (owner, required) in db.required_methods(id) {
            // The same method required through two declared traits is one
            // obligation.
            if !checked.insert((owner.id.clone(), required.name.clone())) {
                continue;
            }
            let trait_name = if owner.id == declared.id {
                owner.name.clone()
            } else {
                format!("{} (required by {})", owner.name, declared.name)
            };
            let Some(found) = by_name.get(&normalize_name(&required.name)) else {
                if !required.has_default {
                    issues.push(ConformanceIssue {
                        severity,
                        kind: IssueKind::MissingMethod,
                        trait_name,
                        method: Some(required.name.clone()),
                        message: format!(
                            "Trait '{}' requires method `{}`, but this Blueprint has no custom event or macro named `{}`.",
                            owner.name,
                            signature_text(required),
                            required.name
                        ),
                    });
                }
                continue;
            };
            issues.extend(compare(&trait_name, required, found, severity));
        }
    }
    issues
}

fn compare(
    trait_name: &str,
    required: &TraitMethodRecord,
    found: &ProvidedMethod,
    severity: Severity,
) -> Vec<ConformanceIssue> {
    let issue = |kind, message: String| ConformanceIssue {
        severity,
        kind,
        trait_name: trait_name.to_owned(),
        method: Some(required.name.clone()),
        message,
    };
    if required.params.len() != found.params.len() {
        return vec![issue(
            IssueKind::ParamCount,
            format!(
                "`{}` takes {} parameter(s) but the trait declares `{}` ({}).",
                found.name,
                found.params.len(),
                signature_text(required),
                required.params.len()
            ),
        )];
    }
    let mut issues = Vec::new();
    for (index, (want, have)) in required.params.iter().zip(&found.params).enumerate() {
        if let Some(have_type) = &have.type_name {
            if !compatible(&want.type_name, have_type) {
                issues.push(issue(
                    IssueKind::ParamType,
                    format!(
                        "Parameter {} of `{}` is `{}` but the trait declares `{}: {}`.",
                        index + 1,
                        found.name,
                        have_type,
                        want.name,
                        want.type_name
                    ),
                ));
            }
        }
    }
    let wants_unit = matches!(type_class(&required.return_type), TypeClass::Unit);
    let mismatch = match (&found.returns, wants_unit) {
        (ProvidedReturn::Unknown, _) => None,
        (ProvidedReturn::Unit, true) => None,
        (ProvidedReturn::Unit, false) => Some(format!(
            "`{}` returns nothing but the trait declares `-> {}`.",
            found.name, required.return_type
        )),
        (ProvidedReturn::Value(_), true) => Some(format!(
            "`{}` returns a value but the trait declares it returns nothing.",
            found.name
        )),
        (ProvidedReturn::Value(Some(have)), false) if !compatible(&required.return_type, have) => {
            Some(format!(
                "`{}` returns `{}` but the trait declares `-> {}`.",
                found.name, have, required.return_type
            ))
        }
        (ProvidedReturn::Value(_), false) => None,
    };
    issues.extend(mismatch.map(|message| issue(IssueKind::ReturnType, message)));
    issues
}

/// `name(a: T, b: U) -> R`, as a trait author would write it.
pub fn signature_text(method: &TraitMethodRecord) -> String {
    let params = method
        .params
        .iter()
        .map(|param| format!("{}: {}", param.name, param.type_name))
        .collect::<Vec<_>>()
        .join(", ");
    if matches!(type_class(&method.return_type), TypeClass::Unit) {
        format!("{}({params})", method.name)
    } else {
        format!("{}({params}) -> {}", method.name, method.return_type)
    }
}

/// One line per issue, errors first, for a compiler message.
pub fn format_issues(issues: &[ConformanceIssue]) -> String {
    let mut sorted: Vec<&ConformanceIssue> = issues.iter().collect();
    sorted.sort_by_key(|issue| issue.severity == Severity::Warning);
    sorted
        .iter()
        .map(|issue| {
            let level = match issue.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            };
            format!("{level}: [{}] {}", issue.trait_name, issue.message)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Snake case with every run of non-alphanumerics collapsed:
/// `OnGameStart`, `On Game Start` and `on-game-start` are `on_game_start`.
pub fn normalize_name(name: &str) -> String {
    let mut out = String::new();
    let mut previous_lower = false;
    for character in name.trim().chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase() && previous_lower {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
            previous_lower = character.is_ascii_lowercase() || character.is_ascii_digit();
        } else {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            previous_lower = false;
        }
    }
    out.trim_end_matches('_').to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TypeClass {
    Unit,
    Number,
    Bool,
    Text,
    /// Cannot be compared: matches anything.
    Any,
    Named(String),
}

fn type_class(text: &str) -> TypeClass {
    let compact: String = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .trim_start_matches('&')
        .to_owned();
    let lower = compact.to_ascii_lowercase();
    match lower.as_str() {
        "" | "()" | "void" | "execution" | "unit" => TypeClass::Unit,
        "bool" | "boolean" => TypeClass::Bool,
        "string" | "str" | "char" => TypeClass::Text,
        "number" | "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32"
        | "u64" | "u128" | "usize" | "f32" | "f64" | "int" | "float" => TypeClass::Number,
        "?" | "any" | "dyn any" | "dynany" | "object" => TypeClass::Any,
        _ if lower.contains(['<', '(', '[', '?']) || lower.starts_with("dyn") => TypeClass::Any,
        _ => TypeClass::Named(
            lower
                .rsplit("::")
                .next()
                .unwrap_or(lower.as_str())
                .to_owned(),
        ),
    }
}

fn compatible(want: &str, have: &str) -> bool {
    let (want, have) = (type_class(want), type_class(have));
    want == TypeClass::Any || have == TypeClass::Any || want == have
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trait_records::{TraitParamRecord, TraitRecord};
    use serde_json::json;

    fn method(name: &str, params: &[(&str, &str)], ret: &str, default: bool) -> TraitMethodRecord {
        TraitMethodRecord {
            name: name.into(),
            params: params
                .iter()
                .map(|(n, t)| TraitParamRecord { name: (*n).into(), type_name: (*t).into() })
                .collect(),
            return_type: ret.into(),
            doc: None,
            has_default: default,
        }
    }

    fn trait_record(id: &str, name: &str, supers: &[&str], methods: Vec<TraitMethodRecord>) -> TraitRecord {
        TraitRecord {
            id: id.into(),
            path: format!("types/traits/{name}.trait.json"),
            name: name.into(),
            display_name: name.into(),
            description: None,
            builtin: false,
            contract_version: 0,
            supertraits: supers.iter().map(|s| s.to_string()).collect(),
            methods,
            type_index_id: None,
        }
    }

    fn db() -> TraitDb {
        TraitDb::build(
            vec![
                trait_record("base", "Base", &[], vec![method("tick", &[("dt", "f32")], "()", false)]),
                trait_record(
                    "mode",
                    "Mode",
                    &["base"],
                    vec![
                        method("on_game_start", &[], "()", false),
                        method("score", &[("who", "u32")], "i32", false),
                        method("optional", &[], "()", true),
                    ],
                ),
            ],
            Vec::new(),
        )
    }

    fn blueprint(ids: &[&str], paths: &[&str], events: Value, subgraphs: Value) -> Value {
        json!({
            "format_version": 1, "main_graph": {}, "variables": [],
            "local_events": events, "subgraphs": subgraphs,
            "blueprint_metadata": {"implemented_trait_ids": ids, "implemented_traits": paths}
        })
    }

    fn event(name: &str, fields: &[(&str, &str)], ret: &str) -> Value {
        json!({
            "uid": name, "name": name, "return_type": ret,
            "fields": fields.iter().map(|(n, t)| json!({"name": n, "type_name": t})).collect::<Vec<_>>()
        })
    }

    fn full() -> Value {
        blueprint(
            &["mode"],
            &[],
            json!([
                event("On Game Start", &[], ""),
                event("score", &[("player", "number")], "number"),
            ]),
            json!([{
                "id": "m", "kind": "macro", "name": "Tick",
                "interface": {
                    "inputs": [{"id": "e", "name": "In", "data_type": "Execution"},
                               {"id": "d", "name": "dt", "data_type": {"Data": {}}}],
                    "outputs": [{"id": "o", "name": "Out", "data_type": "Execution"}]
                }
            }]),
        )
    }

    #[test]
    fn a_blueprint_providing_every_method_conforms() {
        assert_eq!(check_blueprint(&db(), &full()), Vec::new());
    }

    #[test]
    fn names_compare_as_snake_case() {
        assert_eq!(normalize_name("OnGameStart"), "on_game_start");
        assert_eq!(normalize_name("On Game Start"), "on_game_start");
        assert_eq!(normalize_name("on-game--start "), "on_game_start");
        assert_eq!(normalize_name("HTTPServer2"), "httpserver2");
    }

    #[test]
    fn missing_methods_are_errors_including_inherited_ones_but_not_defaults() {
        let bp = blueprint(&["mode"], &[], json!([event("on_game_start", &[], "")]), json!([]));
        let issues = check_blueprint(&db(), &bp);
        let missing: Vec<_> = issues.iter().filter(|i| i.kind == IssueKind::MissingMethod).map(|i| i.method.clone().unwrap()).collect();
        assert_eq!(missing, ["score", "tick"]);
        assert!(issues.iter().all(|i| i.severity == Severity::Error));
        assert!(issues[0].message.contains("score(who: u32) -> i32"));
        assert!(issues.iter().any(|i| i.trait_name == "Base (required by Mode)"));
    }

    #[test]
    fn signature_mismatches_are_reported() {
        let bp = blueprint(
            &["mode"],
            &[],
            json!([
                event("on_game_start", &[("extra", "i32")], ""),
                event("score", &[("who", "string")], ""),
                event("tick", &[("dt", "f32")], "bool"),
            ]),
            json!([]),
        );
        let kinds: Vec<_> = check_blueprint(&db(), &bp).iter().map(|i| (i.method.clone().unwrap(), i.kind)).collect();
        assert!(kinds.contains(&("on_game_start".into(), IssueKind::ParamCount)));
        assert!(kinds.contains(&("score".into(), IssueKind::ParamType)));
        assert!(kinds.contains(&("score".into(), IssueKind::ReturnType)));
        assert!(kinds.contains(&("tick".into(), IssueKind::ReturnType)));
    }

    #[test]
    fn uncomparable_types_and_numeric_widths_do_not_cause_false_errors() {
        assert!(compatible("u32", "number"));
        assert!(compatible("f32", "f64"));
        assert!(compatible("Vec<f32>", "array<number>"));
        assert!(compatible("my::Thing", "Thing"));
        assert!(compatible("u32", "any"));
        assert!(!compatible("u32", "bool"));
        assert!(!compatible("String", "number"));
        assert!(!compatible("my::Thing", "Other"));
    }

    #[test]
    fn legacy_path_only_declarations_are_warnings() {
        let bp = blueprint(&[], &["types/traits/Mode.trait.json"], json!([]), json!([]));
        let issues = check_blueprint(&db(), &bp);
        assert!(!issues.is_empty());
        assert!(issues.iter().all(|i| i.severity == Severity::Warning));
    }

    #[test]
    fn unresolved_declarations_are_reported() {
        let bp = blueprint(&["ghost"], &[], json!([]), json!([]));
        let issues = check_blueprint(&db(), &bp);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, IssueKind::UnresolvedTrait);
        assert_eq!(issues[0].severity, Severity::Error);
    }

    #[test]
    fn blueprints_declaring_nothing_or_in_other_formats_are_not_checked() {
        assert!(check_blueprint(&db(), &blueprint(&[], &[], json!([]), json!([]))).is_empty());
        assert!(check_blueprint(&db(), &json!({"format_version": 9})).is_empty());
    }

    #[test]
    fn collapsed_subgraphs_do_not_count_as_functions() {
        let bp = blueprint(
            &["mode"],
            &[],
            json!([]),
            json!([{"id": "c", "kind": "collapsed", "name": "on_game_start", "interface": {"inputs": [], "outputs": []}}]),
        );
        assert!(check_blueprint(&db(), &bp).iter().any(|i| i.method.as_deref() == Some("on_game_start")));
    }


    #[test]
    fn a_placed_trait_event_node_provides_its_method() {
        let mut bp = blueprint(&["mode"], &[], json!([]), json!([]));
        bp["main_graph"] = json!({"nodes": {
            "n1": {"id": "n1", "node_type": "event::on::Player.on_game_start", "outputs": [{"id": "Body"}]},
            "n2": {"id": "n2", "node_type": "event::on::Player.score", "outputs": [{"id": "Body"}, {"id": "who"}]},
            "n3": {"id": "n3", "node_type": "event::on::Player.tick", "outputs": {"Body": {"id": "Body"}, "dt": {"id": "dt"}}},
        }});
        // The event system has no return value, so `score -> i32` is not
        // judged on it; parameters still count.
        assert_eq!(check_blueprint(&db(), &bp), Vec::new());

        bp["main_graph"]["nodes"]["n2"]["outputs"] = json!([{"id": "Body"}]);
        let issues = check_blueprint(&db(), &bp);
        assert!(issues.iter().any(|i| i.kind == IssueKind::ParamCount && i.method.as_deref() == Some("score")));
    }
    #[test]
    fn formats_errors_before_warnings() {
        let issues = vec![
            ConformanceIssue { severity: Severity::Warning, kind: IssueKind::MissingMethod, trait_name: "A".into(), method: None, message: "w".into() },
            ConformanceIssue { severity: Severity::Error, kind: IssueKind::MissingMethod, trait_name: "B".into(), method: None, message: "e".into() },
        ];
        assert_eq!(format_issues(&issues), "error: [B] e\nwarning: [A] w");
    }
}
