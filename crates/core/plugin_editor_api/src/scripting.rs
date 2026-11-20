//! Scripting languages provided by plugins.
//!
//! The engine runs one bytecode format (`pulsar_script_vm` modules); every
//! scripting language is a plugin that compiles its sources to it. A plugin
//! contributes one [`Language`](pulsar_language::Language) from its root's
//! `build`, and that registration carries the compiler ([`ScriptLanguage`]),
//! so the editor finds the language without depending on any of them, e.g. to
//! validate a project's scripts before Play. The types are defined in
//! `pulsar_language` (the kernel crate the host routes through) and re-exported
//! here for plugin authors.

pub use pulsar_language::{
    CompileDiagnostic, CookOptimization, DiagnosticSeverity, Language, LanguageRegistry,
    NativeRegistry, ScriptLanguage,
};
