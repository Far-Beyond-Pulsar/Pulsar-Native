//! Compiling and verifying a project's script classes headlessly (#879).
//!
//! 1. Every scripting language linked into this tool compiles the project
//!    ([`ScriptLanguage::compile_project`](plugin_editor_api::ScriptLanguage::compile_project)):
//!    each language is a plugin linked with the `scripting` feature.
//! 2. Every class's compiled module is then loaded into a script runtime
//!    with the engine's full native registry, event hub and the project's
//!    capability allowlist, which verifies and links it exactly as the game
//!    will. Any error fails the build.

use std::path::{Path, PathBuf};

use engine_fs::virtual_fs;
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
pub fn compile_project(
    project: &Path,
    settings: &ProjectSettings,
    optimization: plugin_editor_api::CookOptimization,
) -> CompileOutput {
    let mut output = CompileOutput::default();
    // A project from before the module store keeps modules beside its
    // classes: move them in before anything compiles or reads them.
    pulsar_class::modules::migrate_legacy_modules(project, &ClassRegistry::scan(project));
    let registry = ClassRegistry::scan(project);
    // The one-language rule is the host's: a class that is two languages' (or
    // whose module another wrote) fails the build, and no language compiles it.
    output.problems.extend(registry.problems().iter().map(|problem| ScriptProblem {
        error: true,
        class: Some(problem.class().to_owned()),
        file: Some(problem.file()),
        message: problem.to_string(),
    }));
    let duplicate_names = duplicate_class_names(&registry);
    if !duplicate_names.is_empty() {
        output.problems.extend(duplicate_names);
        return output;
    }
    let natives_runtime = pulsar_game::scripting::new_runtime();
    #[cfg(feature = "scripting")]
    let component_events: Vec<_> = pulsar_world_registry::component_event_registrations()
        .map(|registration| plugin_editor_api::ComponentEventMetadata {
            component_class: registration.class_name.to_owned(),
            event: (registration.declaration)(),
        })
        .collect();
    #[cfg(feature = "scripting")]
    run_languages(
        project,
        natives_runtime.natives(),
        &component_events,
        optimization,
        &mut output,
    );
    #[cfg(not(feature = "scripting"))]
    run_languages(
        project,
        natives_runtime.natives(),
        optimization,
        &mut output,
    );

    let store = pulsar_class::ModuleStore::of_project(project);
    let languages = pulsar_language::LanguageRegistry::current();
    let mut modules: Vec<(ClassEntry, Option<Module>)> = Vec::new();
    for entry in registry.entries() {
        let problem = |message: String, file: Option<PathBuf>| ScriptProblem {
            error: true,
            class: Some(entry.name.clone()),
            file,
            message,
        };
        let json = store.module_path(&entry.id);
        // The source file of the class's language: the marker the registry
        // (the languages linked and enabled) says it has.
        let language = languages.class_languages(&entry.dir).into_iter().next();
        let source = language.and_then(|language| {
            language
                .class_markers
                .iter()
                .map(|marker| entry.dir.join(marker))
                .find(|path| virtual_fs::is_file(path))
                .map(|path| (language, path))
        });
        if !virtual_fs::is_file(&json) {
            if let Some((language, source)) = &source {
                output.problems.push(problem(
                    format!(
                        "has {} source but no compiled module: open it in the editor and save, or \
                         link the {} language into this tool to compile it here",
                        source.file_name().and_then(|n| n.to_str()).unwrap_or("its"),
                        language.display_name
                    ),
                    Some(source.clone()),
                ));
            }
            modules.push((entry.clone(), None));
            continue;
        }
        if let Some((_, source)) = &source {
            if is_older(&json, source) {
                output.problems.push(ScriptProblem {
                    error: false,
                    class: Some(entry.name.clone()),
                    file: Some(json.clone()),
                    message: "compiled module is older than its source; it may be stale".into(),
                });
            }
        }
        match virtual_fs::read_file(&json)
            .map_err(|e| e.to_string())
            .and_then(|b| Module::decode(&b).map_err(|e| e.to_string()))
        {
            Ok(module) => {
                if module.name != entry.name {
                    output.problems.push(problem(
                        format!(
                            "module is named `{}`, not after its class directory",
                            module.name
                        ),
                        Some(json),
                    ));
                }
                modules.push((entry.clone(), Some(module)));
            }
            Err(error) => output
                .problems
                .push(problem(format!("unreadable module: {error}"), Some(json))),
        }
    }

    link_all(project, &registry, &modules, settings, &mut output);
    output.classes = modules
        .into_iter()
        .map(|(entry, module)| CompiledClass { entry, module })
        .collect();
    output
}

/// Load every module into one runtime, as the game does: declared events
/// first (classes may use each other's), then each class links.
fn link_all(
    project: &Path,
    registry: &ClassRegistry,
    modules: &[(ClassEntry, Option<Module>)],
    settings: &ProjectSettings,
    output: &mut CompileOutput,
) {
    let mut runtime = pulsar_game::scripting::new_runtime();
    runtime.set_capabilities(pulsar_game::scripting::capability_policy(settings));
    let events = pulsar_game::scripting::ScriptEvents::new(pulsar_events::EventHub::new());
    if let Err(error) = pulsar_world_registry::register_component_events(events.bridge().hub()) {
        tracing::error!(%error, "could not register component events for package validation");
    }
    // Run the same project-data event declarations as a game session before
    // declaring or linking any class modules. This lets plugins validate
    // handlers for events defined by project assets (for example input
    // actions) using the runtime's normal registration path.
    for hook in engine_backend::scene::game_hooks::game_sessions() {
        if let Some(begin) = hook.begin {
            begin(events.bridge().hub(), project);
        }
    }
    for registration in pulsar_world_registry::component_event_registrations() {
        let declaration = (registration.declaration)();
        if let Err(error) = events
            .bridge()
            .register_event_decl(registration.class_name, &declaration)
        {
            output.problems.push(ScriptProblem {
                error: true,
                class: Some(registration.class_name.to_owned()),
                file: None,
                message: error,
            });
        }
    }
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

fn duplicate_class_names(registry: &ClassRegistry) -> Vec<ScriptProblem> {
    let mut by_name = std::collections::BTreeMap::<&str, Vec<&Path>>::new();
    for entry in registry.entries() {
        by_name
            .entry(&entry.name)
            .or_default()
            .push(entry.dir.as_path());
    }
    by_name
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(name, paths)| ScriptProblem {
            error: true,
            class: Some(name.to_owned()),
            file: None,
            message: format!(
                "class name is ambiguous; multiple class folders have this name: {}",
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        })
        .collect()
}

fn is_older(a: &Path, b: &Path) -> bool {
    let modified = |p: &Path| {
        virtual_fs::metadata(p)
            .ok()
            .and_then(|m| m.modified_nanos())
    };
    matches!((modified(a), modified(b)), (Some(a), Some(b)) if a < b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_class_names_report_every_conflicting_folder() {
        let registry = ClassRegistry::from_entries(vec![
            ClassEntry {
                id: pulsar_class::ClassId::from("first"),
                name: "NewBlueprintClass".into(),
                dir: PathBuf::from("classes/NewBlueprintClass.class"),
            },
            ClassEntry {
                id: pulsar_class::ClassId::from("second"),
                name: "NewBlueprintClass".into(),
                dir: PathBuf::from("src/classes/NewBlueprintClass.class"),
            },
        ]);

        let problems = duplicate_class_names(&registry);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].error);
        assert_eq!(problems[0].class.as_deref(), Some("NewBlueprintClass"));
        assert!(
            problems[0]
                .message
                .contains("classes/NewBlueprintClass.class")
        );
        assert!(
            problems[0]
                .message
                .contains("src/classes/NewBlueprintClass.class")
        );
        assert!(duplicate_class_names(&ClassRegistry::default()).is_empty());
    }
}

// The languages linked into this tool: `scripting` links the installed set's
// editor tools (`lib.rs` links the set), and each language registers itself
// (one `pulsar_language::Language` carrying its compiler), so this names none of them and
// finds exactly the ids and compilers the editor uses.
#[cfg(feature = "scripting")]
fn run_languages(
    project: &Path,
    natives: &pulsar_script_vm::NativeRegistry,
    component_events: &[plugin_editor_api::ComponentEventMetadata],
    optimization: plugin_editor_api::CookOptimization,
    output: &mut CompileOutput,
) {
    for (language, compiler) in pulsar_language::LanguageRegistry::current().compilers() {
        tracing::info!(language = language.display_name, "Compiling scripts");
        for diagnostic in
            compiler.compile_project_for_cook(project, natives, component_events, optimization)
        {
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
        output.languages.push(language.id.to_owned());
    }
}

#[cfg(not(feature = "scripting"))]
fn run_languages(
    _project: &Path,
    _natives: &pulsar_script_vm::NativeRegistry,
    _optimization: plugin_editor_api::CookOptimization,
    _output: &mut CompileOutput,
) {
    tracing::warn!(
        "No scripting language compilers are linked into this build of the packager (features `blueprint`, `typescript`); \
         using the classes' existing compiled modules"
    );
}
