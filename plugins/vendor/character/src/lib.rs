//! Character (`pulsar.character`): a character as one component.
//!
//! A [`CharacterComponent`] is what a user places and a script drives: one
//! card in the inspector, one entry in the component tree, one component in
//! scripts (`Presentation::Facade`). Placing it places the classes it is
//! built from, each its own component instance nested under it, each its
//! own plugin's: a `SkeletalMeshComponent` (meshes), an `AnimationComponent`
//! (animation), a `CharacterControllerComponent` (physics) and, when the
//! input plugin is enabled, a `PlayerInputComponent`. The plugin links none
//! of their crates: it requires their `class` contracts
//! (`pulsar-plugin.toml`), and reaches the controller by class name and
//! property, as a script does.
//!
//! Each frame its system [`MOVE`] turns the character's `move_input` and
//! `jump` into the controller's desired velocity and vertical speed: after
//! the player's input is read (`pulsar.input.devices`), before physics steps
//! (`pulsar.physics.step`).
//!
//! With the plugin disabled, a level holding characters loads them as
//! unresolved components with their data kept; the members stay their own
//! plugins' live components.
//!
//! This crate is the plugin's root: [`build`] registers everything.

use engine_backend::scene::{ClassSpec, ComponentsBuilder};
use engine_class_derive::{engine_class, world_component};
use pulsar_world_registry::pulsar_scene_model::attachments::{self, ComponentOwner};
use pulsar_world_registry::systems::COMPONENTS;
use pulsar_world_registry::{Presentation, System, SystemContext};

pulsar_plugins::installed!("pulsar-plugin.toml", build);

/// The class name scripts and level files use.
pub const CHARACTER_CLASS_NAME: &str = "CharacterComponent";

/// The classes a character is built from, placed with it.
pub const MEMBERS: &[&str] = &[
    "SkeletalMeshComponent",
    "AnimationComponent",
    "CharacterControllerComponent",
    "PlayerInputComponent",
];

/// The controller class a character drives.
const CONTROLLER: &str = "CharacterControllerComponent";

/// A character: what moves it, as the player or a script asks.
#[engine_class(category = "Gameplay", clone, debug, serialize, deserialize)]
#[category("Character", category_color = "#F4A261")]
#[serde(default)]
pub struct CharacterComponent {
    /// Walking speed, metres per second, at full input.
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Character")]
    pub walk_speed: f32,
    /// Upward speed a jump starts with, metres per second.
    #[property(min = 0.0, max = 100.0, step = 0.1, category = "Character")]
    pub jump_speed: f32,
    /// Where to walk this frame, in the character's plane (x right, y
    /// forward), each in -1..1: the player's move action or a script sets
    /// it.
    #[property(category = "Character")]
    pub move_input: [f32; 2],
    /// Jump on the next frame the controller is grounded; cleared when it
    /// does.
    #[property(category = "Character")]
    pub jump: bool,
}

impl Default for CharacterComponent {
    fn default() -> Self {
        Self {
            walk_speed: 4.0,
            jump_speed: 5.0,
            move_input: [0.0, 0.0],
            jump: false,
        }
    }
}

/// A `World` class; [`build`] registers it.
#[world_component]
impl CharacterComponent {}

/// The character's movement system.
pub const MOVE: System = System::new("pulsar.character.move", drive)
    .after(&["pulsar.input.devices", COMPONENTS])
    .before(&["pulsar.physics.step"]);

/// Everything the plugin registers: its class (a facade over what it
/// places, with its movement system) and, in the editor, the Create menu's
/// character.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.components().class::<CharacterComponent>(ClassSpec {
        requires: MEMBERS,
        systems: &[MOVE],
        presentation: Presentation::Facade,
        ..ClassSpec::default()
    });
    #[cfg(feature = "editor")]
    editor::build(p);
}

/// One character's step, read before writing (the world is borrowed by the
/// query while reading).
struct Step {
    owner: pulsar_scenedb::Entity,
    instance: pulsar_scenedb::Entity,
    velocity: [f32; 3],
    jump: Option<f32>,
}

/// [`MOVE`]: every enabled character sets its controller's desired velocity
/// from `move_input` (in the owner's facing plane, `walk_speed` at full
/// input) and starts a jump when asked and grounded.
fn drive(ctx: &mut SystemContext<'_>) {
    let world = &mut *ctx.world;
    let steps: Vec<Step> = world
        .query::<(&CharacterComponent, &ComponentOwner)>()
        .filter(|(_, (_, link))| link.is_enabled())
        .map(|(instance, (character, link))| {
            let [x, y] = character.move_input;
            let length = (x * x + y * y).sqrt().max(1.0);
            Step {
                owner: link.entity(),
                instance,
                // Ground plane: x right, z forward (the engine's y is up).
                velocity: [
                    x / length * character.walk_speed,
                    0.0,
                    y / length * character.walk_speed,
                ],
                jump: character.jump.then_some(character.jump_speed),
            }
        })
        .collect();
    for step in steps {
        let Some(controller) = controller_index(world, step.owner) else {
            continue;
        };
        let set = |world: &mut pulsar_scenedb::World, property: &str, value: serde_json::Value| {
            if let Err(error) = pulsar_world_registry::set_component_property(
                world, step.owner, CONTROLLER, controller, property, value,
            ) {
                tracing::warn!("character: {CONTROLLER}.{property}: {error}");
            }
        };
        set(world, "desired_velocity", serde_json::json!(step.velocity));
        if let Some(speed) = step.jump {
            let grounded = pulsar_world_registry::get_component_property(
                world, step.owner, CONTROLLER, controller, "grounded",
            )
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
            if grounded {
                set(world, "vertical_velocity", serde_json::json!(speed));
                if let Some(mut character) = world.get_mut::<CharacterComponent>(step.instance) {
                    character.jump = false;
                }
            }
        }
    }
}

/// The index, among the owner's instances of the controller class, of the
/// one the character drives (its first).
fn controller_index(world: &pulsar_scenedb::World, owner: pulsar_scenedb::Entity) -> Option<u32> {
    attachments::instances(world, owner)
        .iter()
        .any(|instance| {
            attachments::meta(world, *instance).is_some_and(|meta| meta.class_name == CONTROLLER)
        })
        .then_some(0)
}

#[cfg(feature = "editor")]
mod editor {
    use ui_level_editor::commands::{CommandResult, TypedComponent};
    use ui_level_editor::extend::{add_object, CreateEntry};
    use ui_level_editor::scene_edit::{ObjectType, Transform};
    use ui_level_editor::LevelEditorState;

    /// The Create menu's character: an object with a character, which brings
    /// its skeletal mesh, animator, controller and input with it.
    fn add_character(state: &mut LevelEditorState) -> CommandResult {
        add_object(
            state,
            "Character",
            ObjectType::Empty,
            Transform::default(),
            vec![TypedComponent::new(super::CharacterComponent::default())],
        )
    }

    pub(super) fn build(p: &mut pulsar_plugins::PluginBuilder) {
        p.add(CreateEntry {
            origin: p.origin(),
            id: "character",
            menu_label: Some(|| "Character".to_string()),
            environment_label: None,
            quick_add: None,
            order: 50,
            offered: None,
            create: add_character,
        });
    }
}
