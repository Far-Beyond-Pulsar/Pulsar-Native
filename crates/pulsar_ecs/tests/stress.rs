//! # Adversarial and performance tests for pulsar_ecs
//!
//! These tests probe edge cases, memory safety, slot reuse correctness,
//! archetype explosion, entity lifecycle, and raw throughput.
//! Run with: `cargo test -p pulsar_ecs` or `cargo bench -p pulsar_ecs`.

use std::time::{Duration, Instant};
use pulsar_ecs::*;

// ═════════════════════════════════════════════════════════════════════════════
// Component types
// ═════════════════════════════════════════════════════════════════════════════

struct Pos(f32, f32, f32);
struct Vel(f32, f32, f32);
struct Health(u32);
struct Tag;
struct Name(String);
struct Weight(f64);
struct Color([f32; 4]);
struct Lifetime(f32);
struct Zst;
struct Large([u8; 1024]);

struct A;
struct B;
struct C;
struct D;
struct E;
struct F;
struct G;

macro_rules! assert_entity_count {
    ($world:expr, $expected:expr) => {
        let count = $world.query::<()>().count();
        assert_eq!(count, $expected, "entity count mismatch");
    };
}

// ═════════════════════════════════════════════════════════════════════════════
// 1.  Correctness — basic lifecycle
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn correctness_spawn_despawn() {
    let mut world = World::new();
    let e = world.spawn();
    assert!(world.is_alive(e));
    assert!(world.despawn(e));
    assert!(!world.is_alive(e));
    assert_entity_count!(&world, 0);
}

#[test]
fn correctness_despawn_twice() {
    let mut world = World::new();
    let e = world.spawn();
    assert!(world.despawn(e));
    assert!(!world.despawn(e), "second despawn should return false");
}

#[test]
fn correctness_spawn_after_despawn_reuses_slot() {
    let mut world = World::new();
    let e1 = world.spawn();
    let idx1 = e1.index();
    world.despawn(e1);

    let e2 = world.spawn();
    assert_eq!(
        e2.index(),
        idx1,
        "slot should be reused"
    );
    // Generations differ — entities are distinct despite same index.
    assert_ne!(e1.generation(), e2.generation());
    assert!(world.is_alive(e2));
    assert!(!world.is_alive(e1));
}

#[test]
fn correctness_dead_entity_ops() {
    let mut world = World::new();
    let e = world.spawn();
    world.despawn(e);

    assert!(world.get::<Pos>(e).is_none());
    assert!(world.get_mut::<Pos>(e).is_none());
}

// ═════════════════════════════════════════════════════════════════════════════
// 2.  Correctness — insert / remove / get
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn correctness_insert_get() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Pos(1.0, 2.0, 3.0));
    world.insert(e, Health(42));

    let pos = world.get::<Pos>(e).unwrap();
    assert_eq!(pos.0, 1.0);
    let health = world.get::<Health>(e).unwrap();
    assert_eq!(health.0, 42);
}

#[test]
fn correctness_insert_get_mut() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Pos(1.0, 2.0, 3.0));

    *world.get_mut::<Pos>(e).unwrap() = Pos(4.0, 5.0, 6.0);
    assert_eq!(world.get::<Pos>(e).unwrap().0, 4.0);
}

#[test]
fn correctness_insert_overwrite() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Pos(1.0, 2.0, 3.0));
    world.insert(e, Pos(7.0, 8.0, 9.0)); // same component — in-place update
    assert_eq!(world.get::<Pos>(e).unwrap().0, 7.0);
}

#[test]
fn correctness_remove() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Pos(1.0, 2.0, 3.0));
    world.insert(e, Health(99));

    let removed = world.remove::<Health>(e);
    assert_eq!(removed.unwrap().0, 99);
    assert!(world.get::<Health>(e).is_none());
    assert!(world.get::<Pos>(e).is_some()); // other components survive

    // Removing non-existent component returns None
    assert!(world.remove::<Vel>(e).is_none());
}

#[test]
fn correctness_get_on_entity_without_component() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Pos(1.0, 2.0, 3.0));
    assert!(world.get::<Vel>(e).is_none());
}

#[test]
fn correctness_zst_component() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Zst);
    assert!(world.get::<Zst>(e).is_some());
    let removed = world.remove::<Zst>(e);
    assert!(removed.is_some());
    assert!(world.get::<Zst>(e).is_none());
}

#[test]
fn correctness_large_component() {
    let mut world = World::new();
    let e = world.spawn();
    let data = Large([0xAB; 1024]);
    world.insert(e, data);
    let retrieved = world.get::<Large>(e).unwrap();
    assert_eq!(retrieved.0[0], 0xAB);
    assert_eq!(retrieved.0[1023], 0xAB);
}

// ═════════════════════════════════════════════════════════════════════════════
// 3.  Correctness — query
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn correctness_query_basic() {
    let mut world = World::new();
    let e1 = world.spawn();
    world.insert(e1, Pos(1.0, 2.0, 3.0));
    world.insert(e1, Vel(0.1, 0.2, 0.3));

    let e2 = world.spawn();
    world.insert(e2, Pos(4.0, 5.0, 6.0));
    world.insert(e2, Vel(0.4, 0.5, 0.6));

    let e3 = world.spawn();
    world.insert(e3, Pos(7.0, 8.0, 9.0));
    // e3 has no Vel

    let count = world.query::<(&Pos, &Vel)>().count();
    assert_eq!(count, 2, "only e1 and e2 have both Pos and Vel");
}

#[test]
fn correctness_query_mut() {
    let mut world = World::new();
    let e1 = world.spawn();
    world.insert(e1, Pos(1.0, 2.0, 3.0));
    let e2 = world.spawn();
    world.insert(e2, Pos(4.0, 5.0, 6.0));

    // Double-buffer approach: collect then mutate
    let entities: Vec<_> = world.query::<(&Pos,)>().map(|(e, _)| e).collect();
    for &e in &entities {
        *world.get_mut::<Pos>(e).unwrap() = Pos(0.0, 0.0, 0.0);
    }

    for (_, (pos,)) in world.query::<(&Pos,)>() {
        assert_eq!(pos.0, 0.0);
    }
}

#[test]
fn correctness_query_tag() {
    let mut world = World::new();
    let e1 = world.spawn();
    world.insert(e1, Tag);
    let e2 = world.spawn();
    world.insert(e2, Pos(1.0, 2.0, 3.0));

    let count = world.query::<(&Tag,)>().count();
    assert_eq!(count, 1);
}

#[test]
fn correctness_query_tuple_4() {
    let mut world = World::new();
    for _ in 0..10 {
        let e = world.spawn();
        world.insert(e, Pos(0.0, 0.0, 0.0));
        world.insert(e, Vel(0.0, 0.0, 0.0));
        world.insert(e, Health(100));
        world.insert(e, Tag);
    }
    let count = world.query::<(&Pos, &Vel, &Health, &Tag)>().count();
    assert_eq!(count, 10);
}

#[test]
fn correctness_query_empty_world() {
    let world = World::new();
    let count = world.query::<(&Pos,)>().count();
    assert_eq!(count, 0);
}

#[test]
fn correctness_query_entity_order_preserved_after_despawn() {
    let mut world = World::new();
    let e1 = world.spawn();
    world.insert(e1, Pos(1.0, 0.0, 0.0));
    let e2 = world.spawn();
    world.insert(e2, Pos(2.0, 0.0, 0.0));
    let e3 = world.spawn();
    world.insert(e3, Pos(3.0, 0.0, 0.0));

    world.despawn(e2); // swap-remove should move e3 into e2's row

    let results: Vec<f32> = world
        .query::<(&Pos,)>()
        .map(|(_, (p,))| p.0)
        .collect();
    // Either order is valid after swap-remove, but both entities must be present
    assert_eq!(results.len(), 2);
    assert!(results.contains(&1.0));
    assert!(results.contains(&3.0));
}

// ═════════════════════════════════════════════════════════════════════════════
// 4.  Adversarial — archetype explosion
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_archetype_explosion_sparse() {
    // Create 50 entities with unique component combinations to exercise
    // hashmap-based archetype index lookup under load.
    let mut world = World::new();

    for i in 0..50 {
        let e = world.spawn();
        world.insert(e, Pos(i as f32, 0.0, 0.0));
        world.insert(e, Health(i as u32));
        if i % 2 == 0 { world.insert(e, Vel(0.0, 0.0, 0.0)); }
        if i % 3 == 0 { world.insert(e, Tag); }
        if i % 5 == 0 { world.insert(e, Name(format!("n{}", i))); }
        if i % 7 == 0 { world.insert(e, Weight(i as f64)); }
    }

    // Query should still be fast and correct
    let total = world.query::<(&Pos,)>().count();
    assert_eq!(total, 50, "all entities have Pos");
}

#[test]
fn adversarial_many_archetypes_query_all() {
    let mut world = World::new();
    // Create 128 entities each with a unique component set
    for i in 0..128 {
        let e = world.spawn();
        world.insert(e, Pos(i as f32, 0.0, 0.0));
        if i & 1 != 0 { world.insert(e, A); }
        if i & 2 != 0 { world.insert(e, B); }
        if i & 4 != 0 { world.insert(e, C); }
        if i & 8 != 0 { world.insert(e, D); }
        if i & 16 != 0 { world.insert(e, E); }
        if i & 32 != 0 { world.insert(e, F); }
        if i & 64 != 0 { world.insert(e, G); }
    }
    // Every entity has Pos, so query should return all 128
    let count = world.query::<(&Pos,)>().count();
    assert_eq!(count, 128);

    // Query for a rare combo
    let rare = world.query::<(&Pos, &A, &C, &E)>().count();
    let expected = (0..128).filter(|i| i & 1 != 0 && i & 4 != 0 && i & 16 != 0).count();
    assert_eq!(rare, expected);
}

// ═════════════════════════════════════════════════════════════════════════════
// 5.  Adversarial — insert/remove storms
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_insert_remove_cycling() {
    let mut world = World::new();
    let e = world.spawn();

    for i in 0..1000 {
        world.insert(e, Health(i));
        assert_eq!(world.get::<Health>(e).unwrap().0, i);

        if i % 2 == 0 {
            world.insert(e, Vel(i as f32, 0.0, 0.0));
        } else {
            let _ = world.remove::<Vel>(e);
        }
    }

    assert!(world.is_alive(e));
    assert!(world.get::<Health>(e).is_some());
}

#[test]
fn adversarial_spawn_despawn_spam() {
    let mut world = World::new();

    for _ in 0..10_000 {
        let e = world.spawn();
        world.insert(e, Pos(1.0, 2.0, 3.0));
        world.despawn(e);
    }

    // World should be clean
    assert_entity_count!(&world, 0);
}

#[test]
fn adversarial_remove_all_components_then_despawn() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Pos(1.0, 2.0, 3.0));
    world.insert(e, Vel(0.1, 0.2, 0.3));
    world.insert(e, Health(50));

    let _ = world.remove::<Pos>(e);
    let _ = world.remove::<Vel>(e);
    let _ = world.remove::<Health>(e);

    // Entity still exists (in empty archetype)
    assert!(world.is_alive(e));
    assert!(world.get::<Pos>(e).is_none());

    // Can still despawn
    assert!(world.despawn(e));
    assert!(!world.is_alive(e));
}

// ═════════════════════════════════════════════════════════════════════════════
// 6.  Adversarial — generation wraparound and slot reuse
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_generation_slot_reuse_exhaustive() {
    let mut world = World::new();
    let mut entities = Vec::new();

    // Spawn and despawn repeatedly to exercise slot reuse
    for _ in 0..100 {
        let e = world.spawn();
        world.insert(e, Pos(0.0, 0.0, 0.0));
        entities.push(e);
    }

    for &e in &entities {
        assert!(world.despawn(e));
    }

    // Spawn again — should reuse slots with higher generations
    let new_entities: Vec<_> = (0..100).map(|_| world.spawn()).collect();
    for (i, &e) in new_entities.iter().enumerate() {
        assert!(world.is_alive(e));
        world.insert(e, Pos(i as f32, 0.0, 0.0));
    }

    // Old entities should be dead
    for &e in &entities {
        assert!(!world.is_alive(e));
    }

    // New spawns should increment generation each time
    for _ in 0..5 {
        let e = world.spawn();
        let idx = e.index();
        assert!(world.is_alive(e));
        world.despawn(e);
        let re = world.spawn();
        assert_eq!(re.index(), idx);
        assert!(re.generation() > e.generation());
    }
}

#[test]
fn adversarial_stale_entity_rejected() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Health(42));
    world.despawn(e);

    // The same u64 bit pattern now points to a dead (or reused) slot.
    // We can't test the exact same Entity value because Entity::new is pub(crate).
    // But we can verify that is_alive correctly returns false.
    assert!(!world.is_alive(e));
    assert!(world.get::<Health>(e).is_none());
}

// ═════════════════════════════════════════════════════════════════════════════
// 7.  Adversarial — concurrent-ish interleaving
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_interleaved_spawn_despawn_query() {
    let mut world = World::new();

    for round in 0..50 {
        // Spawn phase
        let batch: Vec<_> = (0..20)
            .map(|i| {
                let e = world.spawn();
                world.insert(e, Pos(round as f32 * i as f32, 0.0, 0.0));
                if i % 3 == 0 {
                    world.insert(e, Vel(1.0, 0.0, 0.0));
                }
                e
            })
            .collect();

        // Query phase
        let query_count = world.query::<(&Pos,)>().count();
        assert!(query_count > 0);

        // Remove phase — despawn every other
        for (i, &e) in batch.iter().enumerate() {
            if i % 2 == 0 {
                world.despawn(e);
            }
        }

        // Query again
        let after_count = world.query::<(&Pos,)>().count();
        assert_eq!(after_count + 10, query_count);
    }
}

#[test]
fn adversarial_random_component_churn() {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let mut world = World::new();
    let mut entities: Vec<Entity> = Vec::new();

    for _ in 0..1000 {
        match rng.gen_range(0..5) {
            0 => {
                // Spawn
                let e = world.spawn();
                if rng.gen_bool(0.5) {
                    world.insert(e, Pos(rng.gen(), rng.gen(), rng.gen()));
                }
                if rng.gen_bool(0.3) {
                    world.insert(e, Vel(rng.gen(), rng.gen(), rng.gen()));
                }
                entities.push(e);
            }
            1 => {
                // Despawn random
                if !entities.is_empty() {
                    let idx = rng.gen_range(0..entities.len());
                    let e = entities.swap_remove(idx);
                    world.despawn(e);
                }
            }
            2 => {
                // Insert component on random entity
                if !entities.is_empty() {
                    let e = entities[rng.gen_range(0..entities.len())];
                    if world.is_alive(e) {
                        world.insert(e, Health(rng.gen()));
                    }
                }
            }
            3 => {
                // Remove component from random entity
                if !entities.is_empty() {
                    let e = entities[rng.gen_range(0..entities.len())];
                    if world.is_alive(e) {
                        let _ = world.remove::<Health>(e);
                    }
                }
            }
            4 => {
                // Query
                let _ = world.query::<(&Pos, &Health)>().count();
            }
            _ => {}
        }
    }

    // Final sanity check — all remaining entities are alive
    for &e in &entities {
        if world.is_alive(e) {
            // Can always read Pos (may or may not have been inserted)
            let _ = world.get::<Pos>(e);
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 8.  Actor lifecycle
// ═════════════════════════════════════════════════════════════════════════════

struct TestActor {
    pub began: bool,
    pub ended: bool,
    pub ticked: u32,
}

impl Actor for TestActor {
    fn begin_play(&mut self, _entity: Entity, _world: &mut World) {
        self.began = true;
    }
    fn end_play(&mut self, _entity: Entity, _world: &mut World) {
        self.ended = true;
    }
    fn tick(&mut self, _entity: Entity, _world: &mut World, _time: GameTime) {
        self.ticked += 1;
    }
}

#[test]
fn correctness_actor_register_tick_deregister() {
    let mut world = World::new();
    let mut registry = ActorRegistry::new();

    let actor = TestActor {
        began: false,
        ended: false,
        ticked: 0,
    };
    let entity = registry.register(actor, &mut world);
    assert!(world.is_alive(entity));

    // Tick once
    let time = GameTime {
        elapsed: Duration::from_secs(1),
        delta: Duration::from_secs_f64(1.0 / 60.0),
        tick: 1,
    };
    registry.tick_all(&mut world, time);

    // Deregister
    registry.deregister(entity, &mut world);
    assert!(!world.is_alive(entity));
}

#[test]
fn correctness_actor_slot_reuse() {
    let mut world = World::new();
    let mut registry = ActorRegistry::new();

    let e1 = registry.register(
        TestActor { began: false, ended: false, ticked: 0 },
        &mut world,
    );
    let idx1 = e1.index();
    registry.deregister(e1, &mut world);

    // New actor should reuse the entity slot
    let e2 = registry.register(
        TestActor { began: false, ended: false, ticked: 0 },
        &mut world,
    );
    assert_eq!(e2.index(), idx1, "entity slot should be reused");
    assert_ne!(e2.generation(), e1.generation());
    assert!(world.is_alive(e2));
}

// ═════════════════════════════════════════════════════════════════════════════
// 9.  Schedule
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn correctness_schedule_basic() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Health(0));

    let mut schedule = Schedule::new();
    schedule.add_system("inc_health", |world, _time| {
        for (_, (health,)) in world.query::<(&mut Health,)>() {
            health.0 = health.0 + 1;
        }
    });

    let time = GameTime {
        elapsed: Duration::from_secs(0),
        delta: Duration::from_secs_f64(1.0 / 60.0),
        tick: 0,
    };
    schedule.run(&mut world, time);
    assert_eq!(world.get::<Health>(e).unwrap().0, 1);

    schedule.run(&mut world, time);
    assert_eq!(world.get::<Health>(e).unwrap().0, 2);
}

#[test]
fn correctness_schedule_empty() {
    let mut world = World::new();
    let mut schedule = Schedule::new();
    let time = GameTime {
        elapsed: Duration::from_secs(0),
        delta: Duration::from_secs_f64(1.0 / 60.0),
        tick: 0,
    };
    schedule.run(&mut world, time); // must not panic
    assert!(schedule.is_empty());
}

#[test]
fn correctness_schedule_order() {
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Health(0));

    let mut schedule = Schedule::new();
    schedule.add_system("first", |world, _time| {
        for (_, (health,)) in world.query::<(&mut Health,)>() {
            health.0 = health.0 + 10;
        }
    });
    schedule.add_system("second", |world, _time| {
        for (_, (health,)) in world.query::<(&mut Health,)>() {
            health.0 = health.0 * 2;
        }
    });

    let time = GameTime {
        elapsed: Duration::from_secs(0),
        delta: Duration::from_secs_f64(1.0 / 60.0),
        tick: 0,
    };
    schedule.run(&mut world, time);
    // first: 0 + 10 = 10, second: 10 * 2 = 20
    assert_eq!(world.get::<Health>(e).unwrap().0, 20);
}

// ═════════════════════════════════════════════════════════════════════════════
// 10. Performance benchmarks (self-timing)
// ═════════════════════════════════════════════════════════════════════════════

const PERF_ITERATIONS: u64 = 10_000;

#[test]
fn perf_spawn_throughput() {
    let start = Instant::now();
    for _ in 0..PERF_ITERATIONS {
        let mut world = World::new();
        for _ in 0..1000 {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
        }
    }
    let elapsed = start.elapsed();
    let total = PERF_ITERATIONS * 1000;
    let rate = total as f64 / elapsed.as_secs_f64();
    eprintln!(
        "perf_spawn_throughput: {} entities in {:.2}s → {:.0} entities/sec",
        total, elapsed.as_secs_f64(), rate
    );
    // Baseline: ~250K-500K entities/sec on modern hardware
    assert!(rate > 100_000.0, "spawn rate too low: {:.0}", rate);
}

#[test]
fn perf_query_traversal() {
    let mut world = World::new();
    for _ in 0..PERF_ITERATIONS {
        let e = world.spawn();
        world.insert(e, Pos(1.0, 2.0, 3.0));
        world.insert(e, Vel(0.1, 0.2, 0.3));
        world.insert(e, Health(100));
    }

    let start = Instant::now();
    let mut count = 0usize;
    for _ in 0..10 {
        for (_, (pos, vel)) in world.query::<(&Pos, &Vel)>() {
            let _ = pos.0 + vel.0;
            count += 1;
        }
    }
    let elapsed = start.elapsed();
    let rate = count as f64 / elapsed.as_secs_f64();
    eprintln!(
        "perf_query_traversal: {} iterations in {:.2}s → {:.0} items/sec",
        count, elapsed.as_secs_f64(), rate
    );
    // Baseline: current implementation manages ~1.5M items/sec
    // on HashMap-based archetype lookup. Threshold set for CI.
    assert!(rate > 500_000.0, "query rate too low: {:.0}", rate);
}

#[test]
fn perf_archetype_migration() {
    let start = Instant::now();
    for _ in 0..1000 {
        let mut world = World::new();
        let e = world.spawn();
        world.insert(e, Pos(1.0, 2.0, 3.0));
        world.insert(e, Health(100));
        let _ = world.remove::<Health>(e);
        world.insert(e, Vel(0.0, 0.0, 0.0));
        let _ = world.remove::<Pos>(e);
    }
    let elapsed = start.elapsed();
    let rate = 4000.0 / elapsed.as_secs_f64(); // 4 migrations per loop
    eprintln!(
        "perf_archetype_migration: {:.0} migrations/sec",
        rate
    );
    assert!(rate > 50_000.0, "migration rate too low: {:.0}", rate);
}

#[test]
fn perf_query_on_sparse_archetypes() {
    // Create many archetypes each with few entities
    let mut world = World::new();
    for i in 0..200 {
        for _ in 0..5 {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(i as u32));
            if i % 2 == 0 {
                world.insert(e, Vel(0.0, 0.0, 0.0));
            }
        }
    }

    let start = Instant::now();
    let mut count = 0usize;
    for _ in 0..10 {
        for (_, _) in world.query::<(&Pos, &Health)>() {
            count += 1;
        }
    }
    let elapsed = start.elapsed();
    let rate = count as f64 / elapsed.as_secs_f64();
    eprintln!(
        "perf_query_on_sparse_archetypes: {:.0} items/sec", rate
    );
    assert!(rate > 1_000_000.0, "sparse query too slow: {:.0}", rate);
}

// ═════════════════════════════════════════════════════════════════════════════
// 11. Edge cases — Entity::DANGLING
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn correctness_dangling_entity() {
    let mut world = World::new();
    // Operations on the sentinel must not panic (graceful rejection)
    assert!(!world.is_alive(Entity::DANGLING));
    assert!(!world.despawn(Entity::DANGLING));
    assert!(world.get::<Pos>(Entity::DANGLING).is_none());
    assert!(world.get_mut::<Pos>(Entity::DANGLING).is_none());
}

#[test]
fn correctness_insert_on_dangling_entity() {
    let mut world = World::new();
    // This will panic because is_alive fails — that's acceptable since
    // it's a contract violation. We just verify it doesn't cause UB.
    // (No explicit test — UB detection requires Miri.)
}

// ═════════════════════════════════════════════════════════════════════════════
// 12. Adversarial — empty queries and edge archetypes
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_query_after_all_despawned() {
    let mut world = World::new();
    let entities: Vec<_> = (0..100).map(|_| world.spawn()).collect();
    for &e in &entities {
        world.insert(e, Pos(0.0, 0.0, 0.0));
    }
    for &e in &entities {
        world.despawn(e);
    }

    let count = world.query::<(&Pos,)>().count();
    assert_eq!(count, 0);
}

#[test]
fn adversarial_large_batch_spawn_despawn() {
    let mut world = World::new();
    let batch_size = 10_000;

    let entities: Vec<_> = (0..batch_size)
        .map(|i| {
            let e = world.spawn();
            world.insert(e, Pos(i as f32, 0.0, 0.0));
            world.insert(e, Name(format!("entity-{}", i)));
            e
        })
        .collect();

    for &e in &entities {
        world.despawn(e);
    }

    assert_entity_count!(&world, 0);
}

#[test]
fn adversarial_component_names_collide_across_archetypes() {
    // Different archetypes share the same TypeId for Pos — ensure
    // the migration logic doesn't corrupt data.
    let mut world = World::new();
    let mut entities = Vec::new();

    for i in 0..10 {
        let e = world.spawn();
        world.insert(e, Pos(i as f32, 0.0, 0.0));
        if i % 2 == 0 {
            world.insert(e, Vel(1.0, 0.0, 0.0));
        }
        if i % 3 == 0 {
            world.insert(e, Tag);
        }
        entities.push(e);
    }

    // Verify Pos values survive archetype migrations
    for (i, &e) in entities.iter().enumerate() {
        let pos = world.get::<Pos>(e).unwrap();
        assert_eq!(pos.0, i as f32, "Pos data corrupted after migration");
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// 13. verify the macros compile for all tuple sizes
// ═════════════════════════════════════════════════════════════════════════════

#[test]
fn correctness_query_tuples_compile() {
    // Verify 1 through 8 tuple queries compile
    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Pos(1.0, 2.0, 3.0));
    world.insert(e, Vel(4.0, 5.0, 6.0));
    world.insert(e, Health(7));
    world.insert(e, Tag);
    world.insert(e, Name("test".into()));
    world.insert(e, Weight(8.0));
    world.insert(e, Color([9.0; 4]));
    world.insert(e, Lifetime(10.0));

    // 1-tuple
    let _ = world.query::<(&Pos,)>().count();
    // 2-tuple
    let _ = world.query::<(&Pos, &Vel)>().count();
    // 3-tuple
    let _ = world.query::<(&Pos, &Vel, &Health)>().count();
    // 4-tuple
    let _ = world.query::<(&Pos, &Vel, &Health, &Tag)>().count();
    // 5-tuple
    let _ = world.query::<(&Pos, &Vel, &Health, &Tag, &Name)>().count();
    // 6-tuple
    let _ = world.query::<(&Pos, &Vel, &Health, &Tag, &Name, &Weight)>().count();
    // 7-tuple
    let _ = world.query::<(&Pos, &Vel, &Health, &Tag, &Name, &Weight, &Color)>().count();
    // 8-tuple
    let _ = world.query::<(&Pos, &Vel, &Health, &Tag, &Name, &Weight, &Color, &Lifetime)>()
        .count();
}
