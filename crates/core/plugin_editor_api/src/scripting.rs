//! Scripting languages provided by plugins.
//!
//! The engine runs one bytecode format (`pulsar_script_vm` modules); every
//! scripting language is a plugin that compiles its sources to it. This
//! extension is how the editor finds those languages without depending on
//! any of them, e.g. to validate a project's scripts before Play.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::plugin::EditorPlugin;

pub use pulsar_script_vm::NativeRegistry;

/// How bad a [`CompileDiagnostic`] is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    /// The class does not compile; the project does not build.
    #[default]
    Error,
    /// Worth reporting; the class still compiled.
    Warning,
}

/// One problem found compiling a project's scripts headlessly
/// ([`ScriptLanguage::compile_project`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CompileDiagnostic {
    pub severity: DiagnosticSeverity,
    /// The class (its directory name), when the problem is in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The source file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// The graph node (visual languages) or `line:column` (text).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub message: String,
}

impl CompileDiagnostic {
    pub fn error(class: Option<String>, file: Option<PathBuf>, message: impl Into<String>) -> Self {
        Self { severity: DiagnosticSeverity::Error, class, file, location: None, message: message.into() }
    }

    pub fn is_error(&self) -> bool {
        self.severity == DiagnosticSeverity::Error
    }
}

impl std::fmt::Display for CompileDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.severity {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
        })?;
        if let Some(class) = &self.class {
            write!(f, " [{class}]")?;
        }
        if let Some(file) = &self.file {
            write!(f, " {}", file.display())?;
        }
        if let Some(location) = &self.location {
            write!(f, " ({location})")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// One scripting language (Blueprints, TypeScript, ..).
pub trait ScriptLanguage: Send + Sync {
    /// Stable identifier, e.g. `"blueprint"`.
    fn id(&self) -> &str;

    fn display_name(&self) -> &str;

    /// Check every class of this language under `project_root` before
    /// Play. `Err` carries a human-readable summary and blocks Play.
    fn validate_project(&self, project_root: &Path) -> Result<(), String> {
        let _ = project_root;
        Ok(())
    }

    /// Validate with component event metadata discovered by the host. The
    /// default keeps existing scripting plugins source compatible.
    fn validate_project_with_component_events(
        &self,
        project_root: &Path,
        component_events: &[crate::ComponentEventMetadata],
    ) -> Result<(), String> {
        let _ = component_events;
        self.validate_project(project_root)
    }

    /// Compile every class of this language under `project_root` to engine
    /// script modules (each class's `events/.build/module.json`), with no
    /// GPUI app or editor state: for CI and packaging (#879). Native calls
    /// are resolved against `natives`, the registry the game will link
    /// against. Returns every problem found; any
    /// [error](CompileDiagnostic::is_error) means the project does not
    /// build. The default compiles nothing.
    fn compile_project(&self, project_root: &Path, natives: &NativeRegistry) -> Vec<CompileDiagnostic> {
        let _ = (project_root, natives);
        Vec::new()
    }

    /// Headless compile with the host's component event catalog. This
    /// explicit input works for both editor DLLs and packaging tools, whose
    /// local link-time inventories may not contain host component types.
    fn compile_project_with_component_events(
        &self,
        project_root: &Path,
        natives: &NativeRegistry,
        component_events: &[crate::ComponentEventMetadata],
    ) -> Vec<CompileDiagnostic> {
        let _ = component_events;
        self.compile_project(project_root, natives)
    }
}

/// Plugins that provide scripting languages. The default provides none; a
/// DLL plugin opts in with `export_plugin!(MyPlugin, scripting)`.
pub trait EditorPluginScripting: EditorPlugin {
    fn script_languages(&self) -> Vec<Arc<dyn ScriptLanguage>> {
        Vec::new()
    }
}

/// A scripting language that registers itself at link time.
///
/// A language crate submits one
/// (`plugin_editor_api::inventory::submit! { LinkedScriptLanguage { create } }`)
/// and both the editor and headless tools (`pulsar package`, CI) find it with
/// [`linked_script_languages`] without naming the crate. Which languages a
/// build has is then exactly which language crates it links, and the editor
/// and the packager cannot disagree about a language's id or compiler.
pub struct LinkedScriptLanguage {
    pub create: fn() -> Arc<dyn ScriptLanguage>,
}

inventory::collect!(LinkedScriptLanguage);

pub use inventory;

/// Every linked scripting language, by id (a language linked twice counts
/// once).
pub fn linked_script_languages() -> Vec<Arc<dyn ScriptLanguage>> {
    let mut languages: Vec<Arc<dyn ScriptLanguage>> = inventory::iter::<LinkedScriptLanguage>.into_iter().map(|l| (l.create)()).collect();
    languages.sort_by(|a, b| a.id().cmp(b.id()));
    languages.dedup_by(|a, b| a.id() == b.id());
    languages
}
