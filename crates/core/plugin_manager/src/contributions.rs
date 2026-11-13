//! What plugins and built-ins contribute to the host, and how it is collected.
//!
//! Registration of built-in editors, queries over
//! statusbar buttons, script languages and component definitions, and the
//! one-shot `drain_*` calls the engine backend makes at startup.

use crate::{BuiltinEditorRegistry, PluginManager};
use plugin_editor_api::*;
use std::sync::Arc;

impl PluginManager {
    /// Get a mutable reference to the built-in editor registry.
    ///
    /// This allows external code to register built-in editors during initialization.
    pub fn builtin_registry_mut(&mut self) -> &mut BuiltinEditorRegistry {
        &mut self.builtin_registry
    }

    /// Register all built-in editors with the file type and editor registries.
    ///
    /// This should be called after all built-in editors have been registered
    /// with the builtin registry.
    pub fn register_builtin_editors(&mut self) {
        self.builtin_registry
            .register_all(&mut self.file_type_registry, &mut self.editor_registry);
    }

    /// Get all registered statusbar buttons from all plugins.
    ///
    /// Buttons are sorted by position (left/right) and priority within each position.
    /// Higher priority buttons appear first in their respective position group.
    ///
    /// Returns only the button definitions, not the plugin IDs.
    pub fn get_statusbar_buttons(&self) -> Vec<&StatusbarButtonDefinition> {
        self.statusbar_buttons.iter().map(|(_, btn)| btn).collect()
    }

    /// Get statusbar buttons for a specific position.
    pub fn get_statusbar_buttons_for_position(
        &self,
        position: StatusbarPosition,
    ) -> Vec<&StatusbarButtonDefinition> {
        self.statusbar_buttons
            .iter()
            .filter(|(_, btn)| btn.position == position)
            .map(|(_, btn)| btn)
            .collect()
    }

    // ========================================================================
    // Component Registration (#269)
    // ========================================================================

    /// Every scripting language from loaded plugins and built-in providers.
    pub fn script_languages(&self) -> Vec<Arc<dyn plugin_editor_api::ScriptLanguage>> {
        let mut languages: Vec<Arc<dyn plugin_editor_api::ScriptLanguage>> = self
            .plugins
            .values()
            .flat_map(|loaded| loaded.plugin.script_languages())
            .collect();
        languages.extend(self.builtin_registry.get_all_script_languages());
        languages
    }

    /// Run every scripting language's pre-Play validation on
    /// `project_root`. `Err` joins the failures.
    pub fn validate_scripts(&self, project_root: &std::path::Path) -> Result<(), String> {
        let component_events: Vec<_> = pulsar_world_registry::component_event_registrations()
            .map(|registration| plugin_editor_api::ComponentEventMetadata {
                component_class: registration.class_name.to_owned(),
                event: (registration.declaration)(),
            })
            .collect();
        let failures: Vec<String> = self
            .script_languages()
            .iter()
            .filter_map(|lang| {
                lang.validate_project_with_component_events(project_root, &component_events)
                    .err()
                    .map(|e| format!("{}: {e}", lang.display_name()))
            })
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("\n"))
        }
    }

    /// Get all component definitions registered by all plugins and built-in providers.
    pub fn get_all_component_definitions(&self) -> Vec<ComponentDefinition> {
        let mut all_defs = Vec::new();

        for loaded in self.plugins.values() {
            all_defs.extend(loaded.plugin.component_definitions());
        }

        for (_, def) in self.builtin_registry.get_all_components() {
            all_defs.push(def);
        }

        all_defs
    }

    /// Drain all plugin-provided component registrations (consumes them for engine registration).
    ///
    /// Each entry is `(component_class_name, factory)`.
    /// Injected into `EngineBackend::plugin_components` at startup.
    pub fn drain_component_registrations(
        &mut self,
    ) -> Vec<(String, plugin_editor_api::ComponentFactory)> {
        std::mem::take(&mut self.plugin_component_registrations)
    }
}
