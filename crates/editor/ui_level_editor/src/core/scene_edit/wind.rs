//! The level's global wind (#1123) as the World Settings panel edits it.
//!
//! A level has at most one: its `WindComponent`
//! ([`engine_backend::scene::level_rules`]); every foliage component sways in
//! it (unless one opts out for its own wind). Without one, foliage uses the
//! components' own wind. The panel shows the wind's properties when the
//! level has one and a button that creates it when it does not
//! ([`create_wind`]).

use engine_backend::scene::level_rules;
use engine_backend::scene::SceneWorldExt;
use pulsar_scenedb::World;

use crate::commands::{execute_command, CommandResult, SceneCommand, TypedComponent};
use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;

/// Name of the object [`create_wind`] adds.
pub const WIND_OBJECT_NAME: &str = "Wind";

/// Where the level's wind lives: the object holding it and the
/// component's index in that object's list (the properties panel's
/// addressing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelWind {
    pub object_id: String,
    pub object_name: String,
    pub component_index: usize,
    pub enabled: bool,
}

/// The level's global wind, if it has one.
pub fn level_wind(world: &World) -> Option<LevelWind> {
    let (instance, owner) = level_rules::level_wind(world)?;
    let object_id = world.stable_id_of(owner)?.to_string();
    let component_index = engine_backend::scene::attachments::instances(world, owner)
        .iter()
        .position(|entity| *entity == instance)?;
    Some(LevelWind {
        object_name: world
            .get::<engine_backend::scene::Name>(owner)
            .map(|name| name.0.clone())
            .unwrap_or_else(|| object_id.clone()),
        object_id,
        component_index,
        enabled: engine_backend::scene::attachments::is_enabled(world, instance),
    })
}

/// Add the level's global wind: an object named [`WIND_OBJECT_NAME`]
/// holding a default `WindComponent` (a light breeze), as one undoable
/// command. Refused with a message when the level already has one.
pub fn create_wind(state: &mut LevelEditorState) -> CommandResult {
    execute_command(
        state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: WIND_OBJECT_NAME.to_string(),
                object_type: ObjectType::Empty,
                transform: Transform::default(),
                visible: true,
                locked: false,
                parent: None,
                children: vec![],
                scene_path: String::new(),
                props: Default::default(),
                component_instances: None,
            },
            parent_id: None,
            components: vec![TypedComponent::new(
                helio_component::WindComponent::default(),
            )],
        },
    )
}
