//! `Transform` through the script VM: component methods, the motion gate,
//! and typed failures.

use std::sync::Arc;

use glam::Vec3;
use pulsar_scene_model::motion::{ensure_can_move, MotionGate};
use pulsar_scene_model::Transform;
use pulsar_scenedb::{ComponentChangeKind, Entity, World};
use pulsar_script_math::literal;
use pulsar_script_vm::{
    Budget, Constant, Function, Host, Import, Instr, Module, NativeRegistry, Param, Program,
    ScriptError, ScriptErrorKind, Signature, Type, Value, Vm,
};

use pulsar_world_registry as _;

/// Stand-in for the renderer's `Movability` component: an entity with this
/// is pinned in place.
#[derive(Debug)]
struct Pinned;

inventory::submit! {
    MotionGate {
        name: "Pinned",
        check: |world, entity| {
            if world.get::<Pinned>(entity).is_some() { Err("the object is pinned".into()) } else { Ok(()) }
        },
    }
}

fn transform() -> Type {
    Type::component("Transform")
}

fn vec3() -> Type {
    Type::object("Vec3")
}

fn vec3_const(x: f32, y: f32, z: f32) -> Constant {
    Constant::Value {
        ty: "Vec3".into(),
        json: literal::vec3(Vec3::new(x, y, z)),
    }
}

/// `fn(entity) { Transform::of(entity).<method>(<vec3 literal>) }`
fn call_with_vec3(method: &str, literal: Constant) -> Program {
    let mut module = Module::new("script");
    module.imports = vec![
        Import {
            name: "Transform::of".into(),
            sig: Signature::new(vec![Param::new(Type::Entity)], transform()),
        },
        Import {
            name: format!("Transform::{method}"),
            sig: Signature::new(
                vec![Param::new(transform()), Param::new(vec3())],
                Type::Unit,
            ),
        },
    ];
    module.constants = vec![literal];
    module.functions = vec![Function {
        name: "run".into(),
        exported: true,
        params: vec![Type::Entity],
        ret: Type::Unit,
        registers: vec![Type::Entity, transform(), vec3()],
        code: vec![
            Instr::CallNative {
                import: 0,
                args: vec![0],
                dst: Some(1),
            },
            Instr::Const { dst: 2, index: 0 },
            Instr::CallNative {
                import: 1,
                args: vec![1, 2],
                dst: None,
            },
            Instr::Return { value: None },
        ],
        debug: None,
    }];
    Program::link(Arc::new(module), &NativeRegistry::with_engine_natives()).expect("link")
}

fn run(program: &Program, world: &mut World, target: Entity) -> Result<Value, ScriptError> {
    let me = world.spawn();
    let mut instance = program.instantiate();
    let func = program.entry("run").unwrap();
    let mut host = Host::new(world, me);
    Vm::new().call(
        program,
        &mut instance,
        func,
        &[Value::Entity(target)],
        &mut host,
        &mut Budget::new(1000),
    )
}

fn object(world: &mut World) -> Entity {
    let e = world.spawn();
    world.insert(
        e,
        Transform {
            position: [1.0, 2.0, 3.0],
            ..Default::default()
        },
    );
    e
}

#[test]
fn scripts_move_an_object_through_transform_methods() {
    let mut world = World::new();
    let e = object(&mut world);
    run(
        &call_with_vec3("set_position", vec3_const(5.0, 6.0, 7.0)),
        &mut world,
        e,
    )
    .unwrap();
    assert_eq!(world.get::<Transform>(e).unwrap().position, [5.0, 6.0, 7.0]);

    run(
        &call_with_vec3("translate", vec3_const(1.0, 1.0, 1.0)),
        &mut world,
        e,
    )
    .unwrap();
    assert_eq!(world.get::<Transform>(e).unwrap().position, [6.0, 7.0, 8.0]);

    run(
        &call_with_vec3("set_rotation_degrees", vec3_const(0.0, 90.0, 0.0)),
        &mut world,
        e,
    )
    .unwrap();
    assert_eq!(
        world.get::<Transform>(e).unwrap().rotation,
        [0.0, 90.0, 0.0]
    );

    run(
        &call_with_vec3("set_scale", vec3_const(2.0, 2.0, 2.0)),
        &mut world,
        e,
    )
    .unwrap();
    assert_eq!(world.get::<Transform>(e).unwrap().scale, [2.0, 2.0, 2.0]);
}

#[test]
fn writes_are_journaled_like_any_other_mutation() {
    let mut world = World::new();
    let e = object(&mut world);
    let program = call_with_vec3("set_position", vec3_const(9.0, 9.0, 9.0));
    let mut cursor = world.open_change_cursor::<Transform>();
    run(&program, &mut world, e).unwrap();
    let mut changes = Vec::new();
    let _ = world.read_changes(&mut cursor, &mut changes);
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(changes[0].kind, ComponentChangeKind::Mutated);
}

#[test]
fn a_gated_object_cannot_be_moved_and_is_left_untouched() {
    let mut world = World::new();
    let e = object(&mut world);
    world.insert(e, Pinned);
    assert!(ensure_can_move(&world, e).is_err());

    let program = call_with_vec3("set_position", vec3_const(5.0, 5.0, 5.0));
    let err = run(&program, &mut world, e).unwrap_err();
    assert!(
        matches!(&err.kind, ScriptErrorKind::Native { name, message }
        if name == "Transform::set_position" && message.contains("pinned")),
        "{err}"
    );
    assert_eq!(world.get::<Transform>(e).unwrap().position, [1.0, 2.0, 3.0]);

    // Every setter is gated, not just set_position.
    for method in ["translate", "set_rotation_degrees", "set_scale"] {
        let err = run(
            &call_with_vec3(method, vec3_const(1.0, 1.0, 1.0)),
            &mut world,
            e,
        )
        .unwrap_err();
        assert!(
            matches!(err.kind, ScriptErrorKind::Native { .. }),
            "{method}: {err}"
        );
    }
    assert_eq!(
        *world.get::<Transform>(e).unwrap(),
        Transform {
            position: [1.0, 2.0, 3.0],
            ..Default::default()
        }
    );

    // Removing the pin lets it move again.
    world.remove::<Pinned>(e);
    run(&program, &mut world, e).unwrap();
    assert_eq!(world.get::<Transform>(e).unwrap().position, [5.0, 5.0, 5.0]);
}

#[test]
fn stale_and_missing_targets_fail_instead_of_writing() {
    let mut world = World::new();
    let program = call_with_vec3("set_position", vec3_const(1.0, 1.0, 1.0));

    let dead = object(&mut world);
    world.despawn(dead);
    assert!(
        run(&program, &mut world, dead).is_err(),
        "a despawned entity has no Transform reference"
    );

    let bare = world.spawn();
    let err = run(&program, &mut world, bare).unwrap_err();
    assert!(matches!(err.kind, ScriptErrorKind::Native { .. }), "{err}");
    assert!(world.get::<Transform>(bare).is_none());

    assert!(ensure_can_move(&world, Entity::DANGLING).is_err());
}

#[test]
fn getters_are_side_effect_free_but_not_deterministic() {
    let registry = NativeRegistry::with_engine_natives();
    for name in [
        "Transform::position",
        "Transform::rotation_degrees",
        "Transform::scale",
    ] {
        let native = registry
            .get(name)
            .unwrap_or_else(|| panic!("{name} is registered"));
        assert!(native.flags.side_effect_free, "{name}");
        assert!(
            !native.flags.deterministic,
            "{name}: a world read must not be constant-folded"
        );
    }
    for name in [
        "Transform::set_position",
        "Transform::translate",
        "Transform::set_scale",
    ] {
        assert!(
            !registry.get(name).unwrap().flags.side_effect_free,
            "{name}"
        );
    }
}

/// Mutation equivalence (Pulsar-Native#1035 acceptance, #1081): a script's
/// write reaches the transform's GPU row exactly as a typed write of the
/// same value does, with no refresh call.
#[test]
fn a_script_write_reaches_the_gpu_row_like_a_typed_write() {
    use pulsar_scenedb::gpu::{
        BufferKey, EngineGpuContext, GpuMirrorHandle, SceneGpuConfig, SceneGpuStore,
    };

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else {
        return;
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let ctx = EngineGpuContext::new(Arc::new(device), Arc::new(queue));
    let store = Arc::new(SceneGpuStore::new(
        &ctx,
        SceneGpuConfig {
            classes: vec![],
            tombstone_headroom: 0,
            max_cells_metadata: 0,
        },
    ));
    let mut world = World::new();
    world.attach_gpu_mirror(GpuMirrorHandle::new(
        Arc::clone(&store),
        Arc::clone(ctx.queue()),
    ));

    let scripted = object(&mut world);
    let typed = object(&mut world);
    world.flush_gpu_mirror(ctx.queue()).unwrap();

    run(
        &call_with_vec3("set_position", vec3_const(5.0, 6.0, 7.0)),
        &mut world,
        scripted,
    )
    .unwrap();
    run(
        &call_with_vec3("set_scale", vec3_const(2.0, 2.0, 2.0)),
        &mut world,
        scripted,
    )
    .unwrap();
    {
        let mut t = world.get_mut::<Transform>(typed).unwrap();
        t.position = [5.0, 6.0, 7.0];
        t.scale = [2.0, 2.0, 2.0];
    }
    world.flush_gpu_mirror(ctx.queue()).unwrap();

    // `Transform::packed`: position, rotation, scale; 36 bytes per entity.
    let handle = store
        .resolve_buffer_handle(BufferKey::of("Transform::packed"))
        .unwrap();
    let rows = 1 + scripted.index().max(typed.index()) as u64;
    let staging = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: rows * 36,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = ctx.device().create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&handle.buffer, 0, &staging, 0, rows * 36);
    ctx.queue().submit([encoder.finish()]);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device()
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let words: Vec<f32> = staging
        .slice(..)
        .get_mapped_range()
        .unwrap()
        .chunks_exact(4)
        .map(|w| f32::from_ne_bytes(w.try_into().unwrap()))
        .collect();
    let row = |e: Entity| words[e.index() as usize * 9..e.index() as usize * 9 + 9].to_vec();
    assert_eq!(row(scripted), row(typed));
    assert_eq!(row(scripted), [5.0, 6.0, 7.0, 0.0, 0.0, 0.0, 2.0, 2.0, 2.0]);
}
