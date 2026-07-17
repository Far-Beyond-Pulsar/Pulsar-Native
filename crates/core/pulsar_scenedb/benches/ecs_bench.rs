use criterion::{black_box, criterion_group, criterion_main, Criterion};
use pulsar_scenedb::*;

// â”€â”€ Component types â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
struct Pos(f32, f32, f32);
struct Vel(f32, f32, f32);
struct Health(u32);
struct Tag;
struct Name(String);
struct Weight(f64);
struct Color([f32; 4]);
struct Lifetime(f32);

fn spawn_n(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn");
    for &n in &[100, 1_000, 10_000, 50_000] {
        group.throughput(criterion::Throughput::Elements(n as u64));
        group.bench_with_input(criterion::BenchmarkId::new("empty", n), &n, |b, _| {
            b.iter(|| {
                let mut world = World::new();
                world.reserve_entities(n);
                for _ in 0..n {
                    black_box(world.spawn());
                }
            });
        });
        group.bench_with_input(criterion::BenchmarkId::new("with_component", n), &n, |b, _| {
            b.iter(|| {
                let mut world = World::new();
                world.reserve_entities(n);
                for _ in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                }
            });
        });
        group.bench_with_input(criterion::BenchmarkId::new("with_4_components", n), &n, |b, _| {
            b.iter(|| {
                let mut world = World::new();
                world.reserve_entities(n);
                for _ in 0..n {
                    let e = world.spawn();
                    world.insert(e, Pos(1.0, 2.0, 3.0));
                    world.insert(e, Vel(0.0, 0.0, 0.0));
                    world.insert(e, Health(100));
                    world.insert(e, Tag);
                }
            });
        });
    }
    group.finish();
}

fn query_single(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_single");
    for &n in &[100, 1_000, 10_000, 50_000] {
        let mut world = World::new();
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
        }
        // Add some non-matching entities
        for _ in 0..(n / 10) {
            let e = world.spawn();
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }
        group.throughput(criterion::Throughput::Elements(n as u64));
        group.bench_with_input(criterion::BenchmarkId::new("iter", n), &n, |b, _| {
            b.iter(|| {
                for (_entity, (pos, health)) in world.query::<(&Pos, &Health)>() {
                    black_box((pos, health));
                }
            });
        });
    }
    group.finish();
}

fn query_tuple_8(c: &mut Criterion) {
    let n = 10_000;
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
    let mut group = c.benchmark_group("query_8_tuple");
    group.throughput(criterion::Throughput::Elements(n));
    group.bench_function("iter", |b| {
        b.iter(|| {
            for (_entity, (pos, vel, health, _tag, name, weight, color, lifetime)) in
                world.query::<(&Pos, &Vel, &Health, &Tag, &Name, &Weight, &Color, &Lifetime)>()
            {
                black_box((pos, vel, health, name, weight, color, lifetime));
            }
        });
    });
    group.finish();
}

fn archetype_migration(c: &mut Criterion) {
    let n = 10_000;
    let mut group = c.benchmark_group("archetype_migration");
    group.bench_function("insert_component", |b| {
        b.iter(|| {
            let mut world = World::new();
            let entities: Vec<_> = (0..n).map(|_| world.spawn()).collect();
            for &e in &entities {
                world.insert(e, Pos(1.0, 2.0, 3.0));
            }
        });
    });
    group.bench_function("add_then_remove", |b| {
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
        });
    });
    group.finish();
}

criterion_group!(benches, spawn_n, query_single, query_tuple_8, archetype_migration);
criterion_main!(benches);
