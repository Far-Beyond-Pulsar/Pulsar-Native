//! The level's sky (#1057) as the World Settings panel edits it.
//!
//! A level has one sky: its `AtmosphereComponent`, the sky plugin's
//! ([`sky_component::level_atmosphere`]). Without one the sky renders
//! black. The panel shows the sky's properties when the level has one and
//! a button that creates it when it does not ([`create_sky`]).

use engine_backend::scene::SceneWorldExt;
use pulsar_scenedb::World;

use crate::commands::{CommandResult, SceneCommand, TypedComponent, execute_command};
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
    let (instance, owner) = sky_component::level_atmosphere(world)?;
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

/// Selection address of the `ordinal`th directional light attached to the
/// unified Sky object.
pub fn sky_directional_light(world: &World, ordinal: usize) -> Option<(String, usize)> {
    let (_atmosphere, owner) = sky_component::level_atmosphere(world)?;
    let object_id = world.stable_id_of(owner)?.to_string();
    let mut found = 0usize;
    for (component_index, instance) in engine_backend::scene::attachments::instances(world, owner)
        .into_iter()
        .enumerate()
    {
        let Some(light) = world.get::<helio_component::LightComponent>(instance) else {
            continue;
        };
        if light.general.enabled
            && light.general.light_type == helio_component::LightType::Directional
            && engine_backend::scene::attachments::is_enabled(world, instance)
        {
            if found == ordinal {
                return Some((object_id, component_index));
            }
            found += 1;
        }
    }
    None
}

pub fn default_sun_light() -> helio_component::LightComponent {
    let mut sun = helio_component::LightComponent::default();
    sun.general.light_type = helio_component::LightType::Directional;
    sun.intensity.intensity = 2.0;
    sun.advanced.primary_atmosphere_light = true;
    sun.advanced.rotation_offset = [35.0, 0.0, 0.0];
    sun
}

pub fn default_moon_light() -> helio_component::LightComponent {
    let mut moon = helio_component::LightComponent::default();
    moon.general.light_type = helio_component::LightType::Directional;
    moon.color.color = [0.68, 0.76, 1.0, 1.0];
    moon.intensity.intensity = 0.025;
    moon.advanced.rotation_offset = [18.0, 0.0, 112.0];
    moon.emitter_disk.enabled = true;
    moon.emitter_disk.texture = helio_component::EmitterDiskTexture::MoonSolarSystemScopeCCBY40;
    moon.emitter_disk.angular_radius_degrees = 2.5;
    moon
}

/// Add the unified sky rig: an object named [`SKY_OBJECT_NAME`] with an
/// atmosphere, a sun light, and a dim textured moon light, as one undoable
/// command. Refused when the level already has a sky.
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
            components: vec![
                TypedComponent::new(sky_component::AtmosphereComponent::default()),
                TypedComponent::new(default_sun_light()),
                TypedComponent::new(default_moon_light()),
            ],
        },
    )
}
