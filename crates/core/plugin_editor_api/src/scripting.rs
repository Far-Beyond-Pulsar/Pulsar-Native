//! Scripting languages provided by plugins.
//!
//! The engine runs one bytecode format (`pulsar_script_vm` modules); every
//! scripting language is a plugin that compiles its sources to it. This
//! extension is how the editor finds those languages without depending on
//! any of them, e.g. to validate a project's scripts before Play.

use std::path::Path;
use std::sync::Arc;

use crate::plugin::EditorPlugin;

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
}

/// Plugins that provide scripting languages. The default provides none; a
/// DLL plugin opts in with `export_plugin!(MyPlugin, scripting)`.
pub trait EditorPluginScripting: EditorPlugin {
    fn script_languages(&self) -> Vec<Arc<dyn ScriptLanguage>> {
        Vec::new()
    }
}
