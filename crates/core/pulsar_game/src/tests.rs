#[cfg(test)]
mod ecs {
    use crate::prelude::*;
    #[allow(unused_imports)]
    use engine_backend::scene::SceneWorldExt;

    #[derive(Debug, PartialEq)]
    struct Pos {
        x: f32,
        y: f32,
    }
    #[derive(Debug, PartialEq)]
    struct Vel {
        dx: f32,
        dy: f32,
    }
    #[derive(Debug, PartialEq)]
    struct Health(u32);

    #[test]
    fn spawn_and_query() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Pos { x: 1.0, y: 2.0 });
        world.insert(e, Vel { dx: 0.5, dy: 0.0 });

        let mut found = false;
        for (entity, (pos, vel)) in world.query::<(&Pos, &Vel)>() {
            assert_eq!(entity, e);
            assert_eq!(pos.x, 1.0);
            assert_eq!(vel.dx, 0.5);
            found = true;
        }
        assert!(found);
    }

    #[test]
    fn component_overwrite() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Health(100));
        world.insert(e, Health(50)); // overwrite
        assert_eq!(world.get::<Health>(e).unwrap().0, 50);
    }

    #[test]
    fn remove_component() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Pos { x: 3.0, y: 4.0 });
        world.insert(e, Vel { dx: 1.0, dy: 1.0 });
        let removed = world.remove::<Vel>(e);
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().dx, 1.0);
        // Entity still alive and has Pos but not Vel.
        assert!(world.get::<Pos>(e).is_some());
        assert!(world.get::<Vel>(e).is_none());
    }

    #[test]
    fn despawn_invalidates_entity() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Health(10));
        assert!(world.is_alive(e));
        world.despawn(e);
        assert!(!world.is_alive(e));
        assert!(world.get::<Health>(e).is_none());
    }

    #[test]
    fn many_entities_query() {
        let mut world = World::new();
        for i in 0..1000u32 {
            let e = world.spawn();
            world.insert(e, Health(i));
        }
        let count = world.query::<&Health>().count();
        assert_eq!(count, 1000);
    }

    #[test]
    fn slot_recycling_bumps_generation() {
        let mut world = World::new();
        let e1 = world.spawn();
        world.despawn(e1);
        let e2 = world.spawn();
        // Same index, different generation.
        assert_eq!(e1.index(), e2.index());
        assert_ne!(e1.generation(), e2.generation());
        assert!(!world.is_alive(e1));
        assert!(world.is_alive(e2));
    }
}

#[cfg(test)]
mod actors {
    use crate::prelude::*;
    #[allow(unused_imports)]
    use engine_backend::scene::SceneWorldExt;
    use std::sync::{Arc, Mutex};

    struct Counter(Arc<Mutex<Vec<&'static str>>>);
    impl Actor for Counter {
        fn begin_play(&mut self, _e: Entity, _w: &mut World) {
            self.0.lock().unwrap().push("begin");
        }
        // `Actor::tick` (from `pulsar_scenedb`) is deliberately time-free as
        // of the 2026-08-15 rev bump (Pulsar-Native#561 Phase D) -- see that
        // trait's own doc: per-frame timing is the engine's concern, not
        // the data layer's. No `GameTime` parameter anymore.
        fn tick(&mut self, _e: Entity, _w: &mut World) {
            self.0.lock().unwrap().push("tick");
        }
        fn end_play(&mut self, _e: Entity, _w: &mut World) {
            self.0.lock().unwrap().push("end");
        }
    }

    #[test]
    fn lifecycle_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut tick_loop = TickLoop::new(TickMode::default(), 0);
        // #634: actors register into the loop's SHARED scene store (the one
        // renderers read), not a private world.
        let entity = {
            let mut store = tick_loop.scene_store.write();
            tick_loop
                .actors
                .register(Counter(log.clone()), &mut store.world)
        };
        tick_loop.tick_once();
        {
            let mut store = tick_loop.scene_store.write();
            tick_loop.actors.deregister(entity, &mut store.world);
        }
        let events = log.lock().unwrap().clone();
        assert_eq!(events, vec!["begin", "tick", "end"]);
    }

    /// #634: a mutation an actor/system makes through the shared store is
    /// visible to any other holder of the same handle (e.g. the renderer)
    /// -- there is exactly one world, and the tick loop mutates it.
    #[test]
    fn actor_mutations_land_in_the_shared_store() {
        let mut tick_loop = TickLoop::new(TickMode::default(), 0);

        let entity = {
            let mut store = tick_loop.scene_store.write();
            let e = store.world.spawn_object(engine_backend::scene::SpawnObject::new("Runtime")).unwrap();
            tick_loop
                .actors
                .register(Counter(Arc::new(Mutex::new(Vec::new()))), &mut store.world);
            e
        };

        tick_loop.tick_once();

        // Another handle-holder (what the renderer is) sees the spawned
        // object and its components.
        let store = tick_loop.scene_store.read();
        assert_eq!(store.world.get::<engine_backend::scene::Name>(entity).map(|n| n.0.as_str()), Some("Runtime"));
    }
}

#[cfg(test)]
mod schedule_tests {
    use crate::prelude::*;
    #[allow(unused_imports)]
    use engine_backend::scene::SceneWorldExt;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct Count(#[allow(dead_code)] u32);

    #[test]
    fn systems_run_in_order() {
        let order = Arc::new(Mutex::new(Vec::<u32>::new()));
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Count(0));

        let o1 = order.clone();
        let o2 = order.clone();

        let mut sched = Schedule::new();
        sched.add_system("first", move |_w: &mut World, _t| {
            o1.lock().unwrap().push(1);
        });
        sched.add_system("second", move |_w: &mut World, _t| {
            o2.lock().unwrap().push(2);
        });

        // `Schedule::run` (from `pulsar_scenedb`) requires
        // `pulsar_scenedb::GameTime`, not the prelude's `pulsar_core::GameTime`
        // — see note in the `actors` test module above.
        let time = pulsar_scenedb::GameTime {
            elapsed: std::time::Duration::ZERO,
            delta: std::time::Duration::from_millis(16),
            tick: 0,
        };
        sched.run(&mut world, time);

        assert_eq!(*order.lock().unwrap(), vec![1, 2]);
    }
}

/// Level bindings and project discovery on the script runtime
/// (`crate::scripting`).
#[cfg(test)]
mod script_runtime_bindings {
    use crate::scripting::{self, instance_id_for, BindingError};
    use engine_backend::scene::{RuntimeLevel, SceneWorldExt};
    use pulsar_script_vm::{BinOp, Function, Instr, Module, Type, Value, Variable};

    const BINDINGS_FIXTURE: &str =
        include_str!("../tests/fixtures/level_bindings_sample.level.json");

    /// `total += speed * delta_time` every tick.
    fn tick_probe_module() -> Module {
        let mut m = Module::new("TickProbe");
        m.variables = vec![
            Variable { name: "speed".into(), ty: Type::Float, default: None },
            Variable { name: "total".into(), ty: Type::Float, default: None },
        ];
        m.functions = vec![Function {
            name: "tick".into(),
            exported: true,
            params: vec![Type::Float],
            ret: Type::Unit,
            registers: vec![Type::Float, Type::Float, Type::Float],
            code: vec![
                Instr::LoadVar { dst: 1, var: 0 },
                Instr::Binary { op: BinOp::Mul, dst: 1, a: 1, b: 0 },
                Instr::LoadVar { dst: 2, var: 1 },
                Instr::Binary { op: BinOp::Add, dst: 2, a: 2, b: 1 },
                Instr::StoreVar { var: 1, src: 2 },
                Instr::Return { value: None },
            ],
        }];
        m
    }

    #[test]
    fn module_classes_bind_to_their_objects_and_tick() {
        let root = std::env::temp_dir().join(format!("pulsar_game_script_bindings_{}", std::process::id()));
        let module_path = scripting::module_path_for_class(&root, "TickProbe");
        std::fs::create_dir_all(module_path.parent().unwrap()).unwrap();
        std::fs::write(&module_path, tick_probe_module().to_json().unwrap()).unwrap();

        let file: pulsar_scene::SceneFile = serde_json::from_str(BINDINGS_FIXTURE).unwrap();
        let level = RuntimeLevel::from_scene_file(file).unwrap();
        let store = level.scene();
        let mut bindings = level.extras().blueprint_bindings.clone();
        drop(level);
        // A class with no compiled module is a reported failure.
        bindings.get_mut("lever_a").unwrap().push(pulsar_scene::format::BlueprintBinding {
            class_name: "LegacyOnly".into(),
            overrides: Default::default(),
        });

        let mut runtime = scripting::new_runtime();
        let report = {
            let guard = store.read();
            scripting::apply_script_bindings(&mut runtime, &guard, &root, &bindings)
        };
        assert_eq!(report.applied.len(), 2);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].class_name, "LegacyOnly");
        assert!(matches!(report.failures[0].error, BindingError::ModuleMissing { .. }));

        let a = instance_id_for("lever_a", "TickProbe");
        let b = instance_id_for("lever_b", "TickProbe");
        {
            let guard = store.read();
            assert_eq!(runtime.entity_of(&a), guard.world.entity_for("lever_a"));
        }
        let mut guard = store.write();
        runtime.dispatch_pending_begin_play(&mut guard.world);
        runtime.tick_all(&mut guard.world, 1.0);
        runtime.tick_all(&mut guard.world, 1.0);
        assert_eq!(runtime.variable(&a, "total"), Some(&Value::Float(5.0)));
        assert_eq!(runtime.variable(&b, "total"), Some(&Value::Float(18.0)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn world_lookup_natives_find_level_objects() {
        use pulsar_script_vm::Host;
        let file: pulsar_scene::SceneFile = serde_json::from_str(BINDINGS_FIXTURE).unwrap();
        let level = RuntimeLevel::from_scene_file(file).unwrap();
        let store = level.scene();
        drop(level);
        let runtime = scripting::new_runtime();
        let find = runtime.natives().get("world::find_by_stable_id").expect("registered");
        let mut guard = store.write();
        let expected = guard.world.entity_for("lever_b").unwrap();
        let mut host = Host::new(&mut guard.world, pulsar_scenedb::Entity::DANGLING);
        let found = find.call(&mut host, &mut [Value::from("lever_b")]).unwrap();
        assert_eq!(found, Value::Entity(expected));
        let missing = find.call(&mut host, &mut [Value::from("nope")]).unwrap();
        assert_eq!(missing, Value::Entity(pulsar_scenedb::Entity::DANGLING));
        assert!(runtime.natives().get("world::find_by_name").is_some());
    }

    #[test]
    fn project_discovery_loads_module_classes() {
        let root = std::env::temp_dir().join(format!("pulsar_game_script_discovery_{}", std::process::id()));
        let module_path = scripting::module_path_for_class(&root, "TickProbe");
        std::fs::create_dir_all(module_path.parent().unwrap()).unwrap();
        std::fs::write(&module_path, tick_probe_module().to_json().unwrap()).unwrap();
        std::fs::create_dir_all(root.join("src/classes/NoModule/events/.build")).unwrap();

        let mut runtime = scripting::new_runtime();
        let loaded = scripting::load_project_classes(&mut runtime, &root.join("src/classes"));
        assert_eq!(loaded, ["TickProbe"]);
        assert_eq!(runtime.instance_ids(), ["TickProbe__vm_default"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
