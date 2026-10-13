#![recursion_limit = "512"]

//! # Input plugin
//!
//! Two asset types and the runtime behind them.
//!
//! * An **input action** (`.inputaction`) is something the player can do:
//!   jump, move, look. It says what it reports (on/off, a number, a 2D or 3D
//!   direction), how that value is reshaped (dead zone, scale, curve...) and
//!   when it fires (on press, after a hold, on a double tap, while another
//!   action is held...).
//! * An **input mapping** (`.inputmap`) binds keys to actions. Each entry
//!   binds one key (keyboard, mouse, controller) to one action, and entries
//!   are sorted into categories for the person editing it. Mappings have a
//!   priority, so a menu's mapping can take input from the game's.
//!
//! Both refer to each other by a stable id kept inside the file, never by
//! name or path, so renaming or moving an action breaks no mapping.
//!
//! ## Reaching scripts and plugins
//!
//! The runtime reads the devices from the engine's event hub (key, mouse and
//! wheel events) and from game controllers, evaluates the active mappings
//! every frame, and publishes what the actions do as Gamma events in the
//! `input` namespace: `input::Jump.triggered`, `input::Move.ongoing`, and so
//! on (see [`runtime::events`]). A `PlayerInputComponent` makes a mapping
//! active for an object and sends its events to that object's channel, so
//! the scripts attached to it hear their own player.
//!
//! ## Layers
//!
//! Everything but `editor` knows nothing about GPUI.
//!
//! | Module | What |
//! |--------|------|
//! | [`keys`] | every bindable key of every device |
//! | [`model`] | ids, triggers and modifiers, the two files, editing operations, validation |
//! | [`library`] | the project's actions and mappings by id: scanning, conflicts, usage |
//! | [`runtime`] | devices, evaluation, the Gamma events, the component |
//! | [`preview`] | what an action does, simulated |
//! | `editor` | the docked editors |

use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;
use plugin_editor_api::*;
use ui::dock::PanelView;

mod editor;
mod file_types;
pub mod keys;
pub mod library;
pub mod model;
pub mod preview;
pub mod runtime;

pub use editor::{ACTION_EDITOR_ID, MAPPING_EDITOR_ID};
pub use file_types::{ACTION_EXT, ACTION_ID, MAPPING_EXT, MAPPING_ID};
pub use runtime::component::{PlayerInputComponent, PLAYER_INPUT_CLASS_NAME};

/// The input plugin.
#[derive(Default)]
pub struct InputPlugin;

impl EditorPlugin for InputPlugin {
    fn metadata(&self) -> PluginMetadata {
        PluginMetadata {
            id: PluginId::new("com.pulsar.input"),
            name: "Input".into(),
            version: "0.1.0".into(),
            author: "Pulsar Team".into(),
            description: "Input actions and mappings for keyboards, mice and controllers, and the editors for them".into(),
        }
    }

    fn file_types(&self) -> Vec<FileTypeDefinition> {
        file_types::file_types()
    }

    fn editors(&self) -> Vec<EditorMetadata> {
        vec![
            EditorMetadata {
                id: EditorId::new(editor::ACTION_EDITOR_ID),
                display_name: "Input Action Editor".into(),
                supported_file_types: vec![FileTypeId::new(file_types::ACTION_ID)],
            },
            EditorMetadata {
                id: EditorId::new(editor::MAPPING_EDITOR_ID),
                display_name: "Input Mapping Editor".into(),
                supported_file_types: vec![FileTypeId::new(file_types::MAPPING_ID)],
            },
        ]
    }

    fn on_load(&mut self) {
        tracing::info!("Input plugin loaded");
    }
}

impl EditorPluginEditor for InputPlugin {
    fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
        // A dynamically loaded plugin gets the host's undo service through
        // the settings snapshot, so every view of a file shares one history.
        registry.register_fn_with_settings(EditorId::new(editor::ACTION_EDITOR_ID), |path, settings, window, cx| {
            Self::create_action_editor(path, Some(&settings.services().undo), window, cx)
        });
        registry.register_fn_with_settings(EditorId::new(editor::MAPPING_EDITOR_ID), |path, settings, window, cx| {
            Self::create_mapping_editor(path, Some(&settings.services().undo), window, cx)
        });
    }
}

impl EditorPluginStatusbar for InputPlugin {}
impl EditorPluginAi for InputPlugin {}
impl EditorPluginComponents for InputPlugin {
    fn component_definitions(&self) -> Vec<ComponentDefinition> {
        vec![ComponentDefinition {
            id: PLAYER_INPUT_CLASS_NAME.into(),
            display_name: "Player Input".into(),
            category: "Input".into(),
            description: "Makes an input mapping active for this object; its actions arrive as events on the object".into(),
            icon: None,
        }]
    }

    fn component_factories(&self) -> Vec<(String, ComponentFactory)> {
        vec![(
            PLAYER_INPUT_CLASS_NAME.into(),
            Box::new(|| Box::new(PlayerInputComponent::default()) as Box<dyn pulsar_reflection::EngineClass>),
        )]
    }
}
impl EditorPluginSubsystems for InputPlugin {
    fn subsystems(&self) -> Vec<Box<dyn Subsystem>> {
        Vec::new()
    }
}

impl InputPlugin {
    /// The editor that opens `path`, by its extension.
    pub fn editor_for_path(path: &std::path::Path) -> Option<EditorId> {
        let extension = path.extension()?.to_str()?;
        if extension.eq_ignore_ascii_case(file_types::ACTION_EXT) {
            Some(EditorId::new(editor::ACTION_EDITOR_ID))
        } else if extension.eq_ignore_ascii_case(file_types::MAPPING_EXT) {
            Some(EditorId::new(editor::MAPPING_EDITOR_ID))
        } else {
            None
        }
    }

    /// Open the action at `path` (for hosts that link this crate). `undo` is
    /// the host's undo service; with it, every view of a file shares one
    /// history.
    pub fn create_action_editor(
        path: PathBuf,
        undo: Option<&UndoService>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let panel = cx.new(|cx| editor::action::ActionEditor::new(path, undo, window, cx));
        Ok(Arc::new(panel))
    }

    /// Open the mapping at `path`.
    pub fn create_mapping_editor(
        path: PathBuf,
        undo: Option<&UndoService>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        let panel = cx.new(|cx| editor::mapping::MappingEditor::new(path, undo, window, cx));
        Ok(Arc::new(panel))
    }

    /// Open `path` in the editor `editor_id`.
    pub fn create_editor(
        editor_id: &EditorId,
        path: PathBuf,
        undo: Option<&UndoService>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Arc<dyn PanelView>, PluginError> {
        if editor_id.as_str() == editor::MAPPING_EDITOR_ID {
            Self::create_mapping_editor(path, undo, window, cx)
        } else {
            Self::create_action_editor(path, undo, window, cx)
        }
    }
}

// Static built-ins are instantiated through the Rust API and must not emit the
// process-global dynamic loader symbols shared by every plugin.
#[cfg(not(feature = "builtin"))]
export_plugin!(InputPlugin);

#[cfg(test)]
mod tests {
    // Explicit imports: `super::*` would pull in gpui's own `test` macro.
    use super::InputPlugin;
    use plugin_editor_api::{EditorPlugin, EditorPluginComponents};
    use std::path::Path;

    #[test]
    fn each_asset_type_opens_in_its_own_editor() {
        let plugin = InputPlugin;
        let editors = plugin.editors();
        assert_eq!(editors.len(), 2);
        let types = plugin.file_types();
        for (editor, file_type) in editors.iter().zip(&types) {
            assert!(editor.supported_file_types.contains(&file_type.id));
        }
        assert_eq!(InputPlugin::editor_for_path(Path::new("input/Jump.inputaction")).unwrap().as_str(), super::ACTION_EDITOR_ID);
        assert_eq!(InputPlugin::editor_for_path(Path::new("Game.INPUTMAP")).unwrap().as_str(), super::MAPPING_EDITOR_ID);
        assert!(InputPlugin::editor_for_path(Path::new("notes.txt")).is_none());
    }

    #[test]
    fn the_component_is_a_registered_world_component_with_a_factory() {
        let plugin = InputPlugin;
        let defs = plugin.component_definitions();
        assert_eq!(defs.len(), 1);
        assert!(pulsar_world_registry::component_id_for_class(&defs[0].id).is_some(), "not registered with the world");
        let factories = plugin.component_factories();
        let (_, make) = factories.iter().find(|(name, _)| *name == defs[0].id).unwrap();
        assert!(!make().get_properties().is_empty());
    }
}

#[cfg(test)]
mod render_tests;
