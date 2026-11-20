//! The scripting side of a [`Language`](crate::Language): a compiler from a
//! language's sources to engine bytecode (`pulsar_script_vm` modules).
//!
//! The engine runs one bytecode format; every scripting language is a plugin
//! that compiles its sources to it. A plugin hands its compiler to the host as
//! the `compiler` of its one `Language` registration; the editor (pre-Play
//! validation) and the headless tools (`pulsar package`, CI) find it there
//! without depending on any language.

use std::path::{Path, PathBuf};

use pulsar_script_vm::EventDecl;
pub use pulsar_script_vm::NativeRegistry;

/// A component event exposed to visual scripting editors and compilers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentEventMetadata {
    /// Registered reflected component class name.
    pub component_class: String,
    /// Stable event name and unchanged local payload field types.
    pub event: EventDecl,
}

/// Requested tradeoff for plugin-owned cooking and script compilation.
///
/// This is a request, not a promise that every plugin has two distinct
/// backends. A plugin may use the same output for both levels or report that
/// it cannot produce the requested output.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CookOptimization {
    /// Favor low cook latency and incremental iteration.
    #[default]
    Fast,
    /// Favor runtime performance and production-ready output.
    Optimized,
}

/// How bad a [`CompileDiagnostic`] is.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
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
        Self {
            severity: DiagnosticSeverity::Error,
            class,
            file,
            location: None,
            message: message.into(),
        }
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

/// What the host gives a live check ([`ScriptLanguage::check_source`]).
pub struct CheckContext<'a> {
    /// The project the file belongs to.
    pub project_root: &'a Path,
    /// The registry the game links against: the checker resolves native calls
    /// the way a compile does.
    pub natives: &'a NativeRegistry,
}

/// The compiler of one scripting language (Blueprints, TypeScript, ..): what a
/// [`Language`](crate::Language) carries as its `compiler`. The language's
/// id and display name are the registration's, not the compiler's.
pub trait ScriptLanguage: Send + Sync {
    /// What the class in `class_dir` asks to be compiled to besides engine
    /// bytecode, if anything: `Some("rust")` generates it into the game crate
    /// (the project's Cargo sync). The language reads it from wherever it keeps
    /// the setting; `Err` is a problem to report, not a failure. The default:
    /// the class runs as a script.
    fn compile_target(&self, class_dir: &Path) -> Result<Option<String>, String> {
        let _ = class_dir;
        Ok(None)
    }

    /// Whether the editor should run [`ScriptLanguage::check_source`] as the
    /// person types. A language reads its own plugin setting here; the host
    /// clears the language's compiler diagnostics when it turns false.
    fn live_check_enabled(&self) -> bool {
        true
    }

    /// Diagnostics for one file's *unsaved* text, without writing anything:
    /// what the compiler would say about `text` as the content of `path`.
    /// The host calls it on an editor task, debounced, on edit, open and save,
    /// and publishes the result under the compiler source, beside the
    /// language server's (a server accepts constructs the compiler rejects).
    /// `location` is `line:column`, one-based. A file that is not one of the
    /// language's compiled sources yields nothing. The default: no live check.
    fn check_source(&self, ctx: &CheckContext, path: &Path, text: &str) -> Vec<CompileDiagnostic> {
        let _ = (ctx, path, text);
        Vec::new()
    }

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
        component_events: &[ComponentEventMetadata],
    ) -> Result<(), String> {
        let _ = component_events;
        self.validate_project(project_root)
    }

    /// Compile every class of this language under `project_root` to engine
    /// script modules (the project's module store, `pulsar_class::modules`), with no
    /// GPUI app or editor state: for CI and packaging (#879). Native calls
    /// are resolved against `natives`, the registry the game will link
    /// against. Returns every problem found; any
    /// [error](CompileDiagnostic::is_error) means the project does not
    /// build. The default compiles nothing.
    fn compile_project(
        &self,
        project_root: &Path,
        natives: &NativeRegistry,
    ) -> Vec<CompileDiagnostic> {
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
        component_events: &[ComponentEventMetadata],
    ) -> Vec<CompileDiagnostic> {
        let _ = component_events;
        self.compile_project(project_root, natives)
    }

    /// Compile using a package cook's optimization request. Existing
    /// languages inherit their previous behavior until they opt into
    /// mode-specific compilation.
    fn compile_project_for_cook(
        &self,
        project_root: &Path,
        natives: &NativeRegistry,
        component_events: &[ComponentEventMetadata],
        optimization: CookOptimization,
    ) -> Vec<CompileDiagnostic> {
        let _ = optimization;
        self.compile_project_with_component_events(project_root, natives, component_events)
    }
}
