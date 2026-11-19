//! Languages as plugin contributions.
//!
//! One registration per language, [`Language`], carries everything the host
//! needs to know about it: its identity and the files that are its own (file
//! extensions, the marker files of its classes), an optional script compiler
//! ([`ScriptLanguage`]) and an optional language server ([`LanguageServerSpec`]).
//! A plugin adds it from its root's `build` (`p.add(Language { .. })`); the
//! host only hosts and routes: [`LanguageRegistry`] answers "which language
//! owns this file" and "which languages' classes is this directory", the
//! editor's language-server service asks it which server a file belongs to,
//! the packager which compiler. The host names no language, and a language
//! is added by adding its plugin: no host or other plugin changes.
//!
//! A disabled plugin's `Language` is absent (the registry follows
//! `pulsar_plugins::generation()`), so its markers no longer make a directory
//! a class and its server is no longer routed to.
//!
//! This crate is GPUI-free so the class registry (`pulsar_class`), the
//! packager and command-line tools use it without the editor.

pub mod registry;
pub mod script;
pub mod settings;
pub mod spec;

use std::path::Path;
use std::sync::Arc;

pub use registry::LanguageRegistry;
pub use script::{
    CheckContext, ComponentEventMetadata, CompileDiagnostic, CookOptimization, DiagnosticSeverity,
    NativeRegistry, ScriptLanguage,
};
pub use settings::{server_settings_schema, ServerSettings, StartSetting, TraceLevel};
pub use spec::{
    CommandContext, CommandResult, InstallContext, LanguageServerSpec, LocateContext, LocateError,
    ProgressEvent, ProgressKind, ProgressText, QuickFix, QuickFixEdit, ReadinessRule, RootStrategy,
    ServerCommand, ServerCommandSpec, ServerContext, ServerHooks, ServerInstaller, ServerSignal,
};

#[doc(hidden)]
pub use pulsar_plugins;

/// Everything one language brings, registered once from a plugin's root
/// `build`.
pub struct Language {
    /// The package that submitted it (`pulsar_plugins::origin!()`, or
    /// `engine_origin!()` for an engine crate).
    pub origin: &'static str,
    /// Stable id: the module store's `language` marker, the compiler's
    /// selector, the default LSP `languageId`.
    pub id: &'static str,
    /// What a person reads (`TypeScript`).
    pub display_name: &'static str,
    /// Extensions of the language's source files, without the dot, lower
    /// case (`["ts", "mts", "cts"]`).
    pub file_extensions: &'static [&'static str],
    /// The LSP `languageId` per extension when it is not [`Language::id`]
    /// (`("tsx", "typescriptreact")`).
    pub language_ids: &'static [(&'static str, &'static str)],
    /// A grammar id of the UI highlighter's table (the foundation layer owns
    /// the table; this names an entry). Empty: no highlighting of its own.
    pub highlight_id: &'static str,
    /// Files whose presence in a directory makes it one of this language's
    /// classes. Empty: the language has no classes.
    pub class_markers: &'static [&'static str],
    /// Project files that change this language's server configuration
    /// (`Cargo.toml`, `tsconfig.json`).
    pub config_files: &'static [&'static str],
    /// A compiler to engine bytecode. `None`: not a scripting language.
    pub compiler: Option<fn() -> Arc<dyn ScriptLanguage>>,
    /// A language server. `None`: highlighting only.
    pub server: Option<&'static LanguageServerSpec>,
}

impl pulsar_plugins::Contribution for Language {
    const CATEGORY: &'static str = "language";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        self.id.to_string()
    }

    fn label(&self) -> String {
        self.display_name.to_string()
    }
}

pulsar_plugins::registration!(Language);

impl Language {
    /// Whether `path` is one of this language's source files (its extension
    /// is one of [`Language::file_extensions`]).
    pub fn handles(&self, path: &Path) -> bool {
        extension_len(self.file_extensions, path).is_some()
    }

    /// The LSP `languageId` of `path`.
    pub fn language_id_of(&self, path: &Path) -> &'static str {
        let name = file_name_lower(path);
        self.language_ids
            .iter()
            .filter(|(extension, _)| ends_with_extension(&name, extension))
            .max_by_key(|(extension, _)| extension.len())
            .map(|(_, id)| *id)
            .unwrap_or(self.id)
    }

    /// Whether `dir` holds one of this language's class markers.
    pub fn owns_class_dir(&self, dir: &Path) -> bool {
        self.class_markers
            .iter()
            .any(|marker| engine_fs::virtual_fs::is_file(&dir.join(marker)))
    }
}

fn file_name_lower(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

fn ends_with_extension(lower_name: &str, extension: &str) -> bool {
    lower_name.len() > extension.len() + 1
        && lower_name.ends_with(extension)
        && lower_name.as_bytes()[lower_name.len() - extension.len() - 1] == b'.'
}

/// The length of the longest of `extensions` that `path` ends with.
pub(crate) fn extension_len(extensions: &[&str], path: &Path) -> Option<usize> {
    let name = file_name_lower(path);
    extensions
        .iter()
        .filter(|extension| ends_with_extension(&name, extension))
        .map(|extension| extension.len())
        .max()
}

/// Registers a language that has only class markers, for tests of crates that
/// must not depend on a plugin: `submit_test_language!("id", ["marker.json"])`
/// at module level of a test. It is an engine registration (always enabled).
#[macro_export]
macro_rules! submit_test_language {
    ($id:expr, [$($marker:expr),* $(,)?]) => {
        $crate::pulsar_plugins::inventory::submit! {
            $crate::Language {
                origin: $crate::pulsar_plugins::engine_origin!(),
                id: $id,
                display_name: $id,
                file_extensions: &[],
                language_ids: &[],
                highlight_id: "",
                class_markers: &[$($marker),*],
                config_files: &[],
                compiler: None,
                server: None,
            }
        }
    };
}

/// The standard settings group every language server gets (D10), derived from
/// the registrations: `enabled`, `path`, `args`, `initialization_options`,
/// `trace`, `start`, `idle_stop_minutes`. Registered under the language's own
/// plugin while it is enabled; the plugin adds nothing for it.
fn server_settings_groups() -> Vec<pulsar_settings::plugin::SettingsGroup> {
    pulsar_plugins::all_registered::<Language>()
        .filter_map(|language| {
            let spec = language.server?;
            Some(pulsar_settings::plugin::SettingsGroup {
                origin: language.origin,
                id: spec.id,
                schema: Arc::new(move || {
                    server_settings_schema(
                        spec.display_name,
                        &format!(
                            "The {} language server: completion, diagnostics and navigation for {} files",
                            spec.display_name, language.display_name
                        ),
                    )
                }),
            })
        })
        .collect()
}

pulsar_plugins::inventory::submit! {
    pulsar_settings::plugin::SettingsGroups {
        origin: pulsar_plugins::engine_origin!(),
        id: "language.servers",
        groups: server_settings_groups,
    }
}

impl std::fmt::Debug for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Language")
            .field("id", &self.id)
            .field("origin", &self.origin)
            .field("compiler", &self.compiler.is_some())
            .field("server", &self.server.map(|spec| spec.id))
            .finish()
    }
}
