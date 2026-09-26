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

/// Script helpers on the engine VM (`crate::scripting`): world lookup
/// natives and component-slot binding. The world-driven lifecycle is tested
/// in `scripting::tests`.
#[cfg(test)]
mod script_runtime_bindings {
    use crate::scripting;
    use engine_backend::scene::{RuntimeLevel, SceneWorldExt};
    use pulsar_script_vm::{Module, Type, Value, Variable};

    const BINDINGS_FIXTURE: &str =
        include_str!("../tests/fixtures/level_bindings_sample.level.json");

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

    /// #921: binding a script instance to a placed class fills each hidden
    /// `__slot:<uuid>` variable once with a handle to that instance's real
    /// component (here the second LightComponent, on a generated child).
    /// A slot the instance does not have stays `none`, reported; there is
    /// no fallback to the root.
    #[test]
    fn class_slot_handles_are_resolved_once_at_bind() {
        use helio_component::components::LightComponent;

        let root = std::env::temp_dir().join(format!("pulsar_game_slot_bind_{}", std::process::id()));
        let class_dir = root.join("src/classes/Lamp");
        std::fs::create_dir_all(&class_dir).unwrap();
        let light = serde_json::to_value(LightComponent::default()).unwrap();
        std::fs::write(
            class_dir.join("prefab.json"),
            serde_json::json!({
                "prefab_version": 1, "name": "Lamp",
                "components": [
                    { "class_name": "LightComponent", "enabled": true, "data": light },
                    { "class_name": "LightComponent", "enabled": true, "data": light }
                ]
            })
            .to_string(),
        )
        .unwrap();
        let registry = pulsar_class::ClassRegistry::scan(&root);
        let def = registry.by_name("Lamp").unwrap().load_definition().unwrap();
        let second_slot = def.prefab.components[1].slot_id.clone();
        let missing_slot = pulsar_class::new_slot_id();

        let mut scene = engine_backend::scene::new_scene();
        let placement = pulsar_class::world::instantiate_class(
            &mut scene.world,
            &def,
            pulsar_class::ClassInstance::default(),
            engine_backend::scene::SpawnObject::new("Lamp").with_id("lamp"),
        )
        .unwrap();
        let child = placement.handle(&second_slot).unwrap().entity;
        assert_ne!(child, placement.root(), "second copy lives on a generated child");

        let mut module = Module::new("Lamp");
        for slot in [&second_slot, &missing_slot] {
            module.variables.push(Variable {
                name: pulsar_class::slot_variable_name(slot),
                ty: Type::Component("LightComponent".into()),
                default: None,
            });
        }
        let mut runtime = scripting::new_runtime();
        runtime.load_class(module).unwrap();
        runtime.spawn("lamp::Lamp", "Lamp", Some(placement.root()), &[]).unwrap();
        let unresolved = scripting::bind_class_slots(&mut runtime, "lamp::Lamp", &scene.world, placement.root());
        assert_eq!(unresolved, [missing_slot.clone()]);

        let light_id = pulsar_world_registry::component_id_for_class("LightComponent").unwrap();
        assert_eq!(
            runtime.variable("lamp::Lamp", &pulsar_class::slot_variable_name(&second_slot)),
            Some(&Value::Component(pulsar_scenedb::ComponentRef::new(child, light_id)))
        );
        match runtime.variable("lamp::Lamp", &pulsar_class::slot_variable_name(&missing_slot)) {
            Some(Value::Component(handle)) => assert_eq!(handle.entity, pulsar_scenedb::Entity::DANGLING, "none, not the root"),
            other => panic!("unexpected {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// #922: the script section of the generated `engine_main::setup()`
/// (`engine_backend::services::core_project_builder`), compiled in-tree.
#[cfg(test)]
mod generated_setup_script_section {
    use crate::prelude::*;

    fn setup(game: &mut TickLoop) -> Result<(), String> {
        game.enable_project_scripting()?;
        Ok(())
    }

    #[test]
    fn generated_setup_compiles_and_enables_scripting() {
        // The launcher (or Play-in-Editor) installs the content first; the
        // generated setup names no path.
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join("Pulsar")).unwrap();
        pulsar_content::ContentRoot::project(project.path()).install();
        let mut game = TickLoop::new(TickMode::default(), 0);
        setup(&mut game).unwrap();
        assert_eq!(
            game.scripts.as_ref().unwrap().lock().unwrap().project_root(),
            project.path()
        );
        assert!(game.scripts.is_some());
        game.tick_once();
        let driver = game.scripts.as_ref().unwrap().lock().unwrap();
        assert!(driver.runtime().instance_ids().is_empty(), "no default instances");
    }
}
