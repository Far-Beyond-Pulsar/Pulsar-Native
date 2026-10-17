//! Environment mixer creation and presets.

use crate::commands::{execute_command, CommandResult, SceneCommand, TypedComponent};
use crate::scene_edit::{LightType, ObjectType, SceneObjectData, Transform};
use crate::state::LevelEditorState;
use engine_backend::scene::SceneWorldExt;

#[derive(Clone, Copy, Debug)]
pub enum EnvironmentPreset {
    ClearDay,
    GoldenHour,
    Overcast,
}

impl EnvironmentPreset {
    pub const ALL: [Self; 3] = [Self::ClearDay, Self::GoldenHour, Self::Overcast];

    pub fn label(self) -> &'static str {
        match self {
            Self::ClearDay => "Clear Day",
            Self::GoldenHour => "Golden Hour",
            Self::Overcast => "Overcast",
        }
    }

    fn sun(self) -> ([f32; 4], f32, [f32; 3]) {
        match self {
            Self::ClearDay => ([1.0, 0.96, 0.88, 1.0], 80_000.0, [35.0, 25.0, 0.0]),
            Self::GoldenHour => ([1.0, 0.58, 0.32, 1.0], 38_000.0, [8.0, 285.0, 0.0]),
            Self::Overcast => ([0.72, 0.82, 1.0, 1.0], 24_000.0, [55.0, 20.0, 0.0]),
        }
    }
}

/// Add an environment component as a separate, named scene object and select it.
pub fn create_component(state: &mut LevelEditorState, class_name: &str) -> CommandResult {
    let mut object_type = ObjectType::Empty;
    let components = match class_name {
        "AtmosphereComponent" => vec![TypedComponent::new(
            helio_component::AtmosphereComponent::default(),
        )],
        "WindComponent" => vec![TypedComponent::new(
            helio_component::WindComponent::default(),
        )],
        "LightComponent" => vec![TypedComponent::new(
            helio_component::LightComponent::default(),
        )],
        "DirectionalLight" | "SpotLight" => {
            let mut light = helio_component::LightComponent::default();
            let (light_type, object_light_type) = if class_name == "DirectionalLight" {
                (
                    helio_component::LightType::Directional,
                    LightType::Directional,
                )
            } else {
                (helio_component::LightType::Spot, LightType::Spot)
            };
            light.general.light_type = light_type;
            object_type = ObjectType::Light(object_light_type);
            vec![TypedComponent::new(light)]
        }
        "GlobalFogComponent" => vec![TypedComponent::new(
            helio_component::GlobalFogComponent::default(),
        )],
        "LocalFogVolumeComponent" => vec![TypedComponent::new(
            helio_component::LocalFogVolumeComponent::default(),
        )],
        "PostProcessVolumeComponent" => vec![TypedComponent::new(
            helio_component::PostProcessVolumeComponent::default(),
        )],
        "ReflectionCaptureComponent" => vec![TypedComponent::new(
            helio_component::ReflectionCaptureComponent::default(),
        )],
        "WaterVolumeComponent" => vec![TypedComponent::new(
            helio_component::WaterVolumeComponent::default(),
        )],
        _ => return CommandResult::noop("Unknown environment component"),
    };
    let name = match class_name {
        "AtmosphereComponent" => "Sky",
        "WindComponent" => "Wind",
        "LightComponent" => "Point Light",
        "DirectionalLight" => "Directional Light",
        "SpotLight" => "Spot Light",
        "GlobalFogComponent" => "Global Fog",
        "LocalFogVolumeComponent" => "Local Fog Volume",
        "PostProcessVolumeComponent" => "Post Process Volume",
        "ReflectionCaptureComponent" => "Reflection Capture",
        "WaterVolumeComponent" => "Water Volume",
        _ => unreachable!(),
    };
    if class_name == "LightComponent" {
        object_type = ObjectType::Light(LightType::Point);
    }
    let result = execute_command(
        state,
        SceneCommand::AddObjectWithComponents {
            data: SceneObjectData {
                id: String::new(),
                name: name.to_string(),
                object_type,
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

/// Ensure a sky and sun exist, then apply a starter sun setup through normal
/// undoable component and transform commands.
pub fn apply_preset(state: &mut LevelEditorState, preset: EnvironmentPreset) -> CommandResult {
    let mut changed = false;
    if crate::scene_edit::sky::level_sky(&state.scene.world()).is_none() {
        changed |= crate::scene_edit::sky::create_sky(state).changed;
    }

    let sun = engine_backend::scene::level_rules::directional_lights(&state.scene.world())
        .first()
        .and_then(|instance| {
            let world = state.scene.world();
            let owner = engine_backend::scene::attachments::owner_of(&world, *instance)?;
            let id = world.stable_id_of(owner)?.to_string();
            let index = engine_backend::scene::attachments::instances(&world, owner)
                .iter()
                .position(|candidate| candidate == instance)?;
            Some((id, index))
        });

    let (object_id, component_index) = match sun {
        Some(sun) => sun,
        None => {
            let result = crate::scene_edit::create::create(state, "light_directional");
            changed |= result.changed;
            let Some(id) = result.affected_ids.first().cloned() else {
                return result;
            };
            let index =
                crate::scene_edit::components::get_component_class_names(&state.scene.world(), &id)
                    .iter()
                    .position(|name| name == "LightComponent")
                    .unwrap_or(0);
            (id, index)
        }
    };

    let (color, intensity, rotation) = preset.sun();
    let color_result = execute_command(
        state,
        SceneCommand::SetComponentProperty {
            id: object_id.clone(),
            class_name: "LightComponent".into(),
            component_index,
            prop_name: "color.color".into(),
            value: Box::new(color),
        },
    );
    let intensity_result = execute_command(
        state,
        SceneCommand::SetComponentProperty {
            id: object_id.clone(),
            class_name: "LightComponent".into(),
            component_index,
            prop_name: "intensity.intensity".into(),
            value: Box::new(intensity),
        },
    );
    let transform_result = execute_command(
        state,
        SceneCommand::SetTransform {
            id: object_id,
            position: None,
            rotation: Some(rotation),
            scale: None,
        },
    );
    changed |= color_result.changed || intensity_result.changed || transform_result.changed;

    if changed {
        CommandResult::ok(vec![])
    } else {
        CommandResult::noop("This environment preset is already applied")
    }
}
