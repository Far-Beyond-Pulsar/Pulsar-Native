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
use ui_level_editor::scene_edit::{ObjectType, Transform};

// ── Hierarchy ───────────────────────────────────────────────────────────────

// An object holding a light shows as one.
inventory::submit! {
    ui_level_editor::extend::ObjectIconRegistration {
        origin: pulsar_plugins::origin!(),
        order: 100,
        icon: |world, object| {
            engine_backend::scene::attachments::instances(world, object)
                .iter()
                .any(|instance| world.get::<LightComponent>(*instance).is_some())
                .then_some((
                    ui::IconName::LightBulb,
                    ui::hierarchical_tree::tree_colors::SPECIAL_YELLOW,
                ))
        },
    }
}

// ── Create menu ─────────────────────────────────────────────────────────────

/// The Create menu's light of `shape`: enabled, at the origin.
fn menu_light(state: &mut LevelEditorState, name: &str, shape: Shape) -> CommandResult {
    let mut light = LightComponent::default();
    light.general.enabled = true;
    light.general.light_type = shape;
    add_object(
        state,
        name,
        ObjectType::Empty,
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
        create: |state| menu_light(state, "Point Light", Shape::Point),
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
        create: |state| menu_light(state, "Spot Light", Shape::Spot),
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
            )
        },
    }
}

// ── Environment mixer ───────────────────────────────────────────────────────

/// The mixer's light of `shape` (a directional one starts at 2 lux·sr, a
/// usable sun).
fn environment_light(state: &mut LevelEditorState, name: &str, shape: Shape) -> CommandResult {
    let mut light = LightComponent::default();
    light.general.light_type = shape;
    if shape == Shape::Directional {
        light.intensity.intensity = 2.0;
    }
    add_object(
        state,
        name,
        ObjectType::Empty,
        Transform::default(),
        vec![TypedComponent::new(light)],
    )
}

/// The level has no sun yet (several directional lights are allowed; the
/// Create menu offers the first).
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
                ObjectType::Empty,
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
        create: |state| environment_light(state, "Spot Light", Shape::Spot),
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

/// The sky object's class (the sky plugin's, named, not linked): the sky
/// rig's lights sit on the object that holds it.
const SKY_CLASS: &str = "AtmosphereComponent";

/// The object holding the level's sky (`SKY_CLASS`), by its stable id.
fn sky_object(world: &World) -> Option<String> {
    let instance = world
        .query::<&engine_backend::scene::attachments::ComponentMeta>()
        .find(|(_, meta)| meta.class_name == SKY_CLASS)
        .map(|(instance, _)| instance)?;
    let owner = engine_backend::scene::attachments::owner_of(world, instance)?;
    world.stable_id_of(owner).map(str::to_string)
}

/// The `ordinal`th enabled directional light on the sky object, as its
/// selection address (object id, component index).
fn sky_directional_light(world: &World, ordinal: usize) -> Option<(String, usize)> {
    let object_id = sky_object(world)?;
    let owner = world.entity_for(&object_id)?;
    engine_backend::scene::attachments::instances(world, owner)
        .into_iter()
        .enumerate()
        .filter(|(_, instance)| {
            world
                .get::<LightComponent>(*instance)
                .is_some_and(lights_component::is_directional)
                && engine_backend::scene::attachments::is_enabled(world, *instance)
        })
        .nth(ordinal)
        .map(|(component_index, _)| (object_id.clone(), component_index))
}

/// The sky rig's sun: directional, 2 lux·sr, 35° up.
pub fn default_sun_light() -> LightComponent {
    let mut sun = LightComponent::default();
    sun.general.light_type = Shape::Directional;
    sun.intensity.intensity = 2.0;
    sun.advanced.rotation_offset = [35.0, 0.0, 0.0];
    sun
}

/// The sky rig's moon: a dim, cool directional light drawn as a textured
/// disk.
pub fn default_moon_light() -> LightComponent {
    let mut moon = LightComponent::default();
    moon.general.light_type = Shape::Directional;
    moon.color.color = [0.68, 0.76, 1.0, 1.0];
    moon.intensity.intensity = 0.025;
    moon.advanced.rotation_offset = [18.0, 0.0, 112.0];
    moon.emitter_disk.enabled = true;
    moon.emitter_disk.texture = lights_component::EmitterDiskTexture::MoonSolarSystemScopeCCBY40;
    moon.emitter_disk.angular_radius_degrees = 2.5;
    moon
}

/// The sky rig's lights: a sun and a moon on the sky object, each added
/// when missing.
fn ensure_sky_lights(state: &mut LevelEditorState) -> bool {
    let Some(sky) = sky_object(&state.scene.world()) else {
        return false;
    };
    let mut changed = false;
    for (ordinal, light) in [default_sun_light(), default_moon_light()]
        .into_iter()
        .enumerate()
    {
        if sky_directional_light(&state.scene.world(), ordinal).is_some() {
            continue;
        }
        changed |= execute_command(
            state,
            SceneCommand::AddComponent {
                id: sky.clone(),
                class_name: "LightComponent".into(),
                value: Some(Box::new(light)),
            },
        )
        .changed;
    }
    changed
}

/// Ensure the sky rig (a sky, when the project has one to add, with its sun
/// and moon) exists, then tune the sun through normal undoable component
/// commands. Without a sky plugin, the sun is a directional light of its own.
fn apply_sun(state: &mut LevelEditorState, sun: Sun) -> CommandResult {
    // The sky plugin's entry, when enabled: refused when the level has one.
    let mut changed = create(state, "sky").changed;
    changed |= ensure_sky_lights(state);

    let rigged = sky_directional_light(&state.scene.world(), 0);
    let (object_id, component_index) = match rigged {
        Some(sun) => sun,
        None => {
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
            match existing {
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
            }
        }
    };

    let mut set =
        |state: &mut LevelEditorState, prop: &str, value: Box<dyn std::any::Any + Send>| {
            execute_command(
                state,
                SceneCommand::SetComponentProperty {
                    id: object_id.clone(),
                    class_name: "LightComponent".into(),
                    component_index,
                    prop_name: prop.into(),
                    value,
                },
            )
            .changed
        };
    changed |= set(state, "color.color", Box::new(sun.color));
    changed |= set(state, "intensity.intensity", Box::new(sun.intensity));
    // The light's own direction (the rig's lights share the sky object's
    // transform).
    changed |= set(state, "advanced.rotation_offset", Box::new(sun.rotation));

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
