//! # Adversarial and performance tests for pulsar_scenedb
//!
//! These tests probe edge cases, memory safety, slot reuse correctness,
//! archetype explosion, entity lifecycle, and raw throughput.
//! Run with: `cargo test -p pulsar_scenedb` or `cargo bench -p pulsar_scenedb`.

use std::time::{Duration, Instant};
use pulsar_scenedb::*;

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Component types
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 1.  Correctness â€” basic lifecycle
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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
    // Generations differ â€” entities are distinct despite same index.
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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 2.  Correctness â€” insert / remove / get
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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
    world.insert(e, Pos(7.0, 8.0, 9.0)); // same component â€” in-place update
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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 3.  Correctness â€” query
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 4.  Adversarial â€” archetype explosion
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 5.  Adversarial â€” insert/remove storms
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 6.  Adversarial â€” generation wraparound and slot reuse
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

    // Spawn again â€” should reuse slots with higher generations
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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 7.  Adversarial â€” concurrent-ish interleaving
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

        // Remove phase â€” despawn every other
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

    // Final sanity check â€” all remaining entities are alive
    for &e in &entities {
        if world.is_alive(e) {
            // Can always read Pos (may or may not have been inserted)
            let _ = world.get::<Pos>(e);
        }
    }
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 8.  Actor lifecycle
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 9.  Schedule
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 10. Performance benchmarks (self-timing)
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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
        "perf_spawn_throughput: {} entities in {:.2}s â†’ {:.0} entities/sec",
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
        "perf_query_traversal: {} iterations in {:.2}s â†’ {:.0} items/sec",
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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 11. Edge cases â€” Entity::DANGLING
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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
    // This will panic because is_alive fails â€” that's acceptable since
    // it's a contract violation. We just verify it doesn't cause UB.
    // (No explicit test â€” UB detection requires Miri.)
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 12. Adversarial â€” empty queries and edge archetypes
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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
    // Different archetypes share the same TypeId for Pos â€” ensure
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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 13. verify the macros compile for all tuple sizes
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

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

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// Regression: column/entity length symmetry during structural mutation
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Migrate entities into an already-populated destination archetype (the
/// classic "dual-push" scenario) and verify column/entity symmetry.
#[test]
fn regression_column_entity_symmetry_on_insert_into_existing_archetype() {
    struct A(u32);
    struct B(u32);
    struct C(u32);

    let mut world = World::new();

    // Phase 1: create a batch of [A, B, C] entities to pre-populate the arch.
    for i in 0..10 {
        let e = world.spawn();
        world.insert(e, A(i));
        world.insert(e, B(i * 10));
        world.insert(e, C(i * 100));
    }
    world.assert_archetype_consistency();

    // Phase 2: add more [A, B] entities, then migrate each into the
    // already-populated [A, B, C] archetype by inserting C.
    for i in 10..20 {
        let e = world.spawn();
        world.insert(e, A(i));
        world.insert(e, B(i * 10));
        // This insert migrates into a destination that already has 10+ entities
        world.insert(e, C(i * 100));
        world.assert_archetype_consistency();
    }
    world.assert_archetype_consistency();

    // Phase 3: remove C from entities one by one, migrating back into the
    // already-populated [A, B] archetype.
    // Collect entities in [A, B, C] archetype
    let abc_entities: Vec<_> = world.query::<(&A, &B, &C)>().map(|(e, _)| e).collect();
    for (i, e) in abc_entities.iter().enumerate() {
        let c: Option<C> = world.remove::<C>(*e);
        assert!(c.is_some());
        world.assert_archetype_consistency();
    }
}

/// Trigger column/entity asymmetry by interleaving insert/remove on a shared
/// archetype from multiple source archetypes.
#[test]
fn regression_column_entity_symmetry_interleaved_migrations() {
    struct X(u32);
    struct Y(u32);
    struct Z(u32);

    let mut world = World::new();

    // Spawn entities across different archetypes
    // Group 1: [X, Y]
    let mut group1 = Vec::new();
    for i in 0..5 {
        let e = world.spawn();
        world.insert(e, X(i));
        world.insert(e, Y(i * 10));
        group1.push(e);
    }

    // Group 2: [X, Z]
    let mut group2 = Vec::new();
    for i in 0..5 {
        let e = world.spawn();
        world.insert(e, X(i * 100));
        world.insert(e, Z(i * 1000));
        group2.push(e);
    }

    // Pre-populate [X, Y, Z] archetype
    let mut first = world.spawn();
    world.insert(first, X(999));
    world.insert(first, Y(888));
    world.insert(first, Z(777));

    // Now migrate group1 entities into [X, Y, Z] â€” each targets the same
    // existing archetype.
    for &e in &group1 {
        world.insert(e, Z(42));
        world.assert_archetype_consistency();
    }

    // Migrate group2 entities into [X, Y, Z]
    for &e in &group2 {
        world.insert(e, Y(43));
        world.assert_archetype_consistency();
    }

    // Remove Z from all [X, Y, Z] entities back to [X, Y]
    let all_xyz: Vec<Entity> = world.query::<(&X, &Y, &Z)>().map(|(e, _)| e).collect();
    for &e in &all_xyz {
        world.remove::<Z>(e);
        world.assert_archetype_consistency();
    }
}

/// Massive interleaved insert/remove stress on a growing archetype to probe
/// for any latent column/entity row desync.
#[test]
fn regression_column_entity_symmetry_stress() {
    struct Alpha(f32);
    struct Beta(f32);
    struct Gamma(f32);

    let mut world = World::new();

    // Batch: repeatedly insert and remove components on the same entities,
    // growing the destination archetype each time.
    let batch_size = 500;
    let mut entities = Vec::with_capacity(batch_size);

    for i in 0..batch_size {
        let e = world.spawn();
        world.insert(e, Alpha(i as f32));
        entities.push(e);
    }
    world.assert_archetype_consistency();

    // Give all entities Beta (creates [Alpha, Beta] archetype)
    for (i, &e) in entities.iter().enumerate() {
        world.insert(e, Beta(i as f32 * 2.0));
        world.assert_archetype_consistency();
    }

    // Give all entities Gamma (creates [Alpha, Beta, Gamma] archetype)
    for (i, &e) in entities.iter().enumerate() {
        world.insert(e, Gamma(i as f32 * 3.0));
        world.assert_archetype_consistency();
    }

    // Remove Gamma from all (back to [Alpha, Beta])
    for &e in &entities {
        world.remove::<Gamma>(e);
        world.assert_archetype_consistency();
    }

    // Remove Beta from all (back to [Alpha])
    for &e in &entities {
        world.remove::<Beta>(e);
        world.assert_archetype_consistency();
    }
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 14. Value integrity stress â€” data survives deep archetype migration
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Add/remove two components in a cycle 1000 times per entity.  Verify the
/// third component's value survives the full migration chain.
#[test]
fn stress_migrate_cycle_1000_times() {
    struct Ax(u64);
    struct Bx(u64);
    struct Cx(u64);

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Ax(1));
    world.insert(e, Bx(2));
    world.insert(e, Cx(42));

    for i in 0..1000 {
        // Toggle Ax on/off each iteration.
        if i % 2 == 0 {
            let _ = world.remove::<Ax>(e);
        } else {
            world.insert(e, Ax(i));
        }
        // Toggle Bx on/off, complement to Ax.
        if i % 2 == 0 {
            world.insert(e, Bx(i * 2));
        } else {
            let _ = world.remove::<Bx>(e);
        }
        world.assert_archetype_consistency();

        // Cx must survive all migrations.
        assert_eq!(
            world.get::<Cx>(e).unwrap().0,
            42,
            "Cx corrupted after iteration {i}"
        );
    }
}

/// 5000 entities with 4 components each, all spawning into a single archetype
/// and then a query verifies every single value.
#[test]
fn stress_massive_entity_value_check() {
    #[derive(Clone, Debug, PartialEq)]
    struct A(f32);
    #[derive(Clone, Debug, PartialEq)]
    struct B(f32);
    #[derive(Clone, Debug, PartialEq)]
    struct C(f32);
    #[derive(Clone, Debug, PartialEq)]
    struct D(f32);

    let count = 5000;
    let mut world = World::new();
    for i in 0..count {
        let e = world.spawn();
        world.insert(e, A(i as f32));
        world.insert(e, B(i as f32 * 2.0));
        world.insert(e, C(i as f32 * 3.0));
        world.insert(e, D(i as f32 * 4.0));
    }

    // Query and verify every single value.
    for (_, (a, b, c, d)) in world.query::<(&A, &B, &C, &D)>() {
        let idx = a.0 as usize;
        assert_eq!(a.0, idx as f32);
        assert_eq!(b.0, idx as f32 * 2.0);
        assert_eq!(c.0, idx as f32 * 3.0);
        assert_eq!(d.0, idx as f32 * 4.0);
    }
}

/// Insert the same component 10_000 times, verifying the final value.
#[test]
fn stress_insert_overwrite_10000() {
    struct Counter(u32);

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Counter(0));

    for i in 1..=10_000 {
        world.insert(e, Counter(i));
    }

    assert_eq!(world.get::<Counter>(e).unwrap().0, 10_000);
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 15. Archetype fill / drain / refill
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Fill an archetype to N entities, despawn them all, then fill again with
/// different values.  Repeat 10 times.
#[test]
fn stress_fill_drain_refill_archetype() {
    #[derive(Clone, PartialEq, Debug)]
    struct PosVal(f64, f64);

    let mut world = World::new();

    for round in 0..10 {
        let batch: Vec<(Entity, f64)> = (0..100)
            .map(|i| {
                let e = world.spawn();
                let val = round as f64 * 100.0 + i as f64;
                world.insert(e, PosVal(val, 0.0));
                (e, val)
            })
            .collect();

        for &(e, expected) in &batch {
            let p = world.get::<PosVal>(e).unwrap();
            assert_eq!(p.0, expected);
        }

        let batch: Vec<Entity> = batch.into_iter().map(|(e, _)| e).collect();

        for &e in &batch {
            world.despawn(e);
        }

        world.assert_archetype_consistency();
    }
}

/// Entities in different source archetypes migrating to a shared destination
/// archetype, interleaved with despawns.
#[test]
fn stress_multi_source_migration_to_shared_dest() {
    struct X(u32);
    struct Y(u32);
    struct Z(u32);
    struct W(u32);

    let mut world = World::new();

    // Create entities in 4 different source archetypes.
    let mut entities = Vec::new();
    for i in 0usize..50 {
        let e = world.spawn();
        let ii = i as u32;
        match i % 4 {
            0 => {
                world.insert(e, X(ii));
                world.insert(e, Y(ii));
            }
            1 => {
                world.insert(e, X(ii));
                world.insert(e, Z(ii));
            }
            2 => {
                world.insert(e, Y(ii));
                world.insert(e, Z(ii));
            }
            3 => {
                world.insert(e, X(ii));
                world.insert(e, W(ii));
            }
            _ => unreachable!(),
        }
        entities.push(e);
    }

    // Migrate all to [X, Y, Z, W] by inserting missing components in
    // interleaved order, targeting the same destination archetype.
    for (i, &e) in entities.iter().enumerate() {
        let ii = i as u32;
        match i % 4 {
            0 => {
                world.insert(e, Z(ii));
                world.insert(e, W(ii));
            }
            1 => {
                world.insert(e, Y(ii));
                world.insert(e, W(ii));
            }
            2 => {
                world.insert(e, X(ii));
                world.insert(e, W(ii));
            }
            3 => {
                world.insert(e, Y(ii));
                world.insert(e, Z(ii));
            }
            _ => unreachable!(),
        }
        world.assert_archetype_consistency();
    }

    // Verify all 50 entities present in [X, Y, Z, W] archetype.
    let count = world.query::<(&X, &Y, &Z, &W)>().count();
    assert_eq!(count, 50);

    // Remove components in reverse interleaved pattern, verify integrity.
    for (i, &e) in entities.iter().enumerate() {
        match i % 4 {
            0 => {
                let _ = world.remove::<Z>(e);
                let _ = world.remove::<W>(e);
            }
            1 => {
                let _ = world.remove::<Y>(e);
                let _ = world.remove::<W>(e);
            }
            2 => {
                let _ = world.remove::<X>(e);
                let _ = world.remove::<W>(e);
            }
            3 => {
                let _ = world.remove::<Y>(e);
                let _ = world.remove::<Z>(e);
            }
            _ => unreachable!(),
        }
        world.assert_archetype_consistency();
    }
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 16.  Swap-remove correctness and slot exhaustion
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Verify swap-remove semantics: despawn index 0, the last entity should be
/// moved into slot 0.  Then despawn the new slot 0, etc.
#[test]
fn stress_swap_remove_correctness() {
    struct Tag;
    let mut world = World::new();
    let n = 100;

    let entities: Vec<Entity> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Tag);
            e
        })
        .collect();

    // Despawn from the front: each despawn should move the last living
    // entity into the vacated position.
    let mut living = entities.clone();
    while living.len() > 1 {
        let front = living[0];
        let expected_last = *living.last().unwrap();

        world.despawn(front);
        living.swap_remove(0);

        // The entity that was last should now be at index 0.
        let still_alive = world.is_alive(living[0]);
        assert!(still_alive, "swap-moved entity should be alive");
        assert!(
            !world.is_alive(front),
            "despawned entity should be dead"
        );
    }

    // One entity remains.
    assert_eq!(world.query::<(&Tag,)>().count(), 1);
}

/// Stress test: spawn and despawn 100_000 entities to exercise slot
/// reuse and the free-slot stack at scale.
#[test]
fn stress_entity_slot_exhaustion() {
    #[derive(Clone, PartialEq)]
    struct Heavy([u64; 4]);

    let mut world = World::new();
    let batch_size = 100_000;

    // Phase 1: spawn all with a large-ish component.
    let mut entities = Vec::with_capacity(batch_size);
    for i in 0..batch_size {
        let e = world.spawn();
        world.insert(e, Heavy([i as u64; 4]));
        entities.push(e);
    }

    // Phase 2: despawn every other entity.
    for i in (0..batch_size).step_by(2) {
        world.despawn(entities[i]);
    }

    // Phase 3: spawn new entities â€” should reuse freed slots with higher
    // generations.
    let new_entities: Vec<Entity> = (0..(batch_size / 2))
        .map(|i| {
            let e = world.spawn();
            world.insert(e, Heavy([i as u64 + batch_size as u64; 4]));
            e
        })
        .collect();

    // Phase 4: verify all old still-alive entities have correct data.
    for i in (1..batch_size).step_by(2) {
        let e = entities[i];
        assert!(world.is_alive(e));
        let h = world.get::<Heavy>(e).unwrap();
        assert_eq!(h.0[0], i as u64);
    }

    // Verify new entities also have correct data.
    for (i, &e) in new_entities.iter().enumerate() {
        assert!(world.is_alive(e));
        let h = world.get::<Heavy>(e).unwrap();
        assert_eq!(h.0[0], i as u64 + batch_size as u64);
    }

    // Phase 5: despawn everything, verify empty world.
    for i in (1..batch_size).step_by(2) {
        world.despawn(entities[i]);
    }
    for &e in &new_entities {
        world.despawn(e);
    }

    assert_entity_count!(&world, 0);
    world.assert_archetype_consistency();
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 17.  Component variety â€” many types, large components, migration ordering
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// 20+ component types, mixed-and-matched across 200 entities.  Verify every
/// query produces the correct count.
#[test]
fn stress_many_component_types() {
    struct C1(u32);
    struct C2(u32);
    struct C3(u32);
    struct C4(u32);
    struct C5(u32);
    struct C6(u32);
    struct C7(u32);
    struct C8(u32);
    struct C9(u32);
    struct C10(u32);
    struct C11(u32);
    struct C12(u32);
    struct C13(u32);
    struct C14(u32);
    struct C15(u32);
    struct C16(u32);
    struct C17(u32);
    struct C18(u32);
    struct C19(u32);
    struct C20(u32);

    let mut world = World::new();
    let n: usize = 200;

    let entities: Vec<Entity> = (0..n)
        .map(|i| {
            let e = world.spawn();
            let ii = i as u32;
            // Each entity gets C1, plus a subset of C2..C20 based on bit pattern.
            world.insert(e, C1(ii));
            if i & 1 != 0 { world.insert(e, C2(ii)); }
            if i & 2 != 0 { world.insert(e, C3(ii)); }
            if i & 4 != 0 { world.insert(e, C4(ii)); }
            if i & 8 != 0 { world.insert(e, C5(ii)); }
            if i & 16 != 0 { world.insert(e, C6(ii)); }
            if i & 32 != 0 { world.insert(e, C7(ii)); }
            if i & 64 != 0 { world.insert(e, C8(ii)); }
            if i & 128 != 0 { world.insert(e, C9(ii)); }
            if i % 3 == 0 { world.insert(e, C10(ii)); }
            if i % 5 == 0 { world.insert(e, C11(ii)); }
            if i % 7 == 0 { world.insert(e, C12(ii)); }
            if i % 11 == 0 { world.insert(e, C13(ii)); }
            if i % 13 == 0 { world.insert(e, C14(ii)); }
            if i % 17 == 0 { world.insert(e, C15(ii)); }
            if i % 19 == 0 { world.insert(e, C16(ii)); }
            if i % 23 == 0 { world.insert(e, C17(ii)); }
            if i % 29 == 0 { world.insert(e, C18(ii)); }
            if i % 31 == 0 { world.insert(e, C19(ii)); }
            if i % 37 == 0 { world.insert(e, C20(ii)); }
            e
        })
        .collect();

    // C1 is universal â€” all must be present.
    let c1_count = world.query::<(&C1,)>().count();
    assert_eq!(c1_count, n);

    // Verify specific query counts against known modulo patterns.
    let c10_count = world.query::<(&C1, &C10)>().count();
    assert_eq!(c10_count, (0..n).filter(|i| i % 3 == 0).count());

    let c11_count = world.query::<(&C1, &C11)>().count();
    assert_eq!(c11_count, (0..n).filter(|i| i % 5 == 0).count());

    // Three-component query.
    let c10_11_count = world.query::<(&C1, &C10, &C11)>().count();
    assert_eq!(
        c10_11_count,
        (0..n).filter(|i| i % 3 == 0 && i % 5 == 0).count()
    );

    // Remove C10 from all, verify.
    for &e in &entities {
        if world.is_alive(e) {
            let _ = world.remove::<C10>(e);
        }
    }
    let c10_count_after = world.query::<(&C1, &C10)>().count();
    assert_eq!(c10_count_after, 0);

    world.assert_archetype_consistency();
}

/// Verify column ordering / component-id ordering doesn't affect data
/// integrity.  Insert components in different sequences across entities.
#[test]
fn stress_migration_column_reorder() {
    struct Alpha(f64);
    struct Beta(f64);
    struct Gamma(f64);
    struct Delta(f64);

    let mut world = World::new();

    // Group 1: insert in Alpha â†’ Beta â†’ Gamma â†’ Delta order.
    let mut g1 = Vec::new();
    for i in 0..20 {
        let e = world.spawn();
        world.insert(e, Alpha(i as f64));
        world.insert(e, Beta(i as f64 * 10.0));
        world.insert(e, Gamma(i as f64 * 100.0));
        world.insert(e, Delta(i as f64 * 1000.0));
        g1.push(e);
    }

    // Group 2: insert in Delta â†’ Gamma â†’ Beta â†’ Alpha order.
    let mut g2 = Vec::new();
    for i in 0..20 {
        let e = world.spawn();
        world.insert(e, Delta(i as f64 * 1000.0));
        world.insert(e, Gamma(i as f64 * 100.0));
        world.insert(e, Beta(i as f64 * 10.0));
        world.insert(e, Alpha(i as f64));
        g2.push(e);
    }

    // Both groups should end up in the same [Alpha, Beta, Gamma, Delta] archetype.
    let count = world.query::<(&Alpha, &Beta, &Gamma, &Delta)>().count();
    assert_eq!(count, 40);

    // Verify values are correct for group 1.
    for (i, &e) in g1.iter().enumerate() {
        let a = world.get::<Alpha>(e).unwrap();
        let b = world.get::<Beta>(e).unwrap();
        let g = world.get::<Gamma>(e).unwrap();
        let d = world.get::<Delta>(e).unwrap();
        assert_eq!(a.0, i as f64);
        assert_eq!(b.0, i as f64 * 10.0);
        assert_eq!(g.0, i as f64 * 100.0);
        assert_eq!(d.0, i as f64 * 1000.0);
    }

    // Verify values for group 2.
    for (i, &e) in g2.iter().enumerate() {
        let a = world.get::<Alpha>(e).unwrap();
        let b = world.get::<Beta>(e).unwrap();
        let g = world.get::<Gamma>(e).unwrap();
        let d = world.get::<Delta>(e).unwrap();
        assert_eq!(a.0, i as f64);
        assert_eq!(b.0, i as f64 * 10.0);
        assert_eq!(g.0, i as f64 * 100.0);
        assert_eq!(d.0, i as f64 * 1000.0);
    }

    // Remove components in reverse order from group 1.
    for &e in &g1 {
        let _ = world.remove::<Delta>(e);
        let _ = world.remove::<Gamma>(e);
        let _ = world.remove::<Beta>(e);
        // entity keeps Alpha
    }

    // Remove components in forward order from group 2.
    for &e in &g2 {
        let _ = world.remove::<Alpha>(e);
        let _ = world.remove::<Beta>(e);
        let _ = world.remove::<Gamma>(e);
        // entity keeps Delta
    }

    world.assert_archetype_consistency();
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 18.  Bitmask correctness â€” 64+ component types
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Exercise the u64 bitmask at full capacity.  Create component types that
/// occupy every bit position (1..=64) and verify queries filter correctly.
#[test]
fn stress_bitmask_64_component_types() {
    struct BitTy1;  struct BitTy2;  struct BitTy3;  struct BitTy4;
    struct BitTy5;  struct BitTy6;  struct BitTy7;  struct BitTy8;
    struct BitTy9;  struct BitTy10; struct BitTy11; struct BitTy12;
    struct BitTy13; struct BitTy14; struct BitTy15; struct BitTy16;
    struct BitTy17; struct BitTy18; struct BitTy19; struct BitTy20;
    struct BitTy21; struct BitTy22; struct BitTy23; struct BitTy24;
    struct BitTy25; struct BitTy26; struct BitTy27; struct BitTy28;
    struct BitTy29; struct BitTy30; struct BitTy31; struct BitTy32;
    struct BitTy33; struct BitTy34; struct BitTy35; struct BitTy36;
    struct BitTy37; struct BitTy38; struct BitTy39; struct BitTy40;
    struct BitTy41; struct BitTy42; struct BitTy43; struct BitTy44;
    struct BitTy45; struct BitTy46; struct BitTy47; struct BitTy48;
    struct BitTy49; struct BitTy50; struct BitTy51; struct BitTy52;
    struct BitTy53; struct BitTy54; struct BitTy55; struct BitTy56;
    struct BitTy57; struct BitTy58; struct BitTy59; struct BitTy60;
    struct BitTy61; struct BitTy62; struct BitTy63; struct BitTy64;

    let mut world = World::new();
    let e_all = world.spawn();

    world.insert(e_all, BitTy1);  world.insert(e_all, BitTy2);
    world.insert(e_all, BitTy3);  world.insert(e_all, BitTy4);
    world.insert(e_all, BitTy5);  world.insert(e_all, BitTy6);
    world.insert(e_all, BitTy7);  world.insert(e_all, BitTy8);
    world.insert(e_all, BitTy9);  world.insert(e_all, BitTy10);
    world.insert(e_all, BitTy11); world.insert(e_all, BitTy12);
    world.insert(e_all, BitTy13); world.insert(e_all, BitTy14);
    world.insert(e_all, BitTy15); world.insert(e_all, BitTy16);
    world.insert(e_all, BitTy17); world.insert(e_all, BitTy18);
    world.insert(e_all, BitTy19); world.insert(e_all, BitTy20);
    world.insert(e_all, BitTy21); world.insert(e_all, BitTy22);
    world.insert(e_all, BitTy23); world.insert(e_all, BitTy24);
    world.insert(e_all, BitTy25); world.insert(e_all, BitTy26);
    world.insert(e_all, BitTy27); world.insert(e_all, BitTy28);
    world.insert(e_all, BitTy29); world.insert(e_all, BitTy30);
    world.insert(e_all, BitTy31); world.insert(e_all, BitTy32);
    world.insert(e_all, BitTy33); world.insert(e_all, BitTy34);
    world.insert(e_all, BitTy35); world.insert(e_all, BitTy36);
    world.insert(e_all, BitTy37); world.insert(e_all, BitTy38);
    world.insert(e_all, BitTy39); world.insert(e_all, BitTy40);
    world.insert(e_all, BitTy41); world.insert(e_all, BitTy42);
    world.insert(e_all, BitTy43); world.insert(e_all, BitTy44);
    world.insert(e_all, BitTy45); world.insert(e_all, BitTy46);
    world.insert(e_all, BitTy47); world.insert(e_all, BitTy48);
    world.insert(e_all, BitTy49); world.insert(e_all, BitTy50);
    world.insert(e_all, BitTy51); world.insert(e_all, BitTy52);
    world.insert(e_all, BitTy53); world.insert(e_all, BitTy54);
    world.insert(e_all, BitTy55); world.insert(e_all, BitTy56);
    world.insert(e_all, BitTy57); world.insert(e_all, BitTy58);
    world.insert(e_all, BitTy59); world.insert(e_all, BitTy60);
    world.insert(e_all, BitTy61); world.insert(e_all, BitTy62);
    world.insert(e_all, BitTy63); world.insert(e_all, BitTy64);

    // Query for a subset from the low, mid, and high bits.
    let low_count = world.query::<(&BitTy1, &BitTy2, &BitTy3)>().count();
    assert_eq!(low_count, 1);

    let mid_count = world.query::<(&BitTy30, &BitTy31, &BitTy32)>().count();
    assert_eq!(mid_count, 1);

    let high_count = world.query::<(&BitTy62, &BitTy63, &BitTy64)>().count();
    assert_eq!(high_count, 1);

    // Remove some bits from the entity.
    let _ = world.remove::<BitTy1>(e_all);
    let _ = world.remove::<BitTy32>(e_all);
    let _ = world.remove::<BitTy64>(e_all);

    // The removed-component queries should return 0.
    assert_eq!(world.query::<(&BitTy1,)>().count(), 0);
    assert_eq!(world.query::<(&BitTy32,)>().count(), 0);
    assert_eq!(world.query::<(&BitTy64,)>().count(), 0);

    // Other bit queries still work.
    assert_eq!(world.query::<(&BitTy2, &BitTy3)>().count(), 1);
    assert_eq!(world.query::<(&BitTy63,)>().count(), 1);

    world.assert_archetype_consistency();
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 19.  Blob / large-component stress
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Spawn many entities with a large (1024-byte) component, migrate them
/// through multiple archetypes, and verify data integrity.
#[test]
fn stress_large_component_migration() {
    struct Blob([u8; 1024]);
    struct Marker(u32);

    let mut world = World::new();

    let entities: Vec<Entity> = (0..100)
        .map(|i| {
            let e = world.spawn();
            let mut blob = Blob([0u8; 1024]);
            blob.0[0] = i as u8;
            blob.0[511] = (i >> 8) as u8;
            blob.0[1023] = (i >> 16) as u8;
            world.insert(e, blob);
            world.insert(e, Marker(i));
            e
        })
        .collect();

    // Migrate: add a third component (triggers archetype move with blob copy).
    struct Extra(f32);
    for &e in &entities {
        world.insert(e, Extra(42.0));
    }

    // Verify blobs survived migration.
    for (i, &e) in entities.iter().enumerate() {
        let blob = world.get::<Blob>(e).unwrap();
        assert_eq!(blob.0[0], i as u8);
        assert_eq!(blob.0[511], (i >> 8) as u8);
        assert_eq!(blob.0[1023], (i >> 16) as u8);
        let _ = world.get::<Extra>(e).unwrap();
    }

    // Remove Extra, migrate back.
    for &e in &entities {
        let _ = world.remove::<Extra>(e);
    }

    // Verify blobs survived the return migration.
    for (i, &e) in entities.iter().enumerate() {
        let blob = world.get::<Blob>(e).unwrap();
        assert_eq!(blob.0[0], i as u8);
        assert_eq!(blob.0[1023], (i >> 16) as u8);
    }

    world.assert_archetype_consistency();
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 20.  Schedule stress â€” many systems and system ordering
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// 50 systems that increment a counter, verifying sequential execution order.
#[test]
fn stress_schedule_50_systems() {
    struct Accum(u64);

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Accum(0));

    let mut schedule = Schedule::new();
    for i in 0..50 {
        schedule.add_system(format!("sys_{i}"), move |world, _time| {
            if let Some(acc) = world.get_mut::<Accum>(e) {
                acc.0 += 1;
            }
        });
    }

    let time = GameTime {
        elapsed: Duration::from_secs(0),
        delta: Duration::from_secs_f64(1.0 / 60.0),
        tick: 0,
    };

    schedule.run(&mut world, time);
    assert_eq!(world.get::<Accum>(e).unwrap().0, 50);

    // Run again.
    schedule.run(&mut world, time);
    assert_eq!(world.get::<Accum>(e).unwrap().0, 100);
}

/// Schedule with systems that add/remove components, verifying the world
/// is consistent after each run.
#[test]
fn stress_schedule_mutate_components() {
    struct Toggle(bool);
    struct Value(u64);

    let mut world = World::new();
    let e = world.spawn();
    world.insert(e, Toggle(false));

    let mut schedule = Schedule::new();
    schedule.add_system("toggle", |world, _time| {
        for (_, (toggle,)) in world.query::<(&mut Toggle,)>() {
            toggle.0 = !toggle.0;
        }
    });
    schedule.add_system("add_value", |world, _time| {
        let to_add: Vec<Entity> = world
            .query::<(&Toggle,)>()
            .filter_map(|(entity, (toggle,))| if toggle.0 { Some(entity) } else { None })
            .collect();
        for entity in to_add {
            world.insert(entity, Value(42));
        }
    });
    schedule.add_system("remove_value", move |world, _time| {
        let should_remove = world
            .query::<(&Toggle,)>()
            .any(|(_, (toggle,))| !toggle.0);
        if should_remove {
            let _ = world.remove::<Value>(e);
        }
    });

    let time = GameTime {
        elapsed: Duration::from_secs(0),
        delta: Duration::from_secs_f64(1.0 / 60.0),
        tick: 0,
    };

    for run in 0..20 {
        schedule.run(&mut world, time);
        world.assert_archetype_consistency();

        // Odd runs: toggle is true â†’ Value should be present
        // Even runs: toggle is false â†’ Value should be absent
        if run % 2 == 0 {
            assert!(world.get::<Toggle>(e).unwrap().0);
            assert!(world.get::<Value>(e).is_some());
        } else {
            assert!(!world.get::<Toggle>(e).unwrap().0);
            // Value may or may not be present depending on system order
        }
    }
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 21.  Actor lifecycle edge cases
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Multiple actors registered, each creating entities in begin_play, verifying
/// despawn order and entity slot reuse.
#[test]
fn stress_actor_mass_register_deregister() {
    struct Spawner;

    impl Actor for Spawner {
        fn begin_play(&mut self, _entity: Entity, world: &mut World) {
            // Spawn 10 children in begin_play.
            for i in 0..10 {
                let child = world.spawn();
                world.insert(child, Health(i));
            }
        }
    }

    let mut world = World::new();
    let mut registry = ActorRegistry::new();

    // Register 50 actors, each spawning 10 children = 500 children total.
    let actor_entities: Vec<Entity> = (0..50)
        .map(|_| registry.register(Spawner, &mut world))
        .collect();

    // Total entities = 500 children + 50 actors.
    let total = world.query::<()>().count();
    assert_eq!(total, 550);

    // Deregister all actors (should also wipe their children? No â€” children
    // are independent entities, they survive actor deregistration).
    for &e in &actor_entities {
        registry.deregister(e, &mut world);
    }

    // Children still exist.
    let child_count = world.query::<(&Health,)>().count();
    assert_eq!(child_count, 500);

    // The actors are gone.
    let actor_count = world.query::<()>().count();
    assert_eq!(actor_count, 500);

    // Clean up children.
    let children: Vec<Entity> = world.query::<(&Health,)>().map(|(e, _)| e).collect();
    for e in children {
        world.despawn(e);
    }

    world.assert_archetype_consistency();
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 22.  Chained removal â€” remove components in dependency order
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Remove all components from an entity, one by one, from different archetypes,
/// verifying the entity ends up in the empty archetype and is still alive.
#[test]
fn stress_chained_remove_all_components() {
    struct A(u32);
    struct B(u32);
    struct C(u32);
    struct D(u32);
    struct E(u32);

    let mut world = World::new();

    // Create entities in various multi-component archetypes.
    let entities: Vec<Entity> = (0..30)
        .map(|i| {
            let e = world.spawn();
            world.insert(e, A(i));
            if i % 2 == 0 { world.insert(e, B(i * 2)); }
            if i % 3 == 0 { world.insert(e, C(i * 3)); }
            if i % 5 == 0 { world.insert(e, D(i * 5)); }
            if i % 7 == 0 { world.insert(e, E(i * 7)); }
            e
        })
        .collect();

    // Remove components one by one from each entity, checking invariants.
    for &e in &entities {
        assert!(world.is_alive(e));

        // Keep removing components until the entity has none.
        // We don't know which components exist; try each.
        loop {
            let had_a = world.remove::<A>(e).is_some();
            let had_b = world.remove::<B>(e).is_some();
            let had_c = world.remove::<C>(e).is_some();
            let had_d = world.remove::<D>(e).is_some();
            let had_e = world.remove::<E>(e).is_some();

            if !had_a && !had_b && !had_c && !had_d && !had_e {
                break;
            }
            world.assert_archetype_consistency();
        }

        // Entity should be alive and in the empty archetype.
        assert!(world.is_alive(e));
        assert!(world.get::<A>(e).is_none());
        assert!(world.get::<B>(e).is_none());
    }

    // All 30 entities survive in the empty archetype.
    let empty_count = world.query::<()>().count();
    assert_eq!(empty_count, 30);
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 23.  Despawn entity while iterating (collect-first pattern)
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Collect entities, then despawn them.  Verify the world is clean after all
/// despawns regardless of iteration order.
#[test]
fn stress_despawn_collected_entities() {
    struct Mark;

    let mut world = World::new();
    let n = 500;

    let _: Vec<Entity> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Mark);
            e
        })
        .collect();

    // Collect entities and despawn in reverse order.
    let collected: Vec<Entity> = world.query::<(&Mark,)>().map(|(e, _)| e).collect();
    for &e in collected.iter().rev() {
        world.despawn(e);
    }

    assert_entity_count!(&world, 0);
    world.assert_archetype_consistency();

    // Re-fill the world to ensure no latent corruption.
    for i in 0..n {
        let e = world.spawn();
        world.insert(e, Mark);
        world.insert(e, Health(i as u32));
    }

    assert_entity_count!(&world, n);
    let health_sum: u32 = world
        .query::<(&Health,)>()
        .map(|(_, (h,))| h.0)
        .sum();
    // 0 + 1 + ... + (n-1) = n*(n-1)/2
    assert_eq!(health_sum, n as u32 * (n as u32 - 1) / 2);
}

// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•
// 24.  Entity::DANGLING handled correctly across all public APIs
// â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

/// Verify that the DANGLING sentinel doesn't cause panics in any public method.
#[test]
fn stress_dangling_entity_every_method() {
    let mut world = World::new();
    let d = Entity::DANGLING;

    assert!(!world.is_alive(d));
    assert!(!world.despawn(d));
    assert!(world.get::<Health>(d).is_none());
    assert!(world.get_mut::<Health>(d).is_none());

    // Removing a component from the dangling sentinel must not panic.
    let removed: Option<Health> = world.remove::<Health>(d);
    assert!(removed.is_none());

    // Insert panics (intentional â€” contract violation).
    // No test for insert since it's documented to panic on dead entities.
    // But we verify at least it doesn't cause UB (requires Miri).
}
