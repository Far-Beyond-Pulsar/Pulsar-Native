//! Compiling and verifying a project's script classes headlessly (#879).
//!
//! 1. Every scripting language linked into this tool compiles the project
//!    ([`ScriptLanguage::compile_project`](plugin_editor_api::ScriptLanguage::compile_project)):
//!    Blueprints with the `blueprint` feature.
//! 2. Every class's compiled module is then loaded into a script runtime
//!    with the engine's full native registry, event hub and the project's
//!    capability allowlist, which verifies and links it exactly as the game
//!    will. Any error fails the build.

use std::path::{Path, PathBuf};

use pulsar_class::{ClassEntry, ClassRegistry};
use pulsar_content::ProjectSettings;
use pulsar_script_vm::Module;
use serde::Serialize;

/// One problem with a project's scripts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ScriptProblem {
    pub error: bool,
    pub class: Option<String>,
    pub file: Option<PathBuf>,
    pub message: String,
}

impl std::fmt::Display for ScriptProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.error { "error" } else { "warning" })?;
        if let Some(class) = &self.class {
            write!(f, " [{class}]")?;
        }
        if let Some(file) = &self.file {
            write!(f, " {}", file.display())?;
        }
        write!(f, ": {}", self.message)
    }
}

/// A class and its compiled module, ready to ship.
#[derive(Clone, Debug)]
pub struct CompiledClass {
    pub entry: ClassEntry,
    /// `None` for a class with no script (prefab only).
    pub module: Option<Module>,
}

/// What compiling a project produced.
#[derive(Debug, Default)]
pub struct CompileOutput {
    pub classes: Vec<CompiledClass>,
    pub problems: Vec<ScriptProblem>,
    /// Languages that compiled sources (empty: only existing modules were
    /// used).
    pub languages: Vec<String>,
}

impl CompileOutput {
    pub fn has_errors(&self) -> bool {
        self.problems.iter().any(|p| p.error)
    }
}

/// Compile every class of every linked language, then load and link every
/// class's module against the engine. `settings` supplies the capability
/// allowlist of `settings.profile`.
pub fn compile_project(project: &Path, settings: &ProjectSettings) -> CompileOutput {
    let mut output = CompileOutput::default();
    let natives_runtime = pulsar_game::scripting::new_runtime();
    run_languages(project, natives_runtime.natives(), &mut output);

    let registry = ClassRegistry::scan(project);
    let mut modules: Vec<(ClassEntry, Option<Module>)> = Vec::new();
    for entry in registry.entries() {
        let problem = |message: String, file: Option<PathBuf>| ScriptProblem {
            error: true,
            class: Some(entry.name.clone()),
            file,
            message,
        };
        let build = entry.dir.join("events").join(".build");
        let json = build.join(pulsar_game::scripting::MODULE_JSON_FILE);
        let graph = entry.dir.join("graph_save.json");
        if !json.is_file() {
            if graph.is_file() {
                output.problems.push(problem(
                    "has a graph but no compiled module; open it in the editor and save, or build this \
                     tool with the `blueprint` feature to compile it here"
                        .into(),
                    Some(graph),
                ));
            }
            modules.push((entry.clone(), None));
            continue;
        }
        if is_older(&json, &graph) {
            output.problems.push(ScriptProblem {
                error: false,
                class: Some(entry.name.clone()),
                file: Some(json.clone()),
                message: "compiled module is older than its graph; it may be stale".into(),
            });
        }
        match std::fs::read(&json).map_err(|e| e.to_string()).and_then(|b| Module::decode(&b).map_err(|e| e.to_string())) {
            Ok(module) => {
                if module.name != entry.name {
                    output.problems.push(problem(
                        format!("module is named `{}`, not after its class directory", module.name),
                        Some(json),
                    ));
                }
                modules.push((entry.clone(), Some(module)));
            }
            Err(error) => output.problems.push(problem(format!("unreadable module: {error}"), Some(json))),
        }
    }

    link_all(&registry, &modules, settings, &mut output);
    output.classes = modules.into_iter().map(|(entry, module)| CompiledClass { entry, module }).collect();
    output
}

/// Load every module into one runtime, as the game does: declared events
/// first (classes may use each other's), then each class links.
fn link_all(
    registry: &ClassRegistry,
    modules: &[(ClassEntry, Option<Module>)],
    settings: &ProjectSettings,
    output: &mut CompileOutput,
) {
    let mut runtime = pulsar_game::scripting::new_runtime();
    runtime.set_capabilities(pulsar_game::scripting::capability_policy(settings));
    let events = pulsar_game::scripting::ScriptEvents::new(pulsar_events::EventHub::new());
    for entry in registry.entries() {
        events.bridge().add_class(&entry.name, entry.id.as_str());
    }
    runtime.set_event_host(Some(events.host()));
    for (entry, module) in modules {
        let Some(module) = module else { continue };
        if let Err(error) = runtime.declare_events(module) {
            output.problems.push(ScriptProblem {
                error: true,
                class: Some(entry.name.clone()),
                file: None,
                message: error.to_string(),
            });
        }
    }
    for (entry, module) in modules {
        let Some(module) = module else { continue };
        if let Err(error) = runtime.load_class(module.clone()) {
            output.problems.push(ScriptProblem {
                error: true,
                class: Some(entry.name.clone()),
                file: None,
                message: error.to_string(),
            });
        }
    }
}

fn is_older(a: &Path, b: &Path) -> bool {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    matches!((modified(a), modified(b)), (Some(a), Some(b)) if a < b)
}

#[cfg(feature = "blueprint")]
fn run_languages(project: &Path, natives: &pulsar_script_vm::NativeRegistry, output: &mut CompileOutput) {
    use plugin_editor_api::ScriptLanguage;
    let languages: Vec<std::sync::Arc<dyn ScriptLanguage>> = vec![blueprint_editor_plugin::script_language()];
    for language in languages {
        tracing::info!(language = language.display_name(), "Compiling scripts");
        for diagnostic in language.compile_project(project, natives) {
            output.problems.push(ScriptProblem {
                error: diagnostic.is_error(),
                class: diagnostic.class.clone(),
                file: diagnostic.file.clone(),
                message: match &diagnostic.location {
                    Some(location) => format!("{} ({location})", diagnostic.message),
                    None => diagnostic.message.clone(),
                },
            });
        }
        output.languages.push(language.id().to_owned());
    }
}

#[cfg(not(feature = "blueprint"))]
fn run_languages(_project: &Path, _natives: &pulsar_script_vm::NativeRegistry, _output: &mut CompileOutput) {
    tracing::warn!(
        "No scripting language compilers are linked into this build of the packager (feature `blueprint`); \
         using the classes' existing compiled modules"
    );
}
