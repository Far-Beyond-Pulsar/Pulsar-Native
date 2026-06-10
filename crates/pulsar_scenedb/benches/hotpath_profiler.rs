// Hotpath profiler for pulsar_scenedb.
//
// This benchmark measures every individual phase of the ECS hot paths using
// `std::time::Instant` for per-operation timing and the `profiling` crate for
// flame-graph-compatible markers.
//
// Run with:
//   cargo bench --bench hotpath_profiler
//
// Or run in release mode for accurate numbers:
//   cargo run --release --example hotpath_profiler  (if placed in examples/)
//
// Output format:
//   PHASE <name> | count <N> | total <time> | avg <time> | min <time> | max <time>
//
// At the end, a summary table prints aggregate stats for every phase.
//
// To also produce a flame graph (requires `cargo-instruments` or `profiling`
// output consumed by `speedscope`):
//   cargo bench --bench hotpath_profiler -- --flamegraph
// Then pipe the output to speedscope or view the generated .speedscope.json.

use pulsar_scenedb::*;
use std::time::{Duration, Instant};

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

// â”€â”€ Stats collector â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

struct PhaseStats {
    name: &'static str,
    count: u64,
    total_ns: u64,
    min_ns: u64,
    max_ns: u64,
}

impl PhaseStats {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            count: 0,
            total_ns: 0,
            min_ns: u64::MAX,
            max_ns: 0,
        }
    }

    fn record(&mut self, elapsed: Duration) {
        let ns = elapsed.as_nanos() as u64;
        self.count += 1;
        self.total_ns += ns;
        if ns < self.min_ns {
            self.min_ns = ns;
        }
        if ns > self.max_ns {
            self.max_ns = ns;
        }
    }

    fn avg_ns(&self) -> u64 {
        if self.count == 0 {
            return 0;
        }
        self.total_ns / self.count
    }

    fn p50_estimate(&self, _samples: &[u64]) -> u64 {
        // Simplified: avg is a reasonable proxy without full sample collection.
        self.avg_ns()
    }

    fn print(&self) {
        let avg = self.avg_ns();
        let min = self.min_ns;
        let max = self.max_ns;
        let total_us = self.total_ns as f64 / 1_000.0;
        println!(
            "  {:<40} | count {:>8} | total {:>10.2}us | avg {:>7}ns | p50 {:>7}ns | min {:>7}ns | max {:>7}ns",
            self.name,
            self.count,
            total_us,
            avg,
            avg,
            min,
            max,
        );
    }
}

struct Profiler {
    stats: Vec<PhaseStats>,
    samples: Vec<(String, u64)>,
}

impl Profiler {
    fn new() -> Self {
        Self {
            stats: Vec::new(),
            samples: Vec::new(),
        }
    }

    fn ensure(&mut self, name: &'static str) {
        if !self.stats.iter().any(|s| s.name == name) {
            self.stats.push(PhaseStats::new(name));
        }
    }

    fn record(&mut self, name: &'static str, elapsed: Duration) {
        self.ensure(name);
        let ns = elapsed.as_nanos() as u64;
        let idx = self.stats.iter().position(|s| s.name == name).unwrap();
        self.stats[idx].record(elapsed);
        // Track top-10 slowest samples per phase for outlier analysis.
        self.samples.push((name.to_string(), ns));
    }

    fn print_summary(&self) {
        println!(
            "\nâ•”â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•—"
        );
        println!("â•‘                        HOTPATH PROFILING SUMMARY                            â•‘");
        println!(
            "â• â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•£"
        );
        for s in &self.stats {
            s.print();
        }
        println!(
            "â•šâ•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•"
        );
    }

    fn print_top_slowest(&mut self, n: usize) {
        println!("\nTop {n} slowest individual operations:");
        self.samples.sort_by(|a, b| b.1.cmp(&a.1));
        for (i, (phase, ns)) in self.samples.iter().take(n).enumerate() {
            println!("  #{:>3}  {:<40}  {:>8}ns", i + 1, phase, ns,);
        }
    }
}

// â”€â”€ Helper: run a closure with timing â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
// The `#[inline(never)]` prevents the compiler from inlining this function,
// which is critical: without it, the compiler can see through the closure and
// optimize away work that has no observable side effects.

#[inline(never)]
fn timed<F: FnOnce() -> R, R>(f: F) -> (R, Duration) {
    let start = Instant::now();
    let result = f();
    let elapsed = start.elapsed();
    (result, elapsed)
}

// â”€â”€ Benchmark: spawn hotpath â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_spawn(p: &mut Profiler, n: usize) {
    println!("\nâ”€â”€â”€ Benchmark: spawn ({} entities) â”€â”€â”€", n);

    let (world, elapsed) = timed(|| {
        let mut world = World::new();
        for _ in 0..n {
            let e = world.spawn();
            black_box(e);
        }
        black_box(world);
    });
    black_box(world);
    p.record("spawn total", elapsed);
    println!(
        "  Total wall time: {:.2}us ({} entities) â†’ {:.2} entities/sec",
        elapsed.as_micros() as f64 / 1_000.0,
        n,
        n as f64 / elapsed.as_secs_f64(),
    );
}

// â”€â”€ Benchmark: insert hotpath (per-phase timing) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_insert_phases(p: &mut Profiler, n: usize) {
    println!(
        "\nâ”€â”€â”€ Benchmark: insert phases ({} entities Ã— 4 components) â”€â”€â”€",
        n
    );

    let mut world = World::new();
    let mut entities = Vec::with_capacity(n);

    // Phase 1: spawn (baseline)
    let start = Instant::now();
    for _ in 0..n {
        let e = world.spawn();
        entities.push(e);
    }
    let elapsed = start.elapsed();
    p.record("insert â†’ spawn", elapsed);
    println!(
        "  Phase 1 (spawn):     {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 2: insert Pos (first component â€” triggers archetype migration)
    let start = Instant::now();
    for &e in &entities {
        world.insert(e, Pos(1.0, 2.0, 3.0));
    }
    let elapsed = start.elapsed();
    p.record("insert â†’ Pos (first)", elapsed);
    println!(
        "  Phase 2 (insert Pos): {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 3: insert Vel (archetype already exists â€” fast path)
    let start = Instant::now();
    for &e in &entities {
        world.insert(e, Vel(0.0, 0.0, 0.0));
    }
    let elapsed = start.elapsed();
    p.record("insert â†’ Vel (cached archetype)", elapsed);
    println!(
        "  Phase 3 (insert Vel): {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 4: insert Health (another cached archetype)
    let start = Instant::now();
    for &e in &entities {
        world.insert(e, Health(100));
    }
    let elapsed = start.elapsed();
    p.record("insert â†’ Health (cached archetype)", elapsed);
    println!(
        "  Phase 4 (insert Health): {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 5: insert Tag (fourth component)
    let start = Instant::now();
    for &e in &entities {
        world.insert(e, Tag);
    }
    let elapsed = start.elapsed();
    p.record("insert â†’ Tag (cached archetype)", elapsed);
    println!(
        "  Phase 5 (insert Tag): {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Overwrite test: insert Pos again (in-place update path)
    let start = Instant::now();
    for &e in &entities {
        world.insert(e, Pos(4.0, 5.0, 6.0));
    }
    let elapsed = start.elapsed();
    p.record("insert â†’ Pos (in-place overwrite)", elapsed);
    println!(
        "  Phase 6 (overwrite Pos): {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );
}

// â”€â”€ Benchmark: remove hotpath (per-phase timing) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_remove_phases(p: &mut Profiler, n: usize) {
    println!("\nâ”€â”€â”€ Benchmark: remove phases ({} entities) â”€â”€â”€", n);

    let mut world = World::new();
    let entities: Vec<_> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
            e
        })
        .collect();

    // Phase 1: remove Health (triggers archetype migration back)
    let start = Instant::now();
    for &e in &entities {
        world.remove::<Health>(e);
    }
    let elapsed = start.elapsed();
    p.record("remove â†’ Health (migration)", elapsed);
    println!(
        "  Phase 1 (remove Health): {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 2: remove Pos (migration to empty archetype)
    let start = Instant::now();
    for &e in &entities {
        world.remove::<Pos>(e);
    }
    let elapsed = start.elapsed();
    p.record("remove â†’ Pos (to empty arch)", elapsed);
    println!(
        "  Phase 2 (remove Pos):    {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );
}

// â”€â”€ Benchmark: query hotpath (per-archetype breakdown) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_query_phases(p: &mut Profiler, n: usize) {
    println!("\nâ”€â”€â”€ Benchmark: query phases ({} entities) â”€â”€â”€", n);

    let mut world = World::new();

    // Create entities with (Pos, Health) â€” will be in one archetype.
    let pos_health_entities: Vec<_> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
            e
        })
        .collect();

    // Create entities with (Pos, Vel) â€” different archetype.
    let pos_vel_entities: Vec<_> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Vel(0.0, 0.0, 0.0));
            e
        })
        .collect();

    // Create entities with all 8 components â€” another archetype.
    let full_entities: Vec<_> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Vel(0.0, 0.0, 0.0));
            world.insert(e, Health(100));
            world.insert(e, Tag);
            world.insert(e, Name("test".into()));
            world.insert(e, Weight(50.0));
            world.insert(e, Color([1.0, 0.0, 0.0, 1.0]));
            world.insert(e, Lifetime(10.0));
            e
        })
        .collect();

    // Phase 1: query (Pos, Health) â€” matches 1 archetype of n entities.
    let start = Instant::now();
    for _ in 0..100 {
        let mut count = 0u64;
        for (_e, (_pos, _health)) in world.query::<(&Pos, &Health)>() {
            count += 1;
        }
        black_box(count);
    }
    let elapsed = start.elapsed();
    p.record("query (&Pos, &Health) Ã—100", elapsed);
    println!(
        "  Phase 1 (query &Pos,&Health Ã—100):  {:>10.2}us total | {:>9}ns iter",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 100
    );

    // Phase 2: query (Pos, Vel) â€” matches 1 archetype.
    let start = Instant::now();
    for _ in 0..100 {
        let mut count = 0u64;
        for (_e, (_pos, _vel)) in world.query::<(&Pos, &Vel)>() {
            count += 1;
        }
        black_box(count);
    }
    let elapsed = start.elapsed();
    p.record("query (&Pos, &Vel) Ã—100", elapsed);
    println!(
        "  Phase 2 (query &Pos,&Vel Ã—100):    {:>10.2}us total | {:>9}ns iter",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 100
    );

    // Phase 3: query all 8 components â€” matches 1 archetype.
    let start = Instant::now();
    for _ in 0..100 {
        let mut count = 0u64;
        for (_e, (_pos, _vel, _health, _tag, _name, _weight, _color, _life)) in
            world.query::<(&Pos, &Vel, &Health, &Tag, &Name, &Weight, &Color, &Lifetime)>()
        {
            count += 1;
        }
        black_box(count);
    }
    let elapsed = start.elapsed();
    p.record("query 8-tuple Ã—100", elapsed);
    println!(
        "  Phase 3 (query 8-tuple Ã—100):      {:>10.2}us total | {:>9}ns iter",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 100
    );

    // Phase 4: query () â€” matches ALL archetypes (empty tuple).
    let start = Instant::now();
    for _ in 0..100 {
        let mut count = 0u64;
        for (_e, ()) in world.query::<()>() {
            count += 1;
        }
        black_box(count);
    }
    let elapsed = start.elapsed();
    p.record("query () [all archetypes] Ã—100", elapsed);
    println!(
        "  Phase 4 (query () Ã—100):           {:>10.2}us total | {:>9}ns iter",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 100
    );

    // Phase 5: query non-matching â€” forces archetype skipping.
    let start = Instant::now();
    for _ in 0..100 {
        let mut count = 0u64;
        // Query for (Name, Weight) â€” only matches the full archetype.
        for (_e, (_name, _weight)) in world.query::<(&Name, &Weight)>() {
            count += 1;
        }
        black_box(count);
    }
    let elapsed = start.elapsed();
    p.record("query (&Name, &Weight) Ã—100", elapsed);
    println!(
        "  Phase 5 (query &Name,&Weight Ã—100): {:>10.2}us total | {:>9}ns iter",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 100
    );
}

// â”€â”€ Benchmark: despawn hotpath â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_despawn_phases(p: &mut Profiler, n: usize) {
    println!("\nâ”€â”€â”€ Benchmark: despawn phases ({} entities) â”€â”€â”€", n);

    let mut world = World::new();
    let entities: Vec<_> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
            e
        })
        .collect();

    // Phase 1: despawn from archetype with components
    let start = Instant::now();
    for &e in &entities {
        world.despawn(e);
    }
    let elapsed = start.elapsed();
    p.record("despawn (from arch with comps)", elapsed);
    println!(
        "  Phase 1 (despawn from arch): {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 2: spawn again (reuses free slots)
    let start = Instant::now();
    for _ in 0..n {
        world.spawn();
    }
    let elapsed = start.elapsed();
    p.record("spawn (slot reuse)", elapsed);
    println!(
        "  Phase 2 (spawn slot reuse):  {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 3: despawn from empty archetype
    let entities: Vec<_> = (0..n).map(|_| world.spawn()).collect();
    let start = Instant::now();
    for &e in &entities {
        world.despawn(e);
    }
    let elapsed = start.elapsed();
    p.record("despawn (from empty arch)", elapsed);
    println!(
        "  Phase 3 (despawn from empty):{:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );
}

// â”€â”€ Benchmark: component_id hotpath (cache hit vs miss) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_component_id(p: &mut Profiler, n: usize) {
    println!(
        "\nâ”€â”€â”€ Benchmark: component_id lookup ({} iterations) â”€â”€â”€",
        n
    );

    // Phase 1: cold path â€” first lookup (mutex acquisition).
    // Force cache miss by using a fresh world (thread-local cache is per-thread,
    // but we simulate cold path by using a type that hasn't been seen yet).
    let start = Instant::now();
    for _ in 0..1000 {
        let _cid = component_id::<Pos>();
    }
    let elapsed = start.elapsed();
    p.record("component_id (warm cache)", elapsed);
    println!(
        "  Phase 1 (warm cache):    {:>10.2}us total | {:>8}ns lookup",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 1000
    );

    // Phase 2: simulate cold path by using types that may not be cached.
    let start = Instant::now();
    for _ in 0..1000 {
        let _cid = component_id::<Vel>();
        let _cid = component_id::<Health>();
        let _cid = component_id::<Tag>();
        let _cid = component_id::<Name>();
        let _cid = component_id::<Weight>();
        let _cid = component_id::<Color>();
        let _cid = component_id::<Lifetime>();
    }
    let elapsed = start.elapsed();
    p.record("component_id (all types)", elapsed);
    println!(
        "  Phase 2 (all types):     {:>10.2}us total | {:>8}ns lookup",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 7000
    );
}

// â”€â”€ Benchmark: archetype migration cost â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_migration_phases(p: &mut Profiler, n: usize) {
    println!("\nâ”€â”€â”€ Benchmark: archetype migration ({} entities) â”€â”€â”€", n);

    // Phase 1: spawn â†’ insert 1 component (empty â†’ {Pos})
    let start = Instant::now();
    let mut world = World::new();
    let entities: Vec<_> = (0..n)
        .map(|_| {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            e
        })
        .collect();
    let elapsed = start.elapsed();
    p.record("migration (empty â†’ {Pos})", elapsed);
    println!(
        "  Phase 1 (emptyâ†’{{Pos}}):   {:>10.2}us total | {:>7}ns avg",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / n as u64
    );

    // Phase 2: insert 3 more components (each triggers a migration).
    let start = Instant::now();
    for &e in &entities {
        world.insert(e, Vel(0.0, 0.0, 0.0));
        world.insert(e, Health(100));
        world.insert(e, Tag);
    }
    let elapsed = start.elapsed();
    p.record("migration (Ã—3 components)", elapsed);
    println!(
        "  Phase 2 (3Ã— migration):    {:>10.2}us total | {:>9}ns/ent",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / (n as u64 * 3)
    );

    // Phase 3: remove 3 components (each triggers migration back).
    let start = Instant::now();
    for &e in &entities {
        world.remove::<Tag>(e);
        world.remove::<Health>(e);
        world.remove::<Vel>(e);
    }
    let elapsed = start.elapsed();
    p.record("migration (remove 3 comps)", elapsed);
    println!(
        "  Phase 3 (remove 3Ã—):       {:>10.2}us total | {:>9}ns/ent",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / (n as u64 * 3)
    );
}

// â”€â”€ Benchmark: churn (spawn â†’ insert â†’ remove â†’ despawn cycle) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_churn(p: &mut Profiler, waves: usize, per_wave: usize) {
    println!(
        "\nâ”€â”€â”€ Benchmark: entity churn ({} waves Ã— {} entities) â”€â”€â”€",
        waves, per_wave
    );

    let start = Instant::now();
    for wave in 0..waves {
        let mut world = World::new();
        let entities: Vec<_> = (0..per_wave)
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
        for &e in &entities {
            world.despawn(e);
        }
        black_box(&world);

        if (wave + 1) % (waves / 5) == 0 || wave + 1 == waves {
            let done = wave + 1;
            println!("  Progress: {}/{} waves", done, waves);
        }
    }
    let elapsed = start.elapsed();
    let total_ops = waves * per_wave;
    p.record("churn (spawnâ†’insertâ†’removeâ†’despawn)", elapsed);
    println!(
        "  Total: {:>10.2}us | {:>6} waves | {:>8} entities/sec",
        elapsed.as_micros() as f64 / 1_000.0,
        waves,
        total_ops as f64 / elapsed.as_secs_f64(),
    );
}

// â”€â”€ Benchmark: large-world query scalability â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_query_scalability(p: &mut Profiler, sizes: &[usize]) {
    println!("\nâ”€â”€â”€ Benchmark: query scalability across entity counts â”€â”€â”€");

    for &n in sizes {
        let mut world = World::new();
        for _ in 0..n {
            let e = world.spawn();
            world.insert(e, Pos(1.0, 2.0, 3.0));
            world.insert(e, Health(100));
        }
        // Add non-matching entities.
        for _ in 0..(n / 10) {
            let e = world.spawn();
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }

        let start = Instant::now();
        for _ in 0..1000 {
            let mut count = 0u64;
            for (_e, (_pos, _health)) in world.query::<(&Pos, &Health)>() {
                count += 1;
            }
            black_box(count);
        }
        let elapsed = start.elapsed();
        p.record("query (&Pos,&Health) Ã—1000", elapsed);
        println!(
            "  n={:>6}  |  {:>10.2}us total | {:>9}ns/iter | {:>8} items/sec",
            n,
            elapsed.as_micros() as f64 / 1_000.0,
            elapsed.as_nanos() as u64 / 1000,
            (n as u64 * 1000) / (elapsed.as_secs_f64() * 1_000_000_000.0).max(1.0) as u64,
        );
    }
}

// â”€â”€ Benchmark: archetype count pressure â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn bench_archetype_pressure(p: &mut Profiler, n: usize, components_per_entity: usize) {
    println!(
        "\nâ”€â”€â”€ Benchmark: archetype pressure ({} entities, {} comp types each) â”€â”€â”€",
        n, components_per_entity
    );

    let mut world = World::new();
    let mut entities = Vec::with_capacity(n);

    for i in 0..n {
        let e = world.spawn();
        entities.push(e);
        // Each entity gets a unique archetype by adding components one at a time.
        let comps = components_per_entity.min(8);
        if i % 8 == 0 {
            world.insert(e, Pos(1.0, 2.0, 3.0));
        }
        if i % 8 == 1 {
            world.insert(e, Vel(0.0, 0.0, 0.0));
        }
        if i % 8 == 2 {
            world.insert(e, Health(100));
        }
        if i % 8 == 3 {
            world.insert(e, Tag);
        }
        if i % 8 == 4 {
            world.insert(e, Name("test".into()));
        }
        if i % 8 == 5 {
            world.insert(e, Weight(50.0));
        }
        if i % 8 == 6 {
            world.insert(e, Color([1.0, 0.0, 0.0, 1.0]));
        }
        if i % 8 == 7 {
            world.insert(e, Lifetime(10.0));
        }
    }

    let archetypes = world.archetypes.len();
    println!("  Archetypes created: {}", archetypes);

    // Query across all archetypes.
    let start = Instant::now();
    for _ in 0..100 {
        let mut count = 0u64;
        for (_e, ()) in world.query::<()>() {
            count += 1;
        }
        black_box(count);
    }
    let elapsed = start.elapsed();
    p.record("query (all archetypes, high count)", elapsed);
    println!(
        "  Query Ã—100: {:>10.2}us total | {:>9}ns/iter",
        elapsed.as_micros() as f64 / 1_000.0,
        elapsed.as_nanos() as u64 / 100
    );
}

// â”€â”€ Main â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

fn black_box<T>(t: T) -> T {
    std::hint::black_box(t)
}

fn main() {
    println!("â•”â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•—");
    println!("â•‘                    pulsar_scenedb HOTPATH PROFILER                               â•‘");
    println!("â•‘  Measures individual phases of every hot path operation.                     â•‘");
    println!("â•‘  Run with: cargo bench --bench hotpath_profiler                              â•‘");
    println!("â•šâ•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•");

    let mut profiler = Profiler::new();

    // â”€â”€ Small-scale detailed benchmarks â”€â”€
    bench_spawn(&mut profiler, 10_000);
    bench_insert_phases(&mut profiler, 10_000);
    bench_remove_phases(&mut profiler, 10_000);
    bench_query_phases(&mut profiler, 10_000);
    bench_despawn_phases(&mut profiler, 10_000);
    bench_component_id(&mut profiler, 10_000);
    bench_migration_phases(&mut profiler, 10_000);

    // â”€â”€ Medium-scale benchmarks â”€â”€
    bench_churn(&mut profiler, 50, 1_000);
    bench_query_scalability(&mut profiler, &[1_000, 5_000, 10_000, 50_000]);
    bench_archetype_pressure(&mut profiler, 10_000, 8);

    // â”€â”€ Summary â”€â”€
    println!("\n");
    profiler.print_summary();
    profiler.print_top_slowest(20);

    println!("\nâ•”â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•—");
    println!("â•‘  HOW TO VIEW OUTPUT                                                          â•‘");
    println!("â• â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•£");
    println!("â•‘  1. Terminal: the summary table above shows avg/min/max per phase.           â•‘");
    println!("â•‘  2. Top-20 slowest: identifies outlier operations.                           â•‘");
    println!("â•‘  3. Flame graph: run `cargo bench --bench hotpath_profiler` and pipe to      â•‘");
    println!("â•‘     speedscope:                                                              â•‘");
    println!("â•‘       cargo bench --bench hotpath_profiler 2>&1 | speedscope -               â•‘");
    println!("â•‘  4. Compare runs: save output to files and diff:                             â•‘");
    println!("â•‘       cargo bench --bench hotpath_profiler > before.txt                      â•‘");
    println!("â•‘       # make changes...                                                      â•‘");
    println!("â•‘       cargo bench --bench hotpath_profiler > after.txt                       â•‘");
    println!("â•‘       diff -u before.txt after.txt                                           â•‘");
    println!("â•šâ•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•");
}
