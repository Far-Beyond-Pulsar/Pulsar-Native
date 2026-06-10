// Comprehensive Criterion benchmark suite for pulsar_scenedb.
//
// This file provides statistically rigorous benchmarks with:
// - Multiple entity scales (100 â†’ 100K)
// - Multiple component counts (0 â†’ 8 per entity)
// - Archetype pressure scenarios
// - Memory allocation tracking
// - Throughput measurements
//
// Run all benchmarks:
//   cargo bench --bench ecs_detailed_bench
//
// Run a specific benchmark group:
//   cargo bench --bench ecs_detailed_bench -- spawn
//   cargo bench --bench ecs_detailed_bench -- query
//   cargo bench --bench ecs_detailed_bench -- churn
//
// View the HTML report (after running):
//   open target/criterion/report/index.html
//
// The report includes:
//   - Mean/median timing with confidence intervals
//   - Distribution histograms (sample times)
//   - Throughput (entities/sec, queries/sec, etc.)
//   - Regression detection (compares against baseline)
//
// Generate a standalone HTML report:
//   cargo bench --bench ecs_detailed_bench --save-baseline baseline
//   # ... make changes ...
//   cargo bench --bench ecs_detailed_bench --baseline baseline
//
// To export data for external analysis (CSV):
//   cargo bench --bench ecs_detailed_bench --output-format json
// Then parse the JSON files in target/criterion/<bench-name>/

use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};
use pulsar_scenedb::*;
use std::time::Duration;

// â”€â”€ Component types â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

#[derive(Clone, Copy)]
struct Pos(f32, f32, f32);
#[derive(Clone, Copy)]
struct Vel(f32, f32, f32);
#[derive(Clone, Copy)]
struct Health(u32);
#[derive(Clone, Copy)]
struct Tag;
#[derive(Clone)]
struct Name(String);
#[derive(Clone, Copy)]
struct Weight(f64);
#[derive(Clone, Copy)]
struct Color([f32; 4]);
#[derive(Clone, Copy)]
struct Lifetime(f32);

// =============================================================================
// 1. SPAWN BENCHMARKS
// =============================================================================
// Measures entity allocation throughput at various scales.
// Tests empty spawn, single-component spawn, and multi-component spawn.
//
// What it measures:
//   - Free-slot recycling (warm pool)
//   - Entity slot Vec allocation
//   - Archetype push (empty archetype)
//   - Generation counter increment
//
// How to view:
//   - HTML report: target/criterion/spawn_n/index.html
//   - Look for "Mean" row for average time per iteration
//   - Look for "Throughput" row for entities/sec
// =============================================================================

fn spawn_n(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn_n");

    // Configure criterion for statistical rigor.
    group
        .measurement_time(Duration::from_secs(10));

    for &n in &[100, 1_000, 10_000, 50_000, 100_000] {
        group.throughput(Throughput::Elements(n as u64));

        // Empty spawn: baseline allocation cost.
        group.bench_with_input(BenchmarkId::new("empty", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                for _ in 0..n {
                    black_box(world.spawn());
                }
                black_box(world);
            });
        });

        // Single component: spawn + first insert (archetype creation).
        group.bench_with_input(BenchmarkId::new("with_Pos", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                for _ in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    black_box(e);
                }
                black_box(world);
            });
        });

        // Four components: spawn + multiple inserts (archetype migration).
        group.bench_with_input(BenchmarkId::new("with_4_comps", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                for _ in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    world.insert(e, Vel(0.0, 0.0, 0.0));
                    world.insert(e, Health(100));
                    world.insert(e, Tag);
                    black_box(e);
                }
                black_box(world);
            });
        });

        // Eight components: maximal archetype complexity.
        group.bench_with_input(BenchmarkId::new("with_8_comps", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                for _ in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    world.insert(e, Vel(0.0, 0.0, 0.0));
                    world.insert(e, Health(100));
                    world.insert(e, Tag);
                    world.insert(e, Name("test".into()));
                    world.insert(e, Weight(50.0));
                    world.insert(e, Color([1.0, 0.0, 0.0, 1.0]));
                    world.insert(e, Lifetime(10.0));
                    black_box(e);
                }
                black_box(world);
            });
        });
    }

    group.finish();
}

// =============================================================================
// 2. QUERY BENCHMARKS
// =============================================================================
// Measures query iteration throughput at various scales and tuple sizes.
// Tests archetype bitmask filtering, column fetch, and iterator overhead.
//
// What it measures:
//   - Archetype iteration (scanning all archetypes)
//   - Bitmask check (fast skip of non-matching archetypes)
//   - Column lookup by ComponentId
//   - Pointer arithmetic for component data access
//   - Iterator next() overhead
//
// How to view:
//   - HTML report: target/criterion/query_single/index.html
//   - "Mean" = average time per query iteration
//   - "Throughput" = entities processed per second
// =============================================================================

fn query_single(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_single");

    group
        .measurement_time(Duration::from_secs(10));

    for &n in &[100, 1_000, 10_000, 50_000, 100_000] {
        let mut world = World::new();

        // Create matching entities (Pos + Health).
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
        }

        // Add non-matching entities (Vel only) to test archetype skipping.
        for _ in 0..(n / 10) {
            let e = world.spawn();
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }

        group.throughput(Throughput::Elements(n as u64));

        group.bench_with_input(BenchmarkId::new("iter", n), &n, |b, &n| {
            b.iter(|| {
                let mut count = 0u64;
                for (_entity, (pos, health)) in world.query::<(&Pos, &Health)>() {
                    black_box((pos, health));
                    count += 1;
                }
                black_box(count);
                assert_eq!(count, n as u64);
            });
        });
    }

    group.finish();
}

fn query_tuple_size(c: &mut Criterion) {
    let n = 10_000;
    let mut group = c.benchmark_group("query_tuple_size");

    group
        .measurement_time(Duration::from_secs(10));

    // Build world with all 8 component types.
    let mut world = World::new();
    for _ in 0..n {
        let e = world.spawn();
        world.insert(e, Pos(1.0, 2.0, 3.0));
        world.insert(e, Vel(0.0, 0.0, 0.0));
        world.insert(e, Health(100));
        world.insert(e, Tag);
        world.insert(e, Name("test".into()));
        world.insert(e, Weight(50.0));
        world.insert(e, Color([1.0, 0.0, 0.0, 1.0]));
        world.insert(e, Lifetime(10.0));
    }

    group.throughput(Throughput::Elements(n));

    // Query with 1 component.
    group.bench_function("tuple_1", |b| {
        b.iter(|| {
            let mut count = 0u64;
            for (_e, pos) in world.query::<(&Pos,)>() {
                black_box(pos);
                count += 1;
            }
            black_box(count);
        });
    });

    // Query with 2 components.
    group.bench_function("tuple_2", |b| {
        b.iter(|| {
            let mut count = 0u64;
            for (_e, (pos, health)) in world.query::<(&Pos, &Health)>() {
                black_box((pos, health));
                count += 1;
            }
            black_box(count);
        });
    });

    // Query with 4 components.
    group.bench_function("tuple_4", |b| {
        b.iter(|| {
            let mut count = 0u64;
            for (_e, (pos, vel, health, tag)) in
                world.query::<(&Pos, &Vel, &Health, &Tag)>()
            {
                black_box((pos, vel, health, tag));
                count += 1;
            }
            black_box(count);
        });
    });

    // Query with 8 components.
    group.bench_function("tuple_8", |b| {
        b.iter(|| {
            let mut count = 0u64;
            for (_e, (pos, vel, health, tag, name, weight, color, life)) in
                world.query::<(&Pos, &Vel, &Health, &Tag, &Name, &Weight, &Color, &Lifetime)>()
            {
                black_box((pos, vel, health, name, weight, color, life));
                count += 1;
            }
            black_box(count);
        });
    });

    // Query with () â€” matches all archetypes, no data fetch.
    group.bench_function("tuple_0", |b| {
        b.iter(|| {
            let mut count = 0u64;
            for (_e, ()) in world.query::<()>() {
                count += 1;
            }
            black_box(count);
        });
    });

    group.finish();
}

fn query_selectivity(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_selectivity");

    group
        .measurement_time(Duration::from_secs(10));

    for &n in &[10_000, 50_000] {
        let mut world = World::new();

        // Create entities spread across different archetypes.
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
        }
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
        }
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }

        let total = n * 4;
        group.throughput(Throughput::Elements(total as u64));

        // Query for (Pos, Health) â€” matches ~50% of entities.
        group.bench_with_input(BenchmarkId::new("50pct_match", n), &n, |b, &n| {
            b.iter(|| {
                let mut count = 0u64;
                for (_e, (pos, health)) in world.query::<(&Pos, &Health)>() {
                    black_box((pos, health));
                    count += 1;
                }
                black_box(count);
            });
        });

        // Query for (Pos, Vel, Health) â€” matches ~25% of entities.
        group.bench_with_input(BenchmarkId::new("25pct_match", n), &n, |b, &n| {
            b.iter(|| {
                let mut count = 0u64;
                for (_e, (pos, vel, health)) in world.query::<(&Pos, &Vel, &Health)>() {
                    black_box((pos, vel, health));
                    count += 1;
                }
                black_box(count);
            });
        });

        // Query for () â€” matches 100% of entities, all archetypes.
        group.bench_with_input(BenchmarkId::new("100pct_match", n), &n, |b, &n| {
            b.iter(|| {
                let mut count = 0u64;
                for (_e, ()) in world.query::<()>() {
                    count += 1;
                }
                black_box(count);
            });
        });
    }

    group.finish();
}

// =============================================================================
// 3. INSERT / ARCHETYPE MIGRATION BENCHMARKS
// =============================================================================
// Measures the cost of component insertion and archetype migration.
// Tests both the fast path (archetype already exists) and slow path
// (new archetype creation).
//
// What it measures:
//   - Archetype index lookup (AHashMap)
//   - Archetype creation (new archetype + key insertion)
//   - Column allocation (Vec extension, Box allocation)
//   - Entity migration (swap-remove + push across columns)
//   - Slot map update
//
// How to view:
//   - HTML report: target/criterion/archetype_migration/index.html
//   - Compare "insert_cached" vs "insert_new" to see archetype creation cost.
// =============================================================================

fn archetype_migration(c: &mut Criterion) {
    let mut group = c.benchmark_group("archetype_migration");

    group
        .measurement_time(Duration::from_secs(10));

    let n = 10_000;

    // Phase 1: insert first component (always creates new archetype).
    group.bench_function("insert_first_component", |b| {
        b.iter(|| {
            let mut world = World::new();
            let entities: Vec<_> = (0..n).map(|_| world.spawn()).collect();
            for &e in &entities {
                world.insert(e, Pos(1.0, 2.0, 3.0));
            }
            black_box(world);
        });
    });

    // Phase 2: insert into existing archetype (fast path).
    {
        let mut world = World::new();
        let entities: Vec<_> = (0..n)
            .map(|_| {
                let e = world.spawn();
                world.insert(e, Pos(1.0, 2.0, 3.0));
                e
            })
            .collect();

        group.bench_function("insert_cached_archetype", |b| {
            b.iter(|| {
                let mut world = World::new();
                let entities: Vec<_> = (0..n)
                    .map(|_| {
                        let e = world.spawn();
                        world.insert(e, Pos(1.0, 2.0, 3.0));
                        e
                    })
                    .collect();
                for &e in &entities {
                    world.insert(e, Vel(0.0, 0.0, 0.0));
                }
                black_box(world);
            });
        });
    }

    // Phase 3: remove component (migration back).
    {
        let mut world = World::new();
        let entities: Vec<_> = (0..n)
            .map(|_| {
                let e = world.spawn();
                world.insert(e, Pos(1.0, 2.0, 3.0));
                world.insert(e, Health(100));
                e
            })
            .collect();

        group.bench_function("remove_component", |b| {
            b.iter(|| {
                let mut world = World::new();
                let entities: Vec<_> = (0..n)
                    .map(|_| {
                        let e = world.spawn();
                        world.insert(e, Pos(1.0, 2.0, 3.0));
                        world.insert(e, Health(100));
                        e
                    })
                    .collect();
                for &e in &entities {
                    world.remove::<Health>(e);
                }
                black_box(world);
            });
        });
    }

    // Phase 4: overwrite existing component (no migration).
    {
        let mut world = World::new();
        let entities: Vec<_> = (0..n)
            .map(|_| {
                let e = world.spawn();
                world.insert(e, Pos(1.0, 2.0, 3.0));
                world.insert(e, Health(100));
                e
            })
            .collect();

        group.bench_function("overwrite_component", |b| {
            b.iter(|| {
                let mut world = World::new();
                let entities: Vec<_> = (0..n)
                    .map(|_| {
                        let e = world.spawn();
                        world.insert(e, Pos(1.0, 2.0, 3.0));
                        world.insert(e, Health(100));
                        e
                    })
                    .collect();
                for &e in &entities {
                    world.insert(e, Pos(4.0, 5.0, 6.0));
                }
                black_box(world);
            });
        });
    }

    group.finish();
}

// =============================================================================
// 4. DESPAWN BENCHMARKS
// =============================================================================
// Measures entity removal throughput and slot recycling efficiency.
//
// What it measures:
//   - Generation counter increment
//   - Free slot push
//   - Archetype swap-remove
//   - Swapped-in entity row fixup
//
// How to view:
//   - HTML report: target/criterion/despawn/index.html
//   - Compare "despawn_ordered" vs "despawn_reversed" to detect
//     swap-remove optimization effects.
// =============================================================================

fn despawn(c: &mut Criterion) {
    let mut group = c.benchmark_group("despawn");

    group
        .measurement_time(Duration::from_secs(10));

    for &n in &[1_000, 10_000, 50_000] {
        let mut world = World::new();
        let entities: Vec<_> = (0..n)
            .map(|_| {
                let e = world.spawn();
                world.insert(e, Pos(1.0, 2.0, 3.0));
                world.insert(e, Health(100));
                e
            })
            .collect();

        group.throughput(Throughput::Elements(n as u64));

        // Despawn in order.
        group.bench_with_input(BenchmarkId::new("ordered", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                let entities: Vec<_> = (0..n)
                    .map(|_| {
                        let e = world.spawn();
                        world.insert(e, Pos(1.0, 2.0, 3.0));
                        world.insert(e, Health(100));
                        e
                    })
                    .collect();
                for &e in &entities {
                    world.despawn(e);
                }
                black_box(world);
            });
        });

        // Despawn in reverse (tests swap-remove on last element).
        group.bench_with_input(BenchmarkId::new("reversed", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                let entities: Vec<_> = (0..n)
                    .map(|_| {
                        let e = world.spawn();
                        world.insert(e, Pos(1.0, 2.0, 3.0));
                        world.insert(e, Health(100));
                        e
                    })
                    .collect();
                for i in (0..n).rev() {
                    world.despawn(entities[i]);
                }
                black_box(world);
            });
        });

        // Interleaved spawn + despawn (realistic churn).
        group.bench_with_input(BenchmarkId::new("interleaved", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                let mut entities = Vec::with_capacity(n);
                for i in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    if i % 2 == 0 {
                        world.despawn(e);
                    } else {
                        entities.push(e);
                    }
                }
                for &e in &entities {
                    world.despawn(e);
                }
                black_box(world);
            });
        });
    }

    group.finish();
}

// =============================================================================
// 5. CHURN BENCHMARKS
// =============================================================================
// Measures realistic entity lifecycle patterns: spawn â†’ insert â†’ query â†’
// remove â†’ despawn â†’ respawn.
//
// What it measures:
//   - End-to-end entity lifecycle cost
//   - Free slot reuse efficiency
//   - Archetype pressure under churn
//
// How to view:
//   - HTML report: target/criterion/churn/index.html
//   - Look at "Throughput" for entities processed per second.
//   - Compare different wave/entity counts.
// =============================================================================

fn churn(c: &mut Criterion) {
    let mut group = c.benchmark_group("churn");

    group
        .measurement_time(Duration::from_secs(10));

    for &n in &[100, 1_000, 10_000] {
        group.bench_with_input(BenchmarkId::new("spawn_insert_query_despawn", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                let mut entities = Vec::with_capacity(n);
                for _ in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    world.insert(e, Health(100));
                    entities.push(e);
                }
                // Query.
                let mut count = 0u64;
                for (_e, (pos, health)) in world.query::<(&Pos, &Health)>() {
                    black_box((pos, health));
                    count += 1;
                }
                black_box(count);
                // Despawn all.
                for &e in &entities {
                    world.despawn(e);
                }
                black_box(world);
            });
        });
    }

    // High-frequency churn: many small waves.
    group.bench_function("high_freq_100", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let mut world = World::new();
                let mut entities = Vec::with_capacity(100);
                for _ in 0..100 {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    entities.push(e);
                }
                for &e in &entities {
                    world.despawn(e);
                }
                black_box(world);
            }
        });
    });

    group.bench_function("high_freq_1000", |b| {
        b.iter(|| {
            for _ in 0..100 {
                let mut world = World::new();
                let mut entities = Vec::with_capacity(1_000);
                for _ in 0..1_000 {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    entities.push(e);
                }
                for &e in &entities {
                    world.despawn(e);
                }
                black_box(world);
            }
        });
    });

    group.finish();
}

// =============================================================================
// 6. ARCHETYPE PRESSURE BENCHMARKS
// =============================================================================
// Measures query performance as the number of archetypes grows.
// Tests the archetype scanning overhead in the query path.
//
// What it measures:
//   - Archetype iteration overhead (linear scan)
//   - Bitmask filtering effectiveness
//   - has_columns check cost
//
// How to view:
//   - HTML report: target/criterion/archetype_pressure/index.html
//   - Look for the scaling pattern: is it linear or sub-linear?
// =============================================================================

fn archetype_pressure(c: &mut Criterion) {
    let mut group = c.benchmark_group("archetype_pressure");

    group
        .measurement_time(Duration::from_secs(10));

    for &n in &[1_000, 5_000, 10_000, 50_000] {
        let mut world = World::new();

        // Spread entities across archetypes by varying component sets.
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
        }
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
        }
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }

        let total = n * 4;
        group.throughput(Throughput::Elements(total as u64));

        // Query for (Pos, Health) across many archetypes.
        group.bench_with_input(BenchmarkId::new("query_4_arch", n), &n, |b, &n| {
            b.iter(|| {
                let mut count = 0u64;
                for (_e, (pos, health)) in world.query::<(&Pos, &Health)>() {
                    black_box((pos, health));
                    count += 1;
                }
                black_box(count);
            });
        });

        // Query for () across all archetypes.
        group.bench_with_input(BenchmarkId::new("query_all_arch", n), &n, |b, &n| {
            b.iter(|| {
                let mut count = 0u64;
                for (_e, ()) in world.query::<()>() {
                    count += 1;
                }
                black_box(count);
            });
        });
    }

    group.finish();
}

// =============================================================================
// 7. GET/SET BENCHMARKS
// =============================================================================
// Measures component access via get/get_mut/insert (non-migration path).
//
// What it measures:
//   - Entity slot lookup
//   - Archetype lookup (bitmask + has_columns)
//   - Column downcast
//   - Pointer arithmetic for component data
//
// How to view:
//   - HTML report: target/criterion/component_access/index.html
//   - "get" vs "get_mut" shows shared vs exclusive path overhead.
// =============================================================================

fn component_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("component_access");

    group
        .measurement_time(Duration::from_secs(10));

    for &n in &[1_000, 10_000, 50_000] {
        let mut world = World::new();
        let entities: Vec<_> = (0..n)
            .map(|_| {
                let e = world.spawn();
                world.insert(e, Pos(1.0, 2.0, 3.0));
                world.insert(e, Health(100));
                e
            })
            .collect();

        // Get (shared reference).
        group.bench_with_input(BenchmarkId::new("get", n), &n, |b, &n| {
            b.iter(|| {
                for &e in &entities {
                    let pos = world.get::<Pos>(e);
                    black_box(pos);
                }
            });
        });

        // Get mut (exclusive reference).
        group.bench_with_input(BenchmarkId::new("get_mut", n), &n, |b, &n| {
            b.iter(|| {
                for &e in &entities {
                    let pos = world.get_mut::<Pos>(e);
                    black_box(pos);
                }
            });
        });

        // Insert (in-place, no migration).
        group.bench_with_input(BenchmarkId::new("insert_inplace", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                let entities: Vec<_> = (0..n)
                    .map(|_| {
                        let e = world.spawn();
                        world.insert(e, Pos(1.0, 2.0, 3.0));
                        world.insert(e, Health(100));
                        e
                    })
                    .collect();
                for &e in &entities {
                    world.insert(e, Pos(4.0, 5.0, 6.0));
                }
                black_box(world);
            });
        });

        // Is alive check.
        group.bench_with_input(BenchmarkId::new("is_alive", n), &n, |b, &n| {
            b.iter(|| {
                for &e in &entities {
                    let alive = world.is_alive(e);
                    black_box(alive);
                }
            });
        });
    }

    group.finish();
}

// =============================================================================
// 8. LARGE-SCALE BENCHMARKS
// =============================================================================
// Measures ECS performance at very large entity counts (100K-1M).
// Tests memory bandwidth and cache efficiency.
//
// What it measures:
//   - Memory allocation at scale
//   - Cache line efficiency of columnar storage
//   - Archetype scan overhead with many archetypes
//
// How to view:
//   - HTML report: target/criterion/large_scale/index.html
//   - These benchmarks take longer (60s each by default).
//   - Look at throughput to compare scaling behavior.
// =============================================================================

fn large_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("large_scale");

    group
        .measurement_time(Duration::from_secs(60));

    for &n in &[100_000, 500_000, 1_000_000] {
        let mut world = World::new();

        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
        }

        let archetypes = world.archetypes.len();
        println!("  large_scale n={}: {} archetypes", n, archetypes);

        group.throughput(Throughput::Elements(n as u64));

        // Spawn throughput.
        group.bench_with_input(BenchmarkId::new("spawn_only", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                for _ in 0..n {
                    black_box(world.spawn());
                }
                black_box(world);
            });
        });

        // Query throughput.
        group.bench_with_input(BenchmarkId::new("query", n), &n, |b, &n| {
            b.iter(|| {
                let mut world = World::new();
                for _ in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    world.insert(e, Health(100));
                }
                let mut count = 0u64;
                for (_e, (pos, health)) in world.query::<(&Pos, &Health)>() {
                    black_box((pos, health));
                    count += 1;
                }
                black_box(count);
            });
        });
    }

    group.finish();
}

// =============================================================================
// Criterion group and main
// =============================================================================

criterion_group!(
    benches,
    spawn_n,
    query_single,
    query_tuple_size,
    query_selectivity,
    archetype_migration,
    despawn,
    churn,
    archetype_pressure,
    component_access,
    large_scale,
);
criterion_main!(benches);
