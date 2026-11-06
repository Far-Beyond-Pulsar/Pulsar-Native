//! Environment mixer creation and presets.

use crate::commands::{CommandResult, SceneCommand, TypedComponent, execute_command};
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
            Self::ClearDay => ([1.0, 0.96, 0.88, 1.0], 2.0, [35.0, 25.0, 0.0]),
            Self::GoldenHour => ([1.0, 0.58, 0.32, 1.0], 2.0, [8.0, 285.0, 0.0]),
            Self::Overcast => ([0.72, 0.82, 1.0, 1.0], 2.0, [55.0, 20.0, 0.0]),
        }
    }
}

/// Add an environment component as a separate, named scene object and select it.
pub fn create_component(state: &mut LevelEditorState, class_name: &str) -> CommandResult {
    if class_name == "AtmosphereComponent" {
        let result = crate::scene_edit::sky::create_sky(state);
        if let Some(id) = result.affected_ids.first() {
            execute_command(
                state,
                SceneCommand::SelectObject {
                    id: Some(id.clone()),
                },
            );
        }
        return result;
    }
    let mut object_type = ObjectType::Empty;
    let components = match class_name {
        "WindComponent" => vec![TypedComponent::new(
            foliage_component::WindComponent::default(),
        )],
        "LightComponent" => vec![TypedComponent::new(
            lights_component::LightComponent::default(),
        )],
        "SpotLight" => {
            let mut light = lights_component::LightComponent::default();
            let object_light_type = LightType::Spot;
            light.general.light_type = lights_component::LightType::Spot;
            object_type = ObjectType::Light(object_light_type);
            vec![TypedComponent::new(light)]
        }
        "GlobalFogComponent" => vec![TypedComponent::new(
            fog_component::GlobalFogComponent::default(),
        )],
        "LocalFogVolumeComponent" => vec![TypedComponent::new(
            fog_component::LocalFogVolumeComponent::default(),
        )],
        "PostProcessVolumeComponent" => vec![TypedComponent::new(
            post_component::PostProcessVolumeComponent::default(),
        )],
        "ReflectionCaptureComponent" => vec![TypedComponent::new(
            lights_component::ReflectionCaptureComponent::default(),
        )],
        "WaterVolumeComponent" => vec![TypedComponent::new(
            water_component::WaterVolumeComponent::default(),
        )],
        _ => return CommandResult::noop("Unknown environment component"),
    };
    let name = match class_name {
        "WindComponent" => "Wind",
        "LightComponent" => "Point Light",
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

/// Ensure the unified sky rig exists, then apply a starter sun setup through
/// normal undoable component and transform commands.
pub fn apply_preset(state: &mut LevelEditorState, preset: EnvironmentPreset) -> CommandResult {
    let mut changed = false;
    if crate::scene_edit::sky::level_sky(&state.scene.world()).is_none() {
        changed |= crate::scene_edit::sky::create_sky(state).changed;
    }

    changed |= ensure_sky_lights(state);
    let Some((object_id, component_index)) =
        crate::scene_edit::sky::sky_directional_light(&state.scene.world(), 0)
    else {
        return CommandResult::noop("The Sky object has no sun light");
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
    let rotation_result = execute_command(
        state,
        SceneCommand::SetComponentProperty {
            id: object_id,
            class_name: "LightComponent".into(),
            component_index,
            prop_name: "advanced.rotation_offset".into(),
            value: Box::new(rotation),
        },
    );
    changed |= color_result.changed || intensity_result.changed || rotation_result.changed;

    if changed {
        CommandResult::ok(vec![])
    } else {
        CommandResult::noop("This environment preset is already applied")
    }
}

fn ensure_sky_lights(state: &mut LevelEditorState) -> bool {
    let Some(sky) = crate::scene_edit::sky::level_sky(&state.scene.world()) else {
        return false;
    };
    let mut changed = false;
    for (ordinal, light) in [
        crate::scene_edit::sky::default_sun_light(),
        crate::scene_edit::sky::default_moon_light(),
    ]
    .into_iter()
    .enumerate()
    {
        if crate::scene_edit::sky::sky_directional_light(&state.scene.world(), ordinal).is_some() {
            continue;
        }
        changed |= execute_command(
            state,
            SceneCommand::AddComponent {
                id: sky.object_id.clone(),
                class_name: "LightComponent".into(),
                value: Some(Box::new(light)),
            },
        )
        .changed;
    }
    changed
}
