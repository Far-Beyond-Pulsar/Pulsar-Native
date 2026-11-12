//! The `assets/examples/physics_playground` project: its generator and its
//! packaged-game proof.
//!
//! The level is not hand-written JSON: `generate_physics_playground` builds it
//! from the real component types (`ColliderComponent`, `RigidBodyComponent`,
//! `JointComponent`, `ForceFieldComponent`, `StaticMeshComponent`, ...), so
//! every reflected property is written with an explicit value, and writes
//! `scene/default.level` plus the `.physmat` / `.physlayers` assets. Re-run it
//! after changing the level below:
//!
//! ```text
//! cargo test -p pulsar_package --test physics_playground generate_physics_playground -- --ignored
//! ```
//!
//! `the_example_cooks_and_runs_its_physics` is the check: it cooks the example
//! through the real cook path into loose `Content/`, runs it headless through
//! the standalone entry (`pulsar_game::standalone`) and asserts what the
//! level is built to show. No window, no GPU: rendering is not covered.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use engine_backend::scene::{SceneWorldExt, Transform};
use installed_plugins::physics_plugin::assets::layers::{CollisionLayers, Layer};
use installed_plugins::physics_plugin::assets::material::{
    CombineRule, PhysicsMaterial, SurfaceType,
};
use installed_plugins::physics_plugin::assets::Asset;
use installed_plugins::physics_plugin::{
    simulation_running, wait_idle, BodyType, ColliderAxis, ColliderComponent, ColliderShape,
    FalloffMode, ForceFieldComponent, ForceFieldKind, JointComponent, JointKind,
    RigidBodyComponent, COLLISION_ENTER, TRIGGER_ENTER,
};
use pulsar_content::ContentRoot;
use pulsar_core::TickMode;
use pulsar_events::gamma::{Channel, DynValue, SubscribeOptions};
use pulsar_game::standalone::{self, HeadlessHooks, HEADLESS_STEP};
use pulsar_game::tick::TickLoop;
use pulsar_package::{package, PackageOptions};
use pulsar_world_registry::SessionKind;
use serde_json::{json, Value};

/// Physics threads are counted process-wide: one test at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/examples/physics_playground")
}

// ── Authoring helpers ──────────────────────────────────────────────────────

/// The built-in primitives are 2 units across in the file and imported at
/// 100x: scale 0.005 per metre of size (a 1 m cube is scale 0.005).
const PER_METRE: f64 = 0.005;

const CUBE: &str = "meshes/primitives/SM_Cube.fbx";
const SPHERE: &str = "meshes/primitives/SM_Sphere.fbx";
const CYLINDER: &str = "meshes/primitives/SM_Cylinder.fbx";

const GROUND_MAT: &str = "assets/physics/concrete.physmat";
const WOOD_MAT: &str = "assets/physics/wood.physmat";
const RUBBER_MAT: &str = "assets/physics/rubber.physmat";
const ICE_MAT: &str = "assets/physics/ice.physmat";
const LAYERS: &str = "assets/physics/playground.physlayers";

struct Look {
    color: [f32; 4],
    roughness: f32,
    metallic: f32,
    emissive: f32,
}

const fn look(color: [f32; 4], roughness: f32, metallic: f32, emissive: f32) -> Look {
    Look {
        color,
        roughness,
        metallic,
        emissive,
    }
}

struct Obj {
    id: String,
    name: String,
    position: [f64; 3],
    rotation: [f64; 3],
    scale: [f64; 3],
    parent: Option<String>,
    object_type: Value,
    components: Vec<Value>,
}

impl Obj {
    fn new(id: &str, name: &str, position: [f64; 3]) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            position,
            rotation: [0.0; 3],
            scale: [1.0; 3],
            parent: None,
            object_type: json!("Empty"),
            components: Vec::new(),
        }
    }
    fn rotate(mut self, degrees: [f64; 3]) -> Self {
        self.rotation = degrees;
        self
    }
    fn with(mut self, class: &str, data: Value) -> Self {
        let index = self.components.len();
        self.components.push(
            json!({ "index": index, "class_name": class, "enabled": true, "data": data }),
        );
        self
    }
    fn typed(self, class: &str, value: &impl serde::Serialize) -> Self {
        self.with(class, serde_json::to_value(value).unwrap())
    }
    /// A visible built-in mesh `size` metres across, in `look`'s material.
    fn mesh(mut self, mesh: &str, size: [f64; 3], look: Look, movable: bool, alpha: f32) -> Self {
        self.scale = size.map(|s| s * PER_METRE);
        let emissive = [look.color[0], look.color[1], look.color[2]];
        self.with(
            "StaticMeshComponent",
            json!({
                "mesh_asset": mesh,
                "movability": if movable { "Movable" } else { "Static" },
                "legacy_material_override": {
                    "base_color": [look.color[0], look.color[1], look.color[2], alpha],
                    "metallic": look.metallic,
                    "roughness": look.roughness,
                    "emissive_color": emissive,
                    "emissive_intensity": look.emissive,
                    "alpha": alpha,
                },
            }),
        )
    }
    fn json(&self, children: &[String]) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "object_type": self.object_type,
            "transform": {
                "position": self.position,
                "rotation": self.rotation,
                "scale": self.scale,
            },
            "visible": true,
            "locked": false,
            "parent": self.parent,
            "children": children,
            "scene_path": self.name,
            "props": {},
            "component_instances": self.components,
        })
    }
}

fn body(body_type: BodyType, mass: f32) -> RigidBodyComponent {
    RigidBodyComponent {
        enabled: true,
        body_type,
        mass,
        gravity_scale: 1.0,
        can_sleep: true,
        continuous_collision: body_type == BodyType::Dynamic,
        linear_damping: 0.0,
        angular_damping: 0.05,
        lock_position_x: false,
        lock_position_y: false,
        lock_position_z: false,
        lock_rotation_x: false,
        lock_rotation_y: false,
        lock_rotation_z: false,
        linear_velocity: [0.0; 3],
        angular_velocity: [0.0; 3],
    }
}

fn box_collider(half: [f32; 3], material: &str, layer: &str) -> ColliderComponent {
    ColliderComponent {
        enabled: true,
        shape: ColliderShape::Box,
        center: [0.0; 3],
        half_extents: half,
        radius: 0.5,
        height: 1.0,
        axis: ColliderAxis::Y,
        mesh: String::new(),
        is_trigger: false,
        physics_material: material.into(),
        layer: layer.into(),
    }
}

fn sphere_collider(radius: f32, material: &str) -> ColliderComponent {
    ColliderComponent {
        shape: ColliderShape::Sphere,
        radius,
        ..box_collider([0.5; 3], material, "Props")
    }
}

/// A static box (ground, wall, post): collider, Static body and a visible mesh.
fn static_box(id: &str, name: &str, at: [f64; 3], half: [f32; 3], mat: &str, look: Look) -> Obj {
    let size = half.map(|h| f64::from(h) * 2.0);
    Obj::new(id, name, at)
        .mesh(CUBE, size, look, false, 1.0)
        .typed("ColliderComponent", &box_collider(half, mat, "World"))
        .typed("RigidBodyComponent", &body(BodyType::Static, 0.0))
}

fn dynamic_ball(id: &str, name: &str, at: [f64; 3], radius: f32, mass: f32, look: Look) -> Obj {
    let mat = if mass < 1.0 { WOOD_MAT } else { RUBBER_MAT };
    Obj::new(id, name, at)
        .mesh(SPHERE, [f64::from(radius) * 2.0; 3], look, true, 1.0)
        .typed("ColliderComponent", &sphere_collider(radius, mat))
        .typed("RigidBodyComponent", &body(BodyType::Dynamic, mass))
}

// ── The level ──────────────────────────────────────────────────────────────

fn build_level() -> Value {
    let mut objects: Vec<Obj> = Vec::new();

    // Lighting and sky.
    let mut sun = serde_json::to_value(
        installed_plugins::lights_plugin::lights_component::LightComponent::default(),
    )
    .unwrap();
    sun["general"]["light_type"] = json!("Directional");
    sun["intensity"]["intensity"] = json!(3.0);
    let mut sun_obj = Obj::new("sun", "Sun", [0.0, 0.0, 0.0])
        .rotate([-48.0, -35.0, 0.0])
        .with("LightComponent", sun);
    sun_obj.object_type = json!({ "Light": "Directional" });
    objects.push(sun_obj);
    objects.push(Obj::new("sky", "Sky", [0.0, 0.0, 0.0]).typed(
        "AtmosphereComponent",
        &installed_plugins::sky_plugin::sky_component::AtmosphereComponent::default(),
    ));
    objects.push(
        Obj::new("spectator_camera", "Spectator Camera", [0.0, 15.0, 36.0])
            .rotate([-18.0, 0.0, 0.0])
            .typed(
                "CameraComponent",
                &helio_component::components::CameraComponent::default(),
            ),
    );

    // Ground and boundary walls: static boxes.
    let concrete = || look([0.42, 0.44, 0.40, 1.0], 0.9, 0.0, 0.0);
    objects.push(static_box(
        "ground", "Ground", [0.0, -0.5, 0.0], [40.0, 0.5, 40.0], GROUND_MAT, concrete(),
    ));
    let wall = || look([0.55, 0.52, 0.48, 1.0], 0.8, 0.0, 0.0);
    for (id, name, at, half) in [
        ("wall_north", "Wall North", [0.0, 2.0, -40.5], [41.0, 2.0, 0.5]),
        ("wall_south", "Wall South", [0.0, 2.0, 40.5], [41.0, 2.0, 0.5]),
        ("wall_east", "Wall East", [40.5, 2.0, 0.0], [0.5, 2.0, 40.0]),
        ("wall_west", "Wall West", [-40.5, 2.0, 0.0], [0.5, 2.0, 40.0]),
    ] {
        objects.push(static_box(id, name, at, half, GROUND_MAT, wall()));
    }

    // A pyramid of dynamic wooden crates: 5 + 4 + 3 + 2 + 1, a hair apart.
    let step = 1.02;
    let mut n = 0;
    for row in 0..5usize {
        let count = 5 - row;
        for col in 0..count {
            n += 1;
            let x = -8.0 + (col as f64 - (count as f64 - 1.0) / 2.0) * step;
            let y = 0.5 + row as f64 * step + 0.01;
            let tint = if (row + col) % 2 == 0 {
                look([0.72, 0.5, 0.26, 1.0], 0.7, 0.0, 0.0)
            } else {
                look([0.55, 0.36, 0.18, 1.0], 0.7, 0.0, 0.0)
            };
            let mut crate_collider = box_collider([0.5; 3], WOOD_MAT, "Props");
            crate_collider.half_extents = [0.5, 0.5, 0.5];
            objects.push(
                Obj::new(&format!("crate_{n}"), &format!("Pyramid Crate {n}"), [x, y, -6.0])
                    .mesh(CUBE, [1.0; 3], tint, true, 1.0)
                    .typed("ColliderComponent", &crate_collider)
                    .typed("RigidBodyComponent", &body(BodyType::Dynamic, 5.0)),
            );
        }
    }

    // Spheres dropped from height: rubber balls that bounce.
    for (i, (x, y, z)) in [(-2.0, 10.0, -4.0), (0.0, 13.0, -2.0), (2.0, 16.0, -4.0)]
        .into_iter()
        .enumerate()
    {
        let n = i + 1;
        objects.push(dynamic_ball(
            &format!("bouncy_ball_{n}"),
            &format!("Bouncy Ball {n}"),
            [x, y, z],
            0.5,
            1.0,
            look([0.9, 0.15 + 0.3 * i as f32, 0.15, 1.0], 0.35, 0.0, 0.0),
        ));
    }

    // A capsule dropped tilted, built from a cylinder and two caps.
    let capsule_look = || look([0.2, 0.6, 0.9, 1.0], 0.4, 0.1, 0.0);
    objects.push(
        Obj::new("capsule", "Falling Capsule", [3.0, 14.0, 0.0])
            .rotate([0.0, 0.0, 35.0])
            .typed(
                "ColliderComponent",
                &ColliderComponent {
                    shape: ColliderShape::Capsule,
                    radius: 0.4,
                    height: 1.2,
                    ..box_collider([0.5; 3], RUBBER_MAT, "Props")
                },
            )
            .typed("RigidBodyComponent", &body(BodyType::Dynamic, 2.0)),
    );
    let mut part = |id: &str, name: &str, mesh: &str, at: [f64; 3], size: [f64; 3]| {
        let mut o = Obj::new(id, name, at).mesh(mesh, size, capsule_look(), true, 1.0);
        o.parent = Some("capsule".into());
        objects.push(o);
    };
    part("capsule_shaft", "Capsule Shaft", CYLINDER, [0.0; 3], [0.8, 1.2, 0.8]);
    part("capsule_cap_top", "Capsule Cap Top", SPHERE, [0.0, 0.6, 0.0], [0.8; 3]);
    part("capsule_cap_bottom", "Capsule Cap Bottom", SPHERE, [0.0, -0.6, 0.0], [0.8; 3]);

    // A ramp (a rotated static box, ice) and the ball that rolls down it
    // through a trigger volume. +Z rotation lifts the +X end.
    objects.push(static_box(
        "ramp",
        "Ramp",
        [16.0, 2.5, 8.0],
        [5.0, 0.25, 2.5],
        ICE_MAT,
        look([0.6, 0.85, 0.95, 1.0], 0.15, 0.0, 0.0),
    ).rotate([0.0, 0.0, 20.0]));
    objects.push(dynamic_ball(
        "rolling_ball",
        "Rolling Ball",
        [18.56, 4.23, 8.0],
        0.5,
        1.0,
        look([0.95, 0.8, 0.1, 1.0], 0.3, 0.2, 0.0),
    ));
    let mut zone = ColliderComponent {
        is_trigger: true,
        layer: "Triggers".into(),
        ..box_collider([2.0, 1.5, 2.5], "", "Triggers")
    };
    zone.physics_material = String::new();
    objects.push(
        Obj::new("finish_zone", "Finish Trigger Zone", [9.0, 1.5, 8.0])
            .mesh(CUBE, [4.0, 3.0, 5.0], look([0.1, 0.9, 0.3, 1.0], 0.5, 0.0, 1.5), false, 0.25)
            .typed("ColliderComponent", &zone)
            .typed("RigidBodyComponent", &body(BodyType::Static, 0.0)),
    );

    // Force fields: an updraft that holds a light ball, a radial well.
    objects.push(
        Obj::new("updraft", "Updraft Field", [-14.0, 0.2, 8.0])
            .mesh(CYLINDER, [8.0, 0.2, 8.0], look([0.5, 0.8, 1.0, 1.0], 0.6, 0.0, 0.6), false, 0.35)
            .typed(
                "ForceFieldComponent",
                &ForceFieldComponent {
                    enabled: true,
                    kind: ForceFieldKind::Directional,
                    strength: 25.0,
                    direction: [0.0, 1.0, 0.0],
                    radius: 4.0,
                    falloff: FalloffMode::Linear,
                    affected_layer: String::new(),
                },
            ),
    );
    objects.push(dynamic_ball(
        "feather_ball",
        "Feather Ball",
        [-14.0, 3.0, 8.0],
        0.4,
        0.5,
        look([1.0, 1.0, 1.0, 1.0], 0.5, 0.0, 0.0),
    ));
    objects.push(
        Obj::new("well", "Gravity Well", [-14.0, 0.6, -14.0])
            .mesh(SPHERE, [0.6; 3], look([0.7, 0.2, 0.9, 1.0], 0.4, 0.0, 2.0), false, 1.0)
            .typed(
                "ForceFieldComponent",
                &ForceFieldComponent {
                    enabled: true,
                    kind: ForceFieldKind::Radial,
                    strength: 30.0,
                    direction: [0.0, 1.0, 0.0],
                    radius: 7.0,
                    falloff: FalloffMode::Quadratic,
                    affected_layer: String::new(),
                },
            ),
    );
    for (i, (x, z)) in [(-17.0, -14.0), (-11.0, -16.0)].into_iter().enumerate() {
        objects.push(dynamic_ball(
            &format!("well_ball_{}", i + 1),
            &format!("Well Ball {}", i + 1),
            [x, 0.5, z],
            0.5,
            1.0,
            look([0.8, 0.4, 0.95, 1.0], 0.4, 0.0, 0.0),
        ));
    }

    // A ball-socket pendulum: the bob hangs 4 m from a static pivot and is
    // released off to the side.
    objects.push(static_box(
        "pendulum_pivot",
        "Pendulum Pivot",
        [14.0, 9.0, -10.0],
        [0.3, 0.3, 0.3],
        GROUND_MAT,
        look([0.2, 0.2, 0.25, 1.0], 0.4, 0.8, 0.0),
    ));
    objects.push(
        dynamic_ball(
            "pendulum_bob",
            "Pendulum Bob",
            [16.8, 6.2, -10.0],
            0.6,
            3.0,
            look([0.85, 0.1, 0.1, 1.0], 0.25, 0.6, 0.0),
        )
        .typed(
            "JointComponent",
            &JointComponent {
                kind: JointKind::BallSocket,
                connected_object: "Pendulum Pivot".into(),
                anchor: [-2.8, 2.8, 0.0],
                connected_anchor: [0.0; 3],
                axis: [0.0, 1.0, 0.0],
                collide_connected: false,
                break_force: 0.0,
                limits_enabled: false,
                limit_min: 0.0,
                limit_max: 0.0,
                stiffness: 0.0,
                damping: 0.0,
                rest_length: 0.0,
            },
        ),
    );

    // A hinged door (limited to +-90 degrees) and a ball thrown at it.
    objects.push(static_box(
        "hinge_post",
        "Hinge Post",
        [-6.0, 2.0, 14.0],
        [0.2, 2.0, 0.2],
        GROUND_MAT,
        look([0.3, 0.3, 0.32, 1.0], 0.5, 0.6, 0.0),
    ));
    let mut door_collider = box_collider([1.5, 1.5, 0.1], WOOD_MAT, "Props");
    door_collider.center = [0.0; 3];
    objects.push(
        Obj::new("swing_door", "Swing Door", [-4.3, 2.0, 14.0])
            .mesh(CUBE, [3.0, 3.0, 0.2], look([0.3, 0.5, 0.8, 1.0], 0.6, 0.0, 0.0), true, 1.0)
            .typed("ColliderComponent", &door_collider)
            .typed("RigidBodyComponent", &body(BodyType::Dynamic, 8.0))
            .typed(
                "JointComponent",
                &JointComponent {
                    kind: JointKind::Hinge,
                    connected_object: "Hinge Post".into(),
                    anchor: [-1.7, 0.0, 0.0],
                    connected_anchor: [0.0; 3],
                    axis: [0.0, 1.0, 0.0],
                    collide_connected: false,
                    break_force: 0.0,
                    limits_enabled: true,
                    limit_min: -90.0,
                    limit_max: 90.0,
                    stiffness: 0.0,
                    damping: 0.0,
                    rest_length: 0.0,
                },
            ),
    );
    let mut thrown = body(BodyType::Dynamic, 2.0);
    thrown.linear_velocity = [0.0, 0.0, -12.0];
    objects.push(
        dynamic_ball(
            "door_ball",
            "Door Ball",
            [-4.3, 3.0, 20.0],
            0.5,
            2.0,
            look([0.1, 0.9, 0.9, 1.0], 0.3, 0.0, 0.0),
        )
        .replace_body(&thrown),
    );

    let children_of = |id: &str| -> Vec<String> {
        objects
            .iter()
            .filter(|o| o.parent.as_deref() == Some(id))
            .map(|o| o.id.clone())
            .collect()
    };
    json!({
        "version": "2.2",
        "objects": objects.iter().map(|o| o.json(&children_of(&o.id))).collect::<Vec<_>>(),
        "world_settings": { "gravity": [0.0, -9.81, 0.0], "physics_enabled": true },
        "metadata": {},
        "editor": { "camera": { "position": [0.0, 15.0, 36.0], "yaw": 0.0, "pitch": -0.31 } },
    })
}

impl Obj {
    /// Swap the object's `RigidBodyComponent` record for `value`.
    fn replace_body(mut self, value: &RigidBodyComponent) -> Self {
        for c in &mut self.components {
            if c["class_name"] == "RigidBodyComponent" {
                c["data"] = serde_json::to_value(value).unwrap();
            }
        }
        self
    }
}

fn physics_assets() -> Vec<(&'static str, String)> {
    let material = |friction, restitution, density, surface, description: &str| {
        PhysicsMaterial {
            friction,
            restitution,
            friction_combine: CombineRule::Average,
            restitution_combine: CombineRule::Average,
            density,
            surface,
            description: description.into(),
            ..PhysicsMaterial::default()
        }
        .to_text()
    };
    let mut layers = CollisionLayers::default();
    let layer = |name: &str, with: &[&str], color: &str, about: &str| {
        let mut l = Layer::new(name);
        l.collides_with = with.iter().map(|s| s.to_string()).collect();
        l.color = Some(color.into());
        l.description = about.into();
        l
    };
    layers.layers = vec![
        layer("Default", &["Default", "World", "Props", "Triggers"], "#9aa0a6", "Everything unlabelled"),
        layer("World", &["Default", "World", "Props"], "#6b7280", "Ground and walls"),
        layer("Props", &["Default", "World", "Props", "Triggers"], "#f59e0b", "Dynamic things"),
        layer("Triggers", &["Default", "Props"], "#22c55e", "Overlap volumes: see Props only"),
    ];
    assert!(layers.validate().is_empty(), "{:?}", layers.validate());
    vec![
        ("concrete.physmat", material(0.8, 0.0, 2400.0, SurfaceType::Concrete, "Ground, walls and posts: grippy, dead")),
        ("wood.physmat", material(0.6, 0.1, 600.0, SurfaceType::Wood, "Crates, the door: moderate grip, barely bounces")),
        ("rubber.physmat", material(0.9, 0.85, 1100.0, SurfaceType::Rubber, "Balls and the capsule: grippy and very bouncy")),
        ("ice.physmat", material(0.03, 0.05, 900.0, SurfaceType::Ice, "The ramp: almost frictionless")),
        ("playground.physlayers", layers.to_text()),
    ]
}

#[test]
#[ignore = "regenerates assets/examples/physics_playground; run on purpose"]
fn generate_physics_playground() {
    let dir = example_dir();
    std::fs::create_dir_all(dir.join("scene")).unwrap();
    std::fs::create_dir_all(dir.join("assets/physics")).unwrap();
    std::fs::write(
        dir.join("scene/default.level"),
        serde_json::to_string_pretty(&build_level()).unwrap() + "\n",
    )
    .unwrap();
    for (name, text) in physics_assets() {
        std::fs::write(dir.join("assets/physics").join(name), text).unwrap();
    }
    assert!(LAYERS.ends_with("playground.physlayers"));
}

// ── The packaged proof ─────────────────────────────────────────────────────

#[derive(Default)]
struct Observed {
    kind: Option<SessionKind>,
    crates: Vec<[f64; 3]>,
    others: Vec<(String, [f64; 3])>,
    ball_collisions: usize,
    triggers: Vec<u64>,
    rolling_ball_bits: u64,
    running_after: bool,
    live_threads_after: usize,
}

fn live_physics_threads() -> usize {
    editor_task_queue::thread_snapshots()
        .into_iter()
        .filter(|t| t.name == "Pulsar-Physics" && t.state != editor_task_queue::ThreadState::Exited)
        .count()
}

fn run_cooked(content_dir: &Path, frames: u64) -> Observed {
    let content = ContentRoot::open(content_dir).unwrap();
    content.install();
    let mut game = TickLoop::new(TickMode::Fixed { dt: HEADLESS_STEP }, 1);
    let seen = Arc::new(Mutex::new(Observed::default()));
    let collisions = Arc::new(AtomicUsize::new(0));
    let triggers = Arc::new(Mutex::new(Vec::new()));
    let hooks = HeadlessHooks {
        after_load: Some(Box::new({
            let seen = Arc::clone(&seen);
            let collisions = Arc::clone(&collisions);
            let triggers = Arc::clone(&triggers);
            move |game: &mut TickLoop| {
                let mut seen = seen.lock().unwrap();
                seen.kind = Some(game.session_kind());
                let register = |descriptor| {
                    game.events
                        .register(descriptor, pulsar_events::EventCategory::Physics)
                        .expect("the physics event registers")
                };
                let collision_id = register(COLLISION_ENTER.descriptor());
                let trigger_id = register(TRIGGER_ENTER.descriptor());
                let (ball, zone, rolling) = {
                    let store = game.scene_store.read();
                    (
                        store.world.entity_for("bouncy_ball_1").unwrap(),
                        store.world.entity_for("finish_zone").unwrap(),
                        store.world.entity_for("rolling_ball").unwrap(),
                    )
                };
                seen.rolling_ball_bits = rolling.bits();
                let bus = game.events.bus();
                let subscriptions = [
                    bus.subscribe_dyn(
                        collision_id,
                        SubscribeOptions::channel(Channel::Entity(ball.bits())),
                        move |_| {
                            collisions.fetch_add(1, Ordering::SeqCst);
                        },
                    ),
                    bus.subscribe_dyn(
                        trigger_id,
                        SubscribeOptions::channel(Channel::Entity(zone.bits())),
                        move |event| {
                            if let DynValue::U64(other) = event.fields[2] {
                                triggers.lock().unwrap().push(other);
                            }
                        },
                    ),
                ];
                std::mem::forget(subscriptions);
            }
        })),
        after_tick: Some(Box::new(|game: &mut TickLoop, _| wait_idle(game.session()))),
    };
    standalone::run_headless_with(&mut game, &content, frames, hooks).expect("headless run");

    let store = game.scene_store.read();
    let at = |id: &str| {
        store
            .world
            .get::<Transform>(store.world.entity_for(id).unwrap())
            .unwrap()
            .position
    };
    let mut seen = std::mem::take(&mut *seen.lock().unwrap());
    seen.crates = (1..=15).map(|n| at(&format!("crate_{n}"))).collect();
    for id in [
        "bouncy_ball_1",
        "bouncy_ball_2",
        "bouncy_ball_3",
        "capsule",
        "rolling_ball",
        "feather_ball",
        "well_ball_1",
        "well_ball_2",
        "pendulum_bob",
        "swing_door",
        "door_ball",
    ] {
        seen.others.push((id.to_string(), at(id)));
    }
    seen.ball_collisions = collisions.load(Ordering::SeqCst);
    seen.triggers = triggers.lock().unwrap().clone();
    seen.running_after = simulation_running(game.session());
    seen.live_threads_after = live_physics_threads();
    seen
}

#[test]
fn the_example_cooks_and_runs_its_physics() {
    let _guard = ONE_AT_A_TIME.lock().unwrap_or_else(|p| p.into_inner());
    let project = example_dir();
    assert!(
        project.join("scene/default.level").is_file(),
        "run the generator first (see the module docs)"
    );
    let out = tempfile::tempdir().unwrap();
    let mut options = PackageOptions::new(&project, out.path());
    options.loose = true;
    options.skip_build = true;
    let report = package(&options).unwrap_or_else(|e| panic!("cooking failed: {e}"));
    assert_eq!(report.startup_level.as_deref(), Some("scene/default.level"));
    assert!(
        // The sun light's default disk texture is the string "None": an engine quirk, harmless.
        report.warnings.iter().all(|w| w.contains("emitter_disk.texture")),
        "level warnings: {:?}",
        report.warnings
    );
    let content = out.path().join("Content");
    for file in ["scene/default.level", "Pulsar/project.json", "assets/meshes/primitives/SM_Cube.fbx"] {
        assert!(content.join(file).is_file(), "Content/{file} is cooked");
    }
    let cooked: Value =
        serde_json::from_slice(&std::fs::read(content.join("scene/default.level")).unwrap())
            .unwrap();
    assert_eq!(cooked["world_settings"]["gravity"], json!([0.0, -9.81, 0.0]));
    assert_eq!(cooked["world_settings"]["physics_enabled"], json!(true));

    // 600 ticks: 10 s of simulated time.
    let seen = run_cooked(&content, 600);
    assert_eq!(seen.kind, Some(SessionKind::Headless));

    // The pyramid and everything else: finite, above the ground, no tunneling.
    let finite = |p: &[f64; 3]| p.iter().all(|v| v.is_finite());
    for (n, p) in seen.crates.iter().enumerate() {
        assert!(finite(p), "crate {} position {p:?}", n + 1);
        assert!(
            (0.4..6.0).contains(&p[1]),
            "crate {} settled above the ground (centre y >= 0.5 - skin): {p:?}",
            n + 1
        );
        assert!(p[0].abs() < 14.0 && p[2].abs() < 14.0, "crate {} stayed in the arena: {p:?}", n + 1);
    }
    for (id, p) in &seen.others {
        assert!(finite(p), "{id} position {p:?}");
        assert!(p[1] > -0.2, "{id} fell through the ground: {p:?}");
        assert!(p[0].abs() < 40.0 && p[2].abs() < 40.0, "{id} left the arena: {p:?}");
    }

    // Events: a ball hit something, the rolling ball went through the trigger.
    assert!(seen.ball_collisions >= 1, "collision_enter for a bouncy ball");
    assert!(
        seen.triggers.contains(&seen.rolling_ball_bits),
        "trigger_enter reported the rolling ball: {:?}",
        seen.triggers
    );

    // The pendulum held: the bob is still 3.96 m from its pivot (14, 9, -10).
    let bob = seen.others.iter().find(|(id, _)| id == "pendulum_bob").unwrap().1;
    let reach = ((bob[0] - 14.0).powi(2) + (bob[1] - 9.0).powi(2) + (bob[2] + 10.0).powi(2)).sqrt();
    assert!(
        (reach - 3.96).abs() < 0.3,
        "the ball-socket joint held its length: {reach} (bob {bob:?})"
    );
    assert!(bob[1] > 4.0, "the bob hangs from the pivot: {bob:?}");

    // The session ended and the physics thread was joined.
    assert!(!seen.running_after);
    assert_eq!(seen.live_threads_after, 0, "no Pulsar-Physics thread leaked");
}
