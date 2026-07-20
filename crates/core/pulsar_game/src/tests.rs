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
        // NOTE: `Actor::tick` (from `pulsar_scenedb`) requires
        // `pulsar_scenedb::GameTime`, not the `GameTime` re-exported by this
        // crate's prelude (which is `pulsar_core::GameTime`). The two are
        // structurally identical but nominally distinct types — a leftover
        // of the SceneDB extraction. Using the fully-qualified path here
        // rather than the prelude import.
        fn tick(&mut self, _e: Entity, _w: &mut World, _t: pulsar_scenedb::GameTime) {
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
        let entity = tick_loop
            .actors
            .register(Counter(log.clone()), &mut tick_loop.world);
        tick_loop.tick_once();
        tick_loop.actors.deregister(entity, &mut tick_loop.world);
        let events = log.lock().unwrap().clone();
        assert_eq!(events, vec!["begin", "tick", "end"]);
    }
}

#[cfg(test)]
mod schedule_tests {
    use crate::prelude::*;
    use std::sync::{Arc, Mutex};

    #[derive(Debug)]
    struct Count(u32);

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
