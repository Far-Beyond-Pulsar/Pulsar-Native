#[cfg(test)]
mod ecs {
    use crate::prelude::*;

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
            tick_loop.actors.register(Counter(log.clone()), store.world_mut())
        };
        tick_loop.tick_once();
        {
            let mut store = tick_loop.scene_store.write();
            tick_loop.actors.deregister(entity, store.world_mut());
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
            let e = store.spawn(None, "Runtime", None).unwrap();
            tick_loop.actors.register(Counter(Arc::new(Mutex::new(Vec::new()))), store.world_mut());
            e
        };

        tick_loop.tick_once();

        // Another handle-holder (what the renderer is) sees the spawned
        // object and its components.
        let store = tick_loop.scene_store.read();
        assert_eq!(store.name(entity), Some("Runtime"));
    }
}

#[cfg(test)]
mod schedule_tests {
    use crate::prelude::*;
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

#[cfg(test)]
mod blueprint_instances {
    //! #648 — dispatcher instances bound to real scene entities: per-entity
    //! lifecycle dispatch, independent instances of one class, runtime
    //! bind/unbind, and hot-reload state preservation.
    //!
    //! The probe component registers exactly like engine classes do
    //! (`#[engine_class]` expands to the same inventory submissions), and
    //! the hand-built bytecode mirrors what `pbgc`'s comp-op codegen emits
    //! for `comp_set_prop` / `comp_call` nodes.

    use crate::blueprint_runtime::{BlueprintDispatcher, CompiledBytecode, ExecutorError, VariableDescriptor};
    use crate::prelude::*;
    use pbgc::bytecode::comp_ops::{
        encode_call_name_blob, encode_json_blob, encode_name_blob, JSON_BLOB_CAPACITY,
    };
    use pbgc::{BpProgram, Instruction};
    use pulsar_reflection::{
        ComponentMethodRegistration, EngineClass, EngineClassRegistration, MethodMetadata,
        MethodParameter, MethodReturnType, MethodType, PropertyMetadata, RuntimeTypeInfo,
        RUNTIME_TYPE_REGISTRY,
    };
    use pulsar_scenedb::{component_id, Entity, World};
    use pulsar_world_registry::WorldComponentRegistration;
    use serde::{Deserialize, Serialize};
    use serde_json::Value as JsonValue;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // ── Probe component ──────────────────────────────────────────────────

    #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
    struct TickProbe {
        charges: i32,
        played: bool,
    }

    impl EngineClass for TickProbe {
        fn class_name() -> &'static str {
            "TickProbe"
        }

        fn get_properties(&self) -> Vec<PropertyMetadata> {
            let i32_info: &'static RuntimeTypeInfo =
                RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 registered");
            let bool_info: &'static RuntimeTypeInfo =
                RUNTIME_TYPE_REGISTRY.get::<bool>().expect("bool registered");
            vec![
                PropertyMetadata {
                    name: "charges",
                    display_name: "Charges".into(),
                    category: None,
                    category_color: None,
                    category_default_collapsed: false,
                    category_order: None,
                    type_info: i32_info,
                    getter: Box::new(|c: &dyn EngineClass| {
                        Box::new(c.as_any().downcast_ref::<TickProbe>().unwrap().charges)
                    }),
                    setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                        if let Some(v) = v.downcast_ref::<i32>() {
                            c.as_any_mut().downcast_mut::<TickProbe>().unwrap().charges = *v;
                        }
                    }),
                },
                PropertyMetadata {
                    name: "played",
                    display_name: "Played".into(),
                    category: None,
                    category_color: None,
                    category_default_collapsed: false,
                    category_order: None,
                    type_info: bool_info,
                    getter: Box::new(|c: &dyn EngineClass| {
                        Box::new(c.as_any().downcast_ref::<TickProbe>().unwrap().played)
                    }),
                    setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                        if let Some(v) = v.downcast_ref::<bool>() {
                            c.as_any_mut().downcast_mut::<TickProbe>().unwrap().played = *v;
                        }
                    }),
                },
            ]
        }

        fn get_methods() -> Vec<MethodMetadata> {
            let i32_info: &'static RuntimeTypeInfo =
                RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 registered");
            vec![MethodMetadata {
                name: "add_charges",
                display_name: "Add Charges".into(),
                category: None,
                params: vec![MethodParameter { name: "amount", type_info: i32_info }],
                return_type: Some(MethodReturnType { type_info: i32_info }),
                method_type: MethodType::Fn,
                caller: Box::new(|c: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                    let amount = args.first().and_then(|a| a.downcast_ref::<i32>()).copied()?;
                    let probe = c.as_any_mut().downcast_mut::<TickProbe>()?;
                    probe.charges += amount;
                    Some(Box::new(probe.charges))
                }),
            }]
        }

        fn create_default() -> Box<dyn EngineClass> {
            Box::new(Self::default())
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }

        fn clone_boxed(&self) -> Box<dyn EngineClass> {
            Box::new(self.clone())
        }

        fn to_json(&self) -> Result<JsonValue, String> {
            serde_json::to_value(self).map_err(|e| e.to_string())
        }
    }

    fn tick_probe_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
        world.get::<TickProbe>(entity).map(|c| c as &dyn EngineClass)
    }

    fn tick_probe_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
        world.get_mut::<TickProbe>(entity).map(|c| c.into_inner() as &mut dyn EngineClass)
    }

    fn tick_probe_hydrate(world: &mut World, entity: Entity, data: &JsonValue) -> Result<(), String> {
        let parsed: TickProbe = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
        world.insert(entity, parsed);
        Ok(())
    }

    fn tick_probe_remove(world: &mut World, entity: Entity) {
        let _ = world.remove::<TickProbe>(entity);
    }

    pulsar_world_registry::inventory::submit! {
        WorldComponentRegistration {
            class_name: "TickProbe",
            component_type: component_id::<TickProbe>,
            hydrate: tick_probe_hydrate,
            remove: tick_probe_remove,
            dispatch: |world, entity, _: _, _: usize, _: _| world.get::<TickProbe>(entity).is_some(),
            get_as_engine_class: tick_probe_get,
            get_as_engine_class_mut: tick_probe_get_mut,
            on_removed: |_, _| {},
            refresh_gpu_mirror: |_, _| {},
        }
    }

    pulsar_reflection::inventory::submit! {
        EngineClassRegistration {
            name: "TickProbe",
            category: None,
            constructor: <TickProbe as EngineClass>::create_default,
            from_json: None,
        }
    }

    pulsar_reflection::inventory::submit! {
        ComponentMethodRegistration {
            class_name: "TickProbe",
            methods: <TickProbe as EngineClass>::get_methods,
        }
    }

    // ── Bytecode builder ─────────────────────────────────────────────────

    /// Fixed, 8-aligned staging offsets so runtime blob writes (u64 length
    /// prefixes at the output slot) stay well-defined.
    const NAME_OFF: usize = 64;
    const ARG_OFF: usize = 256;
    const OUT_OFF: usize = 2048;

    /// A one-class bytecode file shaped like real editor output:
    /// a variable layout plus `begin_play`
    /// (`comp_set_prop TickProbe::played = true`) and `tick`
    /// (`comp_call TickProbe::add_charges(1)`) event programs.
    fn build_tick_probe_bytecode() -> CompiledBytecode {
        let mut bytecode = CompiledBytecode::new("TickProbe");
        bytecode.add_variable(VariableDescriptor::f32("speed", 0, 1.0));
        bytecode.calculate_arena_size();

        let mut begin_play = BpProgram::new("begin_play");
        begin_play.instructions = vec![
            Instruction::InitBytes { offset: NAME_OFF, bytes: encode_name_blob("TickProbe", "played") },
            Instruction::InitBytes { offset: ARG_OFF, bytes: encode_json_blob("true") },
            Instruction::Call {
                fn_ptr: 0,
                node_type: "comp_set_prop::TickProbe::played".into(),
                input_offsets: vec![NAME_OFF, ARG_OFF],
                output_offset: 0,
                has_output: false,
                type_slot_offsets: vec![],
            },
            Instruction::Return,
        ];
        begin_play.max_args_count = 2;
        begin_play.arena_size = OUT_OFF;

        let mut tick = BpProgram::new("tick");
        tick.instructions = vec![
            Instruction::InitBytes {
                offset: NAME_OFF,
                bytes: encode_call_name_blob("TickProbe", "add_charges", 1),
            },
            Instruction::InitBytes { offset: ARG_OFF, bytes: encode_json_blob("1") },
            Instruction::Call {
                fn_ptr: 0,
                node_type: "comp_call::TickProbe::add_charges".into(),
                input_offsets: vec![NAME_OFF, ARG_OFF],
                output_offset: OUT_OFF,
                has_output: true,
                type_slot_offsets: vec![],
            },
            Instruction::Return,
        ];
        tick.max_args_count = 2;
        tick.arena_size = OUT_OFF + JSON_BLOB_CAPACITY + 8;

        bytecode.add_event_program("begin_play", begin_play);
        bytecode.add_event_program("tick", tick);
        bytecode
    }

    // ── Acceptance: two entities sharing one class (#648) ────────────────

    /// THE #648 acceptance criterion, through the full TickLoop phase-3
    /// path: two entities share one Blueprint class, each instance keeps
    /// its own arena and addresses its own entity's components.
    #[test]
    fn two_entities_sharing_one_class_tick_independently() {
        let mut game = TickLoop::new(TickMode::default(), 0);
        let mut dispatcher =
            BlueprintDispatcher::new().expect("blueprint executor loads");

        let (ea, eb) = {
            let mut store = game.scene_store.write();
            let ea = store.spawn(None, "ProbeA", None).expect("spawn A");
            let eb = store.spawn(None, "ProbeB", None).expect("spawn B");
            store.world_mut().insert(ea, TickProbe { charges: 5, played: false });
            store.world_mut().insert(eb, TickProbe { charges: 50, played: false });
            (ea, eb)
        };

        // One class, TWO instances with distinct bindings and arenas; A
        // also carries a variable override so arena isolation is visible.
        let mut overrides = HashMap::new();
        overrides.insert("speed".to_string(), serde_json::json!(7.5));
        dispatcher
            .register_bytecode("probe_a".into(), build_tick_probe_bytecode(), Some(ea), Some(overrides))
            .expect("register A");
        dispatcher
            .register_bytecode("probe_b".into(), build_tick_probe_bytecode(), Some(eb), None)
            .expect("register B");

        assert_eq!(dispatcher.instance_entity("probe_a"), Some(ea));
        assert_eq!(dispatcher.instance_entity("probe_b"), Some(eb));

        game.blueprint_dispatcher = Some(Arc::new(Mutex::new(dispatcher)));
        game.tick_once();
        game.tick_once();

        let store = game.scene_store.read();
        let pa = store.world().get::<TickProbe>(ea).expect("A's probe alive");
        let pb = store.world().get::<TickProbe>(eb).expect("B's probe alive");
        assert_eq!(pa.charges, 7, "instance A mutated only its own entity (5 + 2 ticks)");
        assert_eq!(pb.charges, 52, "instance B mutated only its own entity (50 + 2 ticks)");
        assert!(pa.played && pb.played, "begin_play dispatched per bound entity");

        drop(store);
        let dispatcher = game.blueprint_dispatcher.as_ref().unwrap().lock().unwrap();
        assert_eq!(
            dispatcher.instance_variable_bytes("probe_a", "speed"),
            Some(7.5_f32.to_le_bytes().to_vec()),
            "override survives in A's arena"
        );
        assert_eq!(
            dispatcher.instance_variable_bytes("probe_b", "speed"),
            Some(1.0_f32.to_le_bytes().to_vec()),
            "B's arena holds defaults, untouched by A"
        );
    }

    /// Deleting one entity must not disturb its sibling instance — the
    /// dead instance's component ops refuse via liveness while the other
    /// keeps ticking.
    #[test]
    fn despawning_one_entity_does_not_disturb_its_sibling_instance() {
        let mut world = World::new();
        let ea = world.spawn();
        world.insert(ea, TickProbe { charges: 0, played: false });
        let eb = world.spawn();
        world.insert(eb, TickProbe { charges: 100, played: false });

        let mut dispatcher =
            BlueprintDispatcher::new().expect("blueprint executor loads");
        dispatcher
            .register_bytecode("a".into(), build_tick_probe_bytecode(), Some(ea), None)
            .unwrap();
        dispatcher
            .register_bytecode("b".into(), build_tick_probe_bytecode(), Some(eb), None)
            .unwrap();

        dispatcher.dispatch_pending_begin_play(&mut world);
        dispatcher.dispatch_tick_all(&mut world, 0.016);
        assert!(world.get::<TickProbe>(eb).is_some());

        world.despawn(eb);
        dispatcher.dispatch_tick_all(&mut world, 0.016);

        assert_eq!(
            world.get::<TickProbe>(ea).map(|p| p.charges),
            Some(2),
            "survivor kept ticking after sibling despawn"
        );
        assert!(world.get::<TickProbe>(eb).is_none());
    }

    // ── Runtime bind/unbind ──────────────────────────────────────────────

    /// Instances registered before their entities exist (level wiring lands
    /// later, #650) run graphs with component ops refusing; late binding
    /// turns ops on without respawning anything.
    #[test]
    fn unbound_instance_refuses_component_ops_until_bound() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TickProbe { charges: 9, played: false });

        let mut dispatcher =
            BlueprintDispatcher::new().expect("blueprint executor loads");
        dispatcher
            .register_bytecode("solo".into(), build_tick_probe_bytecode(), None, None)
            .unwrap();
        assert_eq!(dispatcher.instance_entity("solo"), None, "starts unbound");

        dispatcher.dispatch_pending_begin_play(&mut world);
        dispatcher.dispatch_tick_all(&mut world, 0.016);
        let probe = world.get::<TickProbe>(e).unwrap();
        assert_eq!(
            (probe.charges, probe.played),
            (9, false),
            "unbound ops refuse without panicking or misaddressing"
        );

        assert!(matches!(
            dispatcher.bind_instance("ghost", e),
            Err(ExecutorError::InstanceNotRegistered(_))
        ));
        dispatcher.bind_instance("solo", e).expect("late bind");
        dispatcher.dispatch_tick_all(&mut world, 0.016);
        assert_eq!(world.get::<TickProbe>(e).unwrap().charges, 10);

        assert_eq!(dispatcher.unbind_instance("solo"), Some(e));
        assert_eq!(dispatcher.unbind_instance("solo"), None, "already unbound");
    }

    // ── Hot reload ───────────────────────────────────────────────────────

    /// PIE recompile path: swapping a class' programs keeps entities and
    /// bindings intact, preserves matching variable state, and the new
    /// programs execute on the next tick.
    #[test]
    fn hot_reload_swaps_programs_without_respawning_instances() {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, TickProbe { charges: 1, played: false });

        let mut dispatcher =
            BlueprintDispatcher::new().expect("blueprint executor loads");
        let mut overrides = HashMap::new();
        overrides.insert("speed".to_string(), serde_json::json!(4.5));
        dispatcher
            .register_bytecode("hot".into(), build_tick_probe_bytecode(), Some(e), Some(overrides))
            .unwrap();

        dispatcher.reload_blueprint(build_tick_probe_bytecode()).expect("same-layout reload");
        assert_eq!(
            dispatcher.instance_variable_bytes("hot", "speed"),
            Some(4.5_f32.to_le_bytes().to_vec()),
            "matching-layout variables survive the swap"
        );
        assert_eq!(dispatcher.instance_entity("hot"), Some(e), "binding survives");

        // Reloaded programs still run against the bound entity.
        dispatcher.dispatch_tick_all(&mut world, 0.016);
        assert_eq!(world.get::<TickProbe>(e).unwrap().charges, 2);
    }
}
