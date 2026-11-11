//! Does an implementer provide what the traits it declares require?
//!
//! Every method of a declared trait (and of its supertraits) without a
//! default body must be provided, with a compatible signature. The check
//! works on [`ProvidedMethod`]s, which each implementer source builds from
//! its own format: the Blueprint plugin from a Blueprint's events and macros
//! (at compile and preflight), the native source from a component's
//! registered methods (`MethodMetadata`), the script runtime from a compiled
//! class's handlers.
//!
//! Names compare as snake case, so `OnGameStart`, `On Game Start` and
//! `on_game_start` are one name. The check is conservative, because a false
//! error blocks a build: parameter *names* are not compared, only their count
//! and types; numeric types are one class (width and signedness are not
//! compared); a type that cannot be compared (a generic, a tuple, an
//! unrecorded type, `any`) matches anything.

use std::collections::{BTreeMap, BTreeSet};

use crate::db::TraitDb;
use crate::records::TraitMethodRecord;

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
                            "Trait '{}' requires method `{}`, but its implementer provides no method named `{}`.",
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

/// Whether a type spelling means "returns nothing" (`()`, empty, `void`):
/// what a source maps to [`ProvidedReturn::Unit`].
pub fn is_unit_type(text: &str) -> bool {
    matches!(type_class(text), TypeClass::Unit)
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
    use crate::records::{TraitParamRecord, TraitRecord};

    fn method(name: &str, params: &[(&str, &str)], ret: &str, default: bool) -> TraitMethodRecord {
        TraitMethodRecord {
            name: name.into(),
            params: params
                .iter()
                .map(|(n, t)| TraitParamRecord {
                    name: (*n).into(),
                    type_name: (*t).into(),
                })
                .collect(),
            return_type: ret.into(),
            doc: None,
            has_default: default,
        }
    }

    fn trait_record(
        id: &str,
        name: &str,
        supers: &[&str],
        methods: Vec<TraitMethodRecord>,
    ) -> TraitRecord {
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
                trait_record(
                    "base",
                    "Base",
                    &[],
                    vec![method("tick", &[("dt", "f32")], "()", false)],
                ),
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

    fn provided(name: &str, params: &[&str], returns: ProvidedReturn) -> ProvidedMethod {
        ProvidedMethod {
            name: name.to_owned(),
            params: params
                .iter()
                .map(|ty| ProvidedParam {
                    name: String::new(),
                    type_name: Some((*ty).to_owned()),
                })
                .collect(),
            returns,
        }
    }

    #[test]
    fn missing_methods_are_errors_including_inherited_ones_but_not_defaults() {
        let issues = check(
            &db(),
            &["mode".to_owned()],
            &[provided("on_game_start", &[], ProvidedReturn::Unit)],
            Severity::Error,
        );
        let missing: Vec<_> = issues
            .iter()
            .filter(|i| i.kind == IssueKind::MissingMethod)
            .map(|i| i.method.clone().unwrap())
            .collect();
        assert_eq!(missing, ["score", "tick"]);
        assert!(issues[0].message.contains("score(who: u32) -> i32"));
        assert!(issues
            .iter()
            .any(|i| i.trait_name == "Base (required by Mode)"));
    }

    #[test]
    fn signature_mismatches_are_reported() {
        let issues = check(
            &db(),
            &["mode".to_owned()],
            &[
                provided("on_game_start", &["i32"], ProvidedReturn::Unit),
                provided("score", &["string"], ProvidedReturn::Unit),
                provided("tick", &["f32"], ProvidedReturn::Value(Some("bool".into()))),
            ],
            Severity::Error,
        );
        let kinds: Vec<_> = issues
            .iter()
            .map(|i| (i.method.clone().unwrap(), i.kind))
            .collect();
        assert!(kinds.contains(&("on_game_start".into(), IssueKind::ParamCount)));
        assert!(kinds.contains(&("score".into(), IssueKind::ParamType)));
        assert!(kinds.contains(&("score".into(), IssueKind::ReturnType)));
        assert!(kinds.contains(&("tick".into(), IssueKind::ReturnType)));
    }

    #[test]
    fn names_compare_as_snake_case() {
        assert_eq!(normalize_name("OnGameStart"), "on_game_start");
        assert_eq!(normalize_name("On Game Start"), "on_game_start");
        assert_eq!(normalize_name("on-game--start "), "on_game_start");
        assert_eq!(normalize_name("HTTPServer2"), "httpserver2");
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
    fn formats_errors_before_warnings() {
        let issues = vec![
            ConformanceIssue {
                severity: Severity::Warning,
                kind: IssueKind::MissingMethod,
                trait_name: "A".into(),
                method: None,
                message: "w".into(),
            },
            ConformanceIssue {
                severity: Severity::Error,
                kind: IssueKind::MissingMethod,
                trait_name: "B".into(),
                method: None,
                message: "e".into(),
            },
        ];
        assert_eq!(format_issues(&issues), "error: [B] e\nwarning: [A] w");
    }
}
