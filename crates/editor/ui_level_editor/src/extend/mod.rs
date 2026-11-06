//! The level editor's extension points.
//!
//! The level editor is a shell: it knows objects, components, the
//! hierarchy, the properties panel, the viewport, undo and saving, and no
//! kind of object. Everything a kind brings to the editor (its tool modes,
//! its Create menu entries, its World Settings sections, its undo steps,
//! what its save does first, its editor-only level data, its viewport debug
//! overlays, its assistant tools) is a registration its plugin's editor
//! crate submits here:
//!
//! ```ignore
//! inventory::submit! {
//!     ui_level_editor::extend::CreateEntry {
//!         origin: pulsar_plugins::origin!(),
//!         id: "sky",
//!         ..
//!     }
//! }
//! ```
//!
//! Each extension point is one type in this module (its one owner). Every
//! registration names its origin, the package that submitted it, and the
//! shell reads them through [`pulsar_plugins::registered`], which skips a
//! registration whose plugin the project disables: a disabled plugin's
//! modes, entries and sections are absent because they are not registered,
//! not because the shell knows to hide them.
//!
//! Registrations act on components only through the components' own
//! methods (the ones scripts call) and the shell's undoable commands
//! ([`crate::commands::execute_command`]); there is no editor-only path into
//! a component.

mod commands;
pub mod panel_kit;
mod plugin_state;
pub mod viewport;

use std::sync::Arc;

use glam::DVec3;
use pulsar_scenedb::{Entity, World};

pub use commands::{ExtensionCommand, ExtensionUndo};
pub use plugin_state::{PluginState, PluginStates};

use crate::commands::{execute_command, CommandResult, SceneCommand, TypedComponent};
use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;
use crate::tool_modes::ToolMode;

/// Implements [`pulsar_plugins::Registration`] and `inventory::collect!`
/// for an extension point.
macro_rules! extension_point {
    ($($ty:ty),* $(,)?) => {$(
        inventory::collect!($ty);
        impl pulsar_plugins::Registration for $ty {
            fn origin(&self) -> &'static str {
                self.origin
            }
        }
    )*};
}
pub(crate) use extension_point;

/// The registrations of `T` whose plugin is enabled, in `order`.
fn sorted<T: pulsar_plugins::Registration>(order: impl Fn(&T) -> i32) -> Vec<&'static T> {
    let mut all: Vec<&'static T> = pulsar_plugins::registered::<T>().collect();
    all.sort_by_key(|registration| order(registration));
    all
}

// ── Tool modes ──────────────────────────────────────────────────────────────

/// A tool mode a plugin adds to the mode picker (the sculpt tool, the
/// spline tool). The built-in Level Edit mode is the only mode the shell
/// has itself.
pub struct ToolModeRegistration {
    pub origin: &'static str,
    /// Place in the mode picker, after Level Edit.
    pub order: i32,
    pub make: fn() -> Box<dyn ToolMode>,
}

/// Every registered tool mode, in order.
pub fn tool_modes() -> Vec<&'static ToolModeRegistration> {
    sorted::<ToolModeRegistration>(|mode| mode.order)
}

// ── Create entries ──────────────────────────────────────────────────────────

/// Something the user can add to a level: an entry of the Create menu,
/// of the World Settings' environment mixer, or both.
pub struct CreateEntry {
    pub origin: &'static str,
    /// Stable id (`"light_point"`, `"sky"`): other entries and presets
    /// name it ([`create_entry`]), so a plugin can use another's entry
    /// without linking it.
    pub id: &'static str,
    /// The Create menu's text (`None`: not in the Create menu). Plugins
    /// translate their own text.
    pub menu_label: Option<fn() -> String>,
    /// The environment mixer's "Add component" text (`None`: not offered
    /// there).
    pub environment_label: Option<&'static str>,
    /// The hierarchy header's quick-add button: `(icon, tooltip)` (`None`:
    /// no button).
    pub quick_add: Option<(fn() -> ui::IconName, &'static str)>,
    /// Place in the menu and the mixer; the Create menu groups entries by
    /// hundreds (objects 0.., lights 100.., meshes 200.., environment 300..).
    pub order: i32,
    /// Whether the mixer offers it now (`None`: always). One sun: a
    /// directional light is not offered while the level has one.
    pub offered: Option<fn(&World) -> bool>,
    /// Add it as one undoable command; the new object is selected.
    pub create: fn(&mut LevelEditorState) -> CommandResult,
}

/// Every registered create entry, in order.
pub fn create_entries() -> Vec<&'static CreateEntry> {
    sorted::<CreateEntry>(|entry| entry.order)
}

/// The registered create entry `id`, if its plugin is enabled.
pub fn create_entry(id: &str) -> Option<&'static CreateEntry> {
    pulsar_plugins::registered::<CreateEntry>().find(|entry| entry.id == id)
}

/// Run the create entry `id`. A no-op when nothing registers it.
pub fn create(state: &mut LevelEditorState, id: &str) -> CommandResult {
    match create_entry(id) {
        Some(entry) => (entry.create)(state),
        None => CommandResult::noop("Nothing in this project adds that"),
    }
}

/// Add a named object holding `components` at `transform` as one undoable
/// command and select it: what most create entries do.
pub fn add_object(
    state: &mut LevelEditorState,
    name: &str,
    object_type: ObjectType,
    transform: Transform,
    components: Vec<TypedComponent>,
) -> CommandResult {
    let mut transform = transform;
    if let Some(position) = state.pending_create_position {
        transform.position = position;
    }
    let result = execute_command(
        state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: name.to_string(),
                object_type,
                transform,
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            parent_id: None,
            components,
        },
    );
    if let Some(id) = result.affected_ids.first() {
        execute_command(
            state,
            SceneCommand::SelectObject {
                id: Some(id.clone()),
            },
        );
    }
    result
}

// ── Environment mixer ───────────────────────────────────────────────────────

/// A starter setup of the World Settings' environment mixer (a clear day:
/// a sky and a tuned sun).
pub struct EnvironmentPreset {
    pub origin: &'static str,
    pub id: &'static str,
    pub label: &'static str,
    pub order: i32,
    pub apply: fn(&mut LevelEditorState) -> CommandResult,
}

/// Every registered environment preset, in order.
pub fn environment_presets() -> Vec<&'static EnvironmentPreset> {
    sorted::<EnvironmentPreset>(|preset| preset.order)
}

/// Apply the environment preset `id`. A no-op when nothing registers it.
pub fn apply_environment_preset(state: &mut LevelEditorState, id: &str) -> CommandResult {
    match pulsar_plugins::registered::<EnvironmentPreset>().find(|preset| preset.id == id) {
        Some(preset) => (preset.apply)(state),
        None => CommandResult::noop("Nothing in this project offers that setup"),
    }
}

/// A component class the environment mixer's sidebar lists, every instance
/// of it in the level (lights, fog volumes, water).
pub struct EnvironmentListing {
    pub origin: &'static str,
    pub class_name: &'static str,
    pub label: &'static str,
    pub order: i32,
    /// The create entry the sidebar offers while the level has none.
    pub create_entry: Option<&'static str>,
}

/// Every registered environment listing, in order.
pub fn environment_listings() -> Vec<&'static EnvironmentListing> {
    sorted::<EnvironmentListing>(|listing| listing.order)
}

// ── Level components (World Settings sections) ──────────────────────────────

/// One component a level holds at most once (its sky, its global wind),
/// edited in World Settings: its own section shows the component's
/// properties in place (undoable) or, when the level has none, a note and a
/// button that creates it. The environment mixer's sidebar lists it first.
pub struct LevelComponentSection {
    pub origin: &'static str,
    /// The section's title ("Sky").
    pub title: &'static str,
    /// The mixer sidebar's title ("Sky and Atmosphere").
    pub sidebar_title: &'static str,
    /// The mixer sidebar's subtitle while the level has none.
    pub sidebar_missing: &'static str,
    pub order: i32,
    /// The component's class (its properties are shown by reflection).
    pub class_name: &'static str,
    /// The level's component: `(instance, owner)` (the component's own
    /// method, e.g. the sky plugin's `level_atmosphere`).
    pub find: fn(&World) -> Option<(Entity, Entity)>,
    /// What the section says while the level has none.
    pub missing_note: &'static str,
    /// The create button's text.
    pub create_label: &'static str,
    /// Create it (one undoable command).
    pub create: fn(&mut LevelEditorState) -> CommandResult,
    /// What the section says about the one it shows: `(object name, enabled)`.
    pub describe: fn(&str, bool) -> String,
}

/// Every registered level component section, in order.
pub fn level_component_sections() -> Vec<&'static LevelComponentSection> {
    sorted::<LevelComponentSection>(|section| section.order)
}

/// Where a level's component lives: the object holding it and the
/// component's index in that object's list (the properties panel's
/// addressing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelComponent {
    pub object_id: String,
    pub object_name: String,
    pub component_index: usize,
    pub enabled: bool,
}

/// The level's component that `find` finds, addressed for the panels.
pub fn level_component(
    world: &World,
    find: fn(&World) -> Option<(Entity, Entity)>,
) -> Option<LevelComponent> {
    use engine_backend::scene::SceneWorldExt;
    let (instance, owner) = find(world)?;
    let object_id = world.stable_id_of(owner)?.to_string();
    let component_index = engine_backend::scene::attachments::instances(world, owner)
        .iter()
        .position(|entity| *entity == instance)?;
    Some(LevelComponent {
        object_name: world
            .get::<engine_backend::scene::Name>(owner)
            .map(|name| name.0.clone())
            .unwrap_or_else(|| object_id.clone()),
        object_id,
        component_index,
        enabled: engine_backend::scene::attachments::is_enabled(world, instance),
    })
}

// ── Saving and loading ──────────────────────────────────────────────────────

/// Work a level save does first, on the scene it saves (the voxel worlds
/// fold old edits into their journals and keep their drawn chunks).
pub struct SaveHook {
    pub origin: &'static str,
    pub before_save: fn(&engine_backend::scene::SharedScene),
}

/// Run every registered save hook.
pub fn run_save_hooks(scene: &engine_backend::scene::SharedScene) {
    for hook in pulsar_plugins::registered::<SaveHook>() {
        (hook.before_save)(scene);
    }
}

/// Editor-only data a plugin keeps with a level (the foliage palette): a
/// key of the level file's editor section, written on save and read on
/// open. A level saved with the plugin disabled keeps what it had (the
/// section is carried through unread).
pub struct EditorData {
    pub origin: &'static str,
    /// Its key in the level's editor section.
    pub key: &'static str,
    /// What to save (`None`: nothing).
    pub save: fn(&LevelEditorState) -> Option<serde_json::Value>,
    /// Take an opened level's value (`None`: the level has none; reset).
    pub load: fn(&mut LevelEditorState, Option<&serde_json::Value>),
}

/// Every registered editor data section.
pub fn editor_data() -> impl Iterator<Item = &'static EditorData> {
    pulsar_plugins::registered::<EditorData>()
}

/// Hand an opened level's editor section to the plugins' editor data
/// (each gets its key's value, or `None` to reset).
pub fn load_editor_data(
    state: &mut LevelEditorState,
    editor: Option<&crate::scene_edit::LevelEditorFileState>,
) {
    for data in editor_data() {
        (data.load)(
            state,
            editor.and_then(|editor| editor.plugins.get(data.key)),
        );
    }
}

/// Save the level to `path` now, on the calling thread, in order with every
/// other save (what the editor's Save does, hooks and editor data included).
pub fn save_level_now(state: &SharedState, path: &std::path::Path) -> Result<(), String> {
    crate::ui::save::save_now(state, path, None).map(|_| ())
}

// ── Interactions, selection, properties, framing ────────────────────────────

/// An interaction a plugin's tool keeps going across frames (a sculpt
/// stroke). The shell finishes it before undo (`force`) and polls it
/// otherwise, so it becomes one undo step once done.
pub struct Interaction {
    pub origin: &'static str,
    pub in_progress: fn(&LevelEditorState) -> bool,
    pub finish: fn(&mut LevelEditorState, bool),
}

/// Whether any plugin's interaction is in progress.
pub fn interaction_in_progress(state: &LevelEditorState) -> bool {
    pulsar_plugins::registered::<Interaction>().any(|interaction| (interaction.in_progress)(state))
}

/// Finish every plugin's interaction in progress (`force`: now, before an
/// undo; otherwise only those that are done).
pub fn finish_interactions(state: &mut LevelEditorState, force: bool) {
    for interaction in pulsar_plugins::registered::<Interaction>() {
        if (interaction.in_progress)(state) {
            (interaction.finish)(state, force);
        }
    }
}

/// Runs after the selection changes outside the plugin's own panels (an
/// assistant tool selected something): the spline tool follows it.
pub struct SelectionFollowUp {
    pub origin: &'static str,
    pub follow: fn(&mut LevelEditorState),
}

/// Run every registered selection follow-up.
pub fn follow_selection(state: &mut LevelEditorState) {
    for follow_up in pulsar_plugins::registered::<SelectionFollowUp>() {
        (follow_up.follow)(state);
    }
}

/// What follows a successful edit of one property of a class (choosing a
/// voxel terrain's generator attaches its settings). It calls the
/// component's own method, as a script setting the property would.
pub struct PropertyFollowUp {
    pub origin: &'static str,
    pub class_name: &'static str,
    pub property: &'static str,
    /// `(world, owner, instance)`.
    pub follow: fn(&mut World, Entity, Entity),
}

/// Run the follow-ups of `class_name`'s `property`.
pub fn follow_property_edit(
    world: &mut World,
    class_name: &str,
    property: &str,
    owner: Entity,
    instance: Entity,
) {
    for follow_up in pulsar_plugins::registered::<PropertyFollowUp>() {
        if follow_up.class_name == class_name && follow_up.property == property {
            (follow_up.follow)(world, owner, instance);
        }
    }
}

/// How the viewport frames an object a plugin knows better than "a few
/// metres back" (a voxel world: over its ground). `frame(world, object,
/// eye, forward)` returns `None` for an object it does not frame, else the
/// camera's position and forward (or why it cannot).
pub struct CameraFraming {
    pub origin: &'static str,
    #[allow(clippy::type_complexity)]
    pub frame: fn(&World, Entity, DVec3, DVec3) -> Option<Result<(DVec3, DVec3), String>>,
}

/// The first registered framing of `object`.
pub fn frame_object(
    world: &World,
    object: Entity,
    eye: DVec3,
    forward: DVec3,
) -> Option<Result<(DVec3, DVec3), String>> {
    pulsar_plugins::registered::<CameraFraming>()
        .find_map(|framing| (framing.frame)(world, object, eye, forward))
}

// ── Assistant tools ─────────────────────────────────────────────────────────

/// A module whose `#[tool]` functions are level editor assistant tools
/// (`module_path!()` of the plugin's tools module).
pub struct AiToolNamespace {
    pub origin: &'static str,
    pub namespace: &'static str,
}

/// Every registered assistant tool namespace.
pub fn ai_tool_namespaces() -> impl Iterator<Item = &'static AiToolNamespace> {
    pulsar_plugins::registered::<AiToolNamespace>()
}

extension_point!(
    ToolModeRegistration,
    CreateEntry,
    EnvironmentPreset,
    EnvironmentListing,
    LevelComponentSection,
    SaveHook,
    EditorData,
    Interaction,
    SelectionFollowUp,
    PropertyFollowUp,
    CameraFraming,
    AiToolNamespace,
);

/// The shared editor state the panels hold.
pub type SharedState = Arc<parking_lot::RwLock<LevelEditorState>>;

/// The shell's pieces a plugin's panels reuse: the frame pump that polls
/// the shared state, and the action that switches tool modes.
pub use crate::ui::frame_pump::spawn_frame_pump;
pub use crate::ui::SetToolMode;
