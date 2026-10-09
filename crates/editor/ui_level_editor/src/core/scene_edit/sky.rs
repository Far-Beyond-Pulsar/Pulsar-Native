//! The level's sky (#1057) as the World Settings panel edits it.
//!
//! A level has one sky: its `AtmosphereComponent`
//! ([`engine_backend::scene::level_rules`]). Without one the sky renders
//! black. The panel shows the sky's properties when the level has one and
//! a button that creates it when it does not ([`create_sky`]).

use engine_backend::scene::level_rules;
use engine_backend::scene::SceneWorldExt;
use pulsar_scenedb::World;

use crate::commands::{execute_command, CommandResult, SceneCommand, TypedComponent};
use crate::scene_edit::{ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;

/// Name of the object [`create_sky`] adds.
pub const SKY_OBJECT_NAME: &str = "Sky";

/// Where the level's sky lives: the object holding it and the
/// component's index in that object's list (the properties panel's
/// addressing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelSky {
    pub object_id: String,
    pub object_name: String,
    pub component_index: usize,
    pub enabled: bool,
}

/// The level's sky, if it has one.
pub fn level_sky(world: &World) -> Option<LevelSky> {
    let (instance, owner) = level_rules::level_atmosphere(world)?;
    let object_id = world.stable_id_of(owner)?.to_string();
    let component_index = engine_backend::scene::attachments::instances(world, owner)
        .iter()
        .position(|entity| *entity == instance)?;
    Some(LevelSky {
        object_name: world
            .get::<engine_backend::scene::Name>(owner)
            .map(|name| name.0.clone())
            .unwrap_or_else(|| object_id.clone()),
        object_id,
        component_index,
        enabled: engine_backend::scene::attachments::is_enabled(world, instance),
    })
}

/// Add the level's sky: an object named [`SKY_OBJECT_NAME`] holding a
/// default (Earth) `AtmosphereComponent`, as one undoable command. Refused
/// with a message when the level already has one.
pub fn create_sky(state: &mut LevelEditorState) -> CommandResult {
    execute_command(
        state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: SKY_OBJECT_NAME.to_string(),
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
                helio_component::AtmosphereComponent::default(),
            )],
        },
    )
}
