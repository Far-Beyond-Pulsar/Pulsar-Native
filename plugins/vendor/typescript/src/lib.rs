//! TypeScript as a scripting language.
//!
//! A class is a directory `src/classes/<Class>/` holding `class.ts`. This
//! plugin compiles it (with `pulsar_script_ts`) to the same
//! `events/.build/module.json` a Blueprint class produces, so the engine
//! loads, links, runs and hot-reloads it exactly like one, and keeps each
//! field's identity in `class.schema.json` next to the source (commit it).
//!
//! It also writes `Pulsar/types/pulsar.d.ts`, generated from the same native
//! registry the compiler checks against, so an editor gets autocomplete that
//! cannot disagree with the compiler.
//!
//! # One language per class
//!
//! `events/.build/language` records which language wrote a class's module. A
//! class that has both `class.ts` and `graph_save.json`, or whose module was
//! written by another language, is refused instead of overwritten.
//!
//! This crate is the root of the `pulsar.typescript` plugin, an editor tool:
//! its root's `build` registers the language
//! (`plugin_editor_api::LinkedScriptLanguage`, for headless tools) and its
//! editor provider (`plugin_manager::PluginEditorProvider`), both gated by
//! the plugin, so a project that disables it has no TypeScript.

mod compile;
mod provider;

pub use compile::{
    compile_class_dir, compile_project_classes, validate_project_classes, CLASS_FILE, SCHEMA_FILE,
};
pub use provider::{script_language, TypeScriptLanguage};

// The plugin's descriptor: linking this crate installs it.
pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// What the plugin registers (this crate is its root): its provider, which
/// the editor asks for the language's validation, and the language itself,
/// which the packager finds without the editor.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(plugin_manager::PluginEditorProvider {
        origin: p.origin(),
        id: provider::PROVIDER_ID,
        create: || std::sync::Arc::new(provider::TypeScriptProvider),
    });

    p.add(plugin_editor_api::LinkedScriptLanguage {
        origin: p.origin(),
        id: compile::LANGUAGE_ID,
        create: script_language,
    });
}
