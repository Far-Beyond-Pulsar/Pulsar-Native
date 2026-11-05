//! Lights in the level editor: the Create menu's lights, the environment
//! mixer's lights and reflection captures, and its starter setups (a sky
//! and a tuned sun), registered through the level editor's extension points
//! (`ui_level_editor::extend`). Every add is one undoable command through
//! the editor's commands; the components are this plugin's.

use engine_backend::scene::SceneWorldExt as _;
use lights_component::{LightComponent, LightType as Shape, ReflectionCaptureComponent};
use pulsar_scenedb::World;
use ui_level_editor::LevelEditorState;
use ui_level_editor::commands::{CommandResult, SceneCommand, TypedComponent, execute_command};
use ui_level_editor::extend::{
    CreateEntry, EnvironmentListing, EnvironmentPreset, add_object, create,
};
use ui_level_editor::scene_edit::{LightType, ObjectType, Transform};

// ── Create menu ─────────────────────────────────────────────────────────────

/// The Create menu's light of `shape`: enabled, at the origin.
fn menu_light(
    state: &mut LevelEditorState,
    name: &str,
    shape: Shape,
    kind: LightType,
) -> CommandResult {
    let mut light = LightComponent::default();
    light.general.enabled = true;
    light.general.light_type = shape;
    add_object(
        state,
        name,
        ObjectType::Light(kind),
        Transform::default(),
        vec![TypedComponent::new(light)],
    )
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "light_point",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.PointLight").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 100,
        create: |state| menu_light(state, "Point Light", Shape::Point, LightType::Point),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "light_spot",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.SpotLight").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 110,
        create: |state| menu_light(state, "Spot Light", Shape::Spot, LightType::Spot),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "light_directional",
        menu_label: Some(|| rust_i18n::t!("LevelEditor.Create.DirectionalLight").to_string()),
        environment_label: None,
        quick_add: None,
        offered: None,
        order: 120,
        create: |state| {
            menu_light(
                state,
                "Directional Light",
                Shape::Directional,
                LightType::Directional,
            )
        },
    }
}

// ── Environment mixer ───────────────────────────────────────────────────────

/// The mixer's light of `shape` (a directional one starts at 2 lux·sr, a
/// usable sun).
fn environment_light(
    state: &mut LevelEditorState,
    name: &str,
    shape: Shape,
    kind: LightType,
) -> CommandResult {
    let mut light = LightComponent::default();
    light.general.light_type = shape;
    if shape == Shape::Directional {
        light.intensity.intensity = 2.0;
    }
    add_object(
        state,
        name,
        ObjectType::Light(kind),
        Transform::default(),
        vec![TypedComponent::new(light)],
    )
}

/// The level has no sun yet (one sun: a second directional light is not
/// offered).
fn no_sun(world: &World) -> bool {
    lights_component::directional_lights(world).is_empty()
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_point_light",
        menu_label: None,
        environment_label: Some("Point Light"),
        quick_add: None,
        offered: None,
        order: 400,
        create: |state| {
            add_object(
                state,
                "Point Light",
                ObjectType::Light(LightType::Point),
                Transform::default(),
                vec![TypedComponent::new(LightComponent::default())],
            )
        },
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_spot_light",
        menu_label: None,
        environment_label: Some("Spot Light"),
        quick_add: None,
        offered: None,
        order: 401,
        create: |state| environment_light(state, "Spot Light", Shape::Spot, LightType::Spot),
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_directional_light",
        menu_label: None,
        environment_label: Some("Directional Light"),
        quick_add: None,
        offered: Some(no_sun),
        order: 402,
        create: |state| {
            environment_light(
                state,
                "Directional Light",
                Shape::Directional,
                LightType::Directional,
            )
        },
    }
}

inventory::submit! {
    CreateEntry {
        origin: pulsar_plugins::origin!(),
        id: "environment_reflection_capture",
        menu_label: None,
        environment_label: Some("Reflection Capture"),
        quick_add: None,
        offered: None,
        order: 440,
        create: |state| {
            add_object(
                state,
                "Reflection Capture",
                ObjectType::Empty,
                Transform::default(),
                vec![TypedComponent::new(ReflectionCaptureComponent::default())],
            )
        },
    }
}

inventory::submit! {
    EnvironmentListing {
        origin: pulsar_plugins::origin!(),
        class_name: "LightComponent",
        label: "Light",
        order: 0,
        create_entry: Some("environment_point_light"),
    }
}

inventory::submit! {
    EnvironmentListing {
        origin: pulsar_plugins::origin!(),
        class_name: "ReflectionCaptureComponent",
        label: "Reflection Capture",
        order: 40,
        create_entry: Some("environment_reflection_capture"),
    }
}

// ── Starter setups ──────────────────────────────────────────────────────────

/// A starter sun: colour, intensity and rotation (degrees).
struct Sun {
    color: [f32; 4],
    intensity: f32,
    rotation: [f32; 3],
}

/// Ensure a sky (when the project has one to add) and a sun exist, then
/// tune the sun through normal undoable component and transform commands.
fn apply_sun(state: &mut LevelEditorState, sun: Sun) -> CommandResult {
    // The sky plugin's entry, when enabled: refused when the level has one.
    let mut changed = create(state, "sky").changed;

    let existing = lights_component::directional_lights(&state.scene.world())
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

    let (object_id, component_index) = match existing {
        Some(sun) => sun,
        None => {
            let result = create(state, "light_directional");
            changed |= result.changed;
            let Some(id) = result.affected_ids.first().cloned() else {
                return result;
            };
            let index = ui_level_editor::scene_edit::components::get_component_class_names(
                &state.scene.world(),
                &id,
            )
            .iter()
            .position(|name| name == "LightComponent")
            .unwrap_or(0);
            (id, index)
        }
    };

    let color_result = execute_command(
        state,
        SceneCommand::SetComponentProperty {
            id: object_id.clone(),
            class_name: "LightComponent".into(),
            component_index,
            prop_name: "color.color".into(),
            value: Box::new(sun.color),
        },
    );
    let intensity_result = execute_command(
        state,
        SceneCommand::SetComponentProperty {
            id: object_id.clone(),
            class_name: "LightComponent".into(),
            component_index,
            prop_name: "intensity.intensity".into(),
            value: Box::new(sun.intensity),
        },
    );
    let transform_result = execute_command(
        state,
        SceneCommand::SetTransform {
            id: object_id,
            position: None,
            rotation: Some(sun.rotation),
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

inventory::submit! {
    EnvironmentPreset {
        origin: pulsar_plugins::origin!(),
        id: "clear_day",
        label: "Clear Day",
        order: 0,
        apply: |state| {
            apply_sun(
                state,
                Sun {
                    color: [1.0, 0.96, 0.88, 1.0],
                    intensity: 2.0,
                    rotation: [35.0, 25.0, 0.0],
                },
            )
        },
    }
}

inventory::submit! {
    EnvironmentPreset {
        origin: pulsar_plugins::origin!(),
        id: "golden_hour",
        label: "Golden Hour",
        order: 10,
        apply: |state| {
            apply_sun(
                state,
                Sun {
                    color: [1.0, 0.58, 0.32, 1.0],
                    intensity: 2.0,
                    rotation: [8.0, 285.0, 0.0],
                },
            )
        },
    }
}

inventory::submit! {
    EnvironmentPreset {
        origin: pulsar_plugins::origin!(),
        id: "overcast",
        label: "Overcast",
        order: 20,
        apply: |state| {
            apply_sun(
                state,
                Sun {
                    color: [0.72, 0.82, 1.0, 1.0],
                    intensity: 2.0,
                    rotation: [55.0, 20.0, 0.0],
                },
            )
        },
    }
}
