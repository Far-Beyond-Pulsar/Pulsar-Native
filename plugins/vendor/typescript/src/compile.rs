//! Compiling a project's TypeScript classes.

use std::path::{Path, PathBuf};

use plugin_editor_api::{CompileDiagnostic, DiagnosticSeverity, NativeRegistry};
use pulsar_script_ts::{compile_class, ClassSchema, ClassSource, Severity};

pub const CLASS_FILE: &str = "class.ts";
pub const SCHEMA_FILE: &str = "class.schema.json";
pub(crate) const LANGUAGE_ID: &str = "typescript";
const LANGUAGE_MARKER: &str = "language";
const GRAPH_FILE: &str = "graph_save.json";

/// The directory name is the class name.
fn class_name(dir: &Path) -> Option<String> {
    dir.file_name().and_then(|n| n.to_str()).map(str::to_owned)
}

/// Every `src/classes/<Class>/` of the project at `root` that has a `class.ts`.
fn class_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root.join("src").join("classes"))
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join(CLASS_FILE).is_file())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

/// Whether this language may compile `dir`: see the crate docs.
fn check_language(dir: &Path) -> Result<(), String> {
    if dir.join(GRAPH_FILE).is_file() {
        return Err(format!(
            "this class has both `{CLASS_FILE}` (TypeScript) and `{GRAPH_FILE}` (Blueprint): a class has one language, so remove one of them"
        ));
    }
    match std::fs::read_to_string(dir.join("events").join(".build").join(LANGUAGE_MARKER)) {
        Ok(other) if !other.trim().is_empty() && other.trim() != LANGUAGE_ID => Err(format!(
            "its compiled module was written by the `{}` language; delete `events/.build` to compile it as TypeScript",
            other.trim()
        )),
        _ => Ok(()),
    }
}

fn diagnostic(class: &str, file: &Path, message: impl Into<String>) -> CompileDiagnostic {
    CompileDiagnostic::error(Some(class.to_owned()), Some(file.to_path_buf()), message)
}

/// Compile `dir` (a class directory). With `write`, the module, language
/// marker and updated schema are written; without, nothing is touched
/// (validation before Play). Problems are returned either way.
pub fn compile_class_dir(
    dir: &Path,
    natives: &NativeRegistry,
    write: bool,
) -> Vec<CompileDiagnostic> {
    let Some(class) = class_name(dir) else {
        return Vec::new();
    };
    let file = dir.join(CLASS_FILE);
    if let Err(message) = check_language(dir) {
        return vec![diagnostic(&class, &file, message)];
    }
    let out = dir.join("events").join(".build").join("module.json");
    let source = match std::fs::read_to_string(&file) {
        Ok(source) => source,
        Err(e) => return vec![diagnostic(&class, &file, format!("failed to read: {e}"))],
    };
    let schema_path = dir.join(SCHEMA_FILE);
    let previous = match std::fs::read_to_string(&schema_path) {
        Ok(text) => match serde_json::from_str::<ClassSchema>(&text) {
            Ok(schema) => Some(schema),
            Err(e) => {
                return vec![diagnostic(
                    &class,
                    &schema_path,
                    format!("`{SCHEMA_FILE}` is not valid: {e}"),
                )]
            }
        },
        Err(_) => None,
    };

    let compiled = compile_class(
        &ClassSource {
            class_name: &class,
            file: CLASS_FILE,
            source: &source,
            schema: previous.as_ref(),
        },
        natives,
    );
    let mut problems: Vec<CompileDiagnostic> = compiled
        .diagnostics
        .iter()
        .map(|d| CompileDiagnostic {
            severity: match d.severity {
                Severity::Error => DiagnosticSeverity::Error,
                Severity::Warning => DiagnosticSeverity::Warning,
            },
            class: Some(class.clone()),
            file: Some(file.clone()),
            location: d.location(),
            message: d.message.clone(),
        })
        .collect();

    let Some(module) = compiled.module else {
        if write {
            // Never leave a module from an older source behind (only our own).
            let _ = std::fs::remove_file(&out);
        }
        return problems;
    };
    if !write {
        return problems;
    }
    let io_error = |what: &str, path: &Path, e: std::io::Error| {
        diagnostic(&class, path, format!("failed to write {what}: {e}"))
    };
    let json = match module.to_json() {
        Ok(json) => json,
        Err(e) => {
            return vec![diagnostic(
                &class,
                &file,
                format!("failed to serialise the module: {e}"),
            )]
        }
    };
    if let Some(build) = out.parent() {
        if let Err(e) = std::fs::create_dir_all(build) {
            problems.push(io_error("`events/.build`", build, e));
            return problems;
        }
        if let Err(e) = std::fs::write(&out, json) {
            problems.push(io_error("the module", &out, e));
            return problems;
        }
        if let Err(e) = std::fs::write(build.join(LANGUAGE_MARKER), LANGUAGE_ID) {
            problems.push(io_error("the language marker", build, e));
        }
    }
    // The schema is the class's field identity: write it when it changed.
    if let Some(schema) = compiled.schema {
        if previous.as_ref() != Some(&schema) {
            match serde_json::to_string_pretty(&schema) {
                Ok(text) => {
                    if let Err(e) = std::fs::write(&schema_path, text + "\n") {
                        problems.push(io_error("the schema", &schema_path, e));
                    }
                }
                Err(e) => problems.push(diagnostic(
                    &class,
                    &schema_path,
                    format!("failed to serialise the schema: {e}"),
                )),
            }
        }
    }
    tracing::info!(class = %dir.display(), "TypeScript class compiled");
    problems
}

/// Compile every TypeScript class of the project at `root` (#879): each
/// `src/classes/<Class>/class.ts` to its `events/.build/module.json`,
/// linking native calls against `natives`, and refresh the generated
/// declarations. Returns every problem found.
pub fn compile_project_classes(root: &Path, natives: &NativeRegistry) -> Vec<CompileDiagnostic> {
    let dirs = class_dirs(root);
    let mut problems = Vec::new();
    for dir in &dirs {
        problems.extend(compile_class_dir(dir, natives, true));
    }
    if !dirs.is_empty() {
        write_declarations(root, natives);
    }
    problems
}

/// Check every TypeScript class without writing anything. `Err` blocks Play.
pub fn validate_project_classes(root: &Path, natives: &NativeRegistry) -> Result<(), String> {
    let errors: Vec<String> = class_dirs(root)
        .iter()
        .flat_map(|dir| compile_class_dir(dir, natives, false))
        .filter(CompileDiagnostic::is_error)
        .map(|d| d.to_string())
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "TypeScript classes have errors:\n{}",
            errors.join("\n")
        ))
    }
}

/// `Pulsar/types/pulsar.d.ts`, when it differs from what the registry says.
fn write_declarations(root: &Path, natives: &NativeRegistry) {
    let text = pulsar_script_ts::declarations(natives);
    let path = root.join("Pulsar").join("types").join("pulsar.d.ts");
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == text) {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, text) {
        tracing::warn!("could not write {}: {e}", path.display());
    }
}
