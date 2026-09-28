//! Heap usage must not grow over ticks.
//!
//! This test binary installs a counting global allocator: every live heap
//! block and byte in the process is counted, whichever crate allocated it.
//! Each scenario runs a real [`TickLoop`] past a warm-up (so caches, queues
//! and collections reach their steady capacity), then samples the live heap
//! every [`WINDOW`] ticks. Between the first and the last sample the loop
//! runs `(SAMPLES - 1) * WINDOW` ticks; a leak of even one block per few
//! thousand ticks, or of a byte per few dozen ticks, shows up as growth
//! beyond the tolerance.
//!
//! Everything runs in one `#[test]` so no other test thread allocates while
//! a scenario is measured; every scenario runs, and the test fails listing
//! each one that grew.

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::Path;
use std::sync::atomic::{AtomicIsize, Ordering};

use engine_backend::scene::{RuntimeLevel, SceneWorldExt};
use pulsar_class::ClassRegistry;
use pulsar_game::scripting::scripting_config_path;
use pulsar_game::tick::TickLoop;
use pulsar_game::TickMode;
use pulsar_script_vm::{
    BinOp, Constant, EventDecl, EventField, EventRef, Function, Import, Instr, Module, Param,
    Signature, Subscription, SubscriptionScope, Type, UnOp, Variable,
};
use serde_json::json;

// ---- the counting allocator -------------------------------------------------

static LIVE_BYTES: AtomicIsize = AtomicIsize::new(0);
static LIVE_BLOCKS: AtomicIsize = AtomicIsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
            LIVE_BLOCKS.fetch_add(1, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            LIVE_BYTES.fetch_add(layout.size() as isize, Ordering::Relaxed);
            LIVE_BLOCKS.fetch_add(1, Ordering::Relaxed);
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
        LIVE_BYTES.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        LIVE_BLOCKS.fetch_sub(1, Ordering::Relaxed);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new = System.realloc(ptr, layout, new_size);
        if !new.is_null() {
            LIVE_BYTES.fetch_add(new_size as isize - layout.size() as isize, Ordering::Relaxed);
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Clone, Copy, Debug)]
struct Heap {
    bytes: isize,
    blocks: isize,
}

fn heap() -> Heap {
    Heap { bytes: LIVE_BYTES.load(Ordering::SeqCst), blocks: LIVE_BLOCKS.load(Ordering::SeqCst) }
}

// ---- measurement --------------------------------------------------------------

/// Ticks before the first sample, for capacities to settle.
const WARMUP: u32 = 3_000;
/// Warm-up for scenarios that change `ClassInstance` every tick. The script
/// driver reads that component through a SceneDB change journal: a ring
/// that grows until it holds `DEFAULT_JOURNAL_CAPACITY` entries (about
/// 1 MiB) and then evicts the oldest, so memory is bounded but only flat
/// once the ring is full. Churn records at least one entry per tick.
const JOURNAL_WARMUP: u32 = pulsar_scenedb::change_journal::DEFAULT_JOURNAL_CAPACITY as u32 + WARMUP;
/// Ticks between samples.
const WINDOW: u32 = 1_000;
const SAMPLES: usize = 10;
/// Growth allowed between the first and last sample. Instance ids of
/// runtime spawns (`<Class>_rt<n>`) legitimately gain a digit at 10^k, a
/// few bytes in total; a real leak over 9 000 ticks is far above this.
const TOLERANCE_BYTES: isize = 256;
const TOLERANCE_BLOCKS: isize = 2;

struct Outcome {
    name: &'static str,
    samples: Vec<Heap>,
    /// Why the scenario did not do the work it measures, if it didn't.
    idle: Option<String>,
}

impl Outcome {
    fn growth(&self) -> Heap {
        let (first, last) = (self.samples[0], self.samples[self.samples.len() - 1]);
        Heap { bytes: last.bytes - first.bytes, blocks: last.blocks - first.blocks }
    }

    /// The scenario ran its scripts as intended.
    fn check(mut self, game: &TickLoop, work: impl FnOnce(&TickLoop) -> Result<(), String>) -> Self {
        let stats = game.script_stats();
        let mut problems = Vec::new();
        if stats.script_errors > 0 || stats.load_errors > 0 {
            problems.push(format!("{} script errors, {} load errors", stats.script_errors, stats.load_errors));
        }
        if let Err(why) = work(game) {
            problems.push(why);
        }
        if !problems.is_empty() {
            self.idle = Some(problems.join("; "));
        }
        self
    }

    fn leaked(&self) -> bool {
        let g = self.growth();
        g.bytes > TOLERANCE_BYTES || g.blocks > TOLERANCE_BLOCKS
    }

    fn report(&self) -> String {
        let g = self.growth();
        let ticks = (SAMPLES as u32 - 1) * WINDOW;
        let bytes: Vec<String> = self.samples.iter().map(|s| s.bytes.to_string()).collect();
        let blocks: Vec<String> = self.samples.iter().map(|s| s.blocks.to_string()).collect();
        let idle = self.idle.as_ref().map(|why| format!(" [DID NOT RUN AS INTENDED: {why}]")).unwrap_or_default();
        format!(
            "{}{idle}: {:+} bytes / {:+} blocks over {ticks} ticks ({:.3} bytes/tick)\n  live bytes:  {}\n  live blocks: {}",
            self.name,
            g.bytes,
            g.blocks,
            g.bytes as f64 / f64::from(ticks),
            bytes.join(" "),
            blocks.join(" "),
        )
    }
}

fn measure(name: &'static str, game: &mut TickLoop, each_tick: impl FnMut(&mut TickLoop)) -> Outcome {
    measure_after(WARMUP, name, game, each_tick)
}

fn measure_after(
    warmup: u32,
    name: &'static str,
    game: &mut TickLoop,
    mut each_tick: impl FnMut(&mut TickLoop),
) -> Outcome {
    let mut tick = |game: &mut TickLoop| {
        each_tick(game);
        game.tick_once();
    };
    for _ in 0..warmup {
        tick(game);
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    samples.push(heap());
    for _ in 1..SAMPLES {
        for _ in 0..WINDOW {
            tick(game);
        }
        samples.push(heap());
    }
    Outcome { name, samples, idle: None }
}

// ---- class modules ------------------------------------------------------------

fn function(name: &str, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>) -> Function {
    let mut registers = params.clone();
    registers.extend(extra);
    Function { name: name.into(), exported: true, params, ret: Type::Unit, registers, code, debug: None }
}

fn import(name: &str, params: Vec<Type>, ret: Type) -> Import {
    Import { name: name.into(), sig: Signature::new(params.into_iter().map(Param::new), ret) }
}

fn var(name: &str, ty: Type) -> Variable {
    Variable { name: name.into(), ty, default: None }
}

/// `tick(dt)`: `total += dt; label = "t=" + to_str(total)` (a fresh string
/// every tick, replacing the last one).
fn ticker_module() -> Module {
    let mut m = Module::new("Ticker");
    m.variables = vec![var("total", Type::Float), var("label", Type::Str)];
    m.constants = vec![Constant::Str("t=".into())];
    m.functions = vec![function("tick", vec![Type::Float], vec![Type::Float, Type::Str, Type::Str], vec![
        Instr::LoadVar { dst: 1, var: 0 },
        Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 0 },
        Instr::StoreVar { var: 0, src: 1 },
        Instr::Unary { op: UnOp::ToStr, dst: 2, src: 1 },
        Instr::Const { dst: 3, index: 0 },
        Instr::Binary { op: BinOp::Add, dst: 3, a: 3, b: 2 },
        Instr::StoreVar { var: 1, src: 3 },
        Instr::Return { value: None },
    ])];
    m
}

/// `begin_play`: `me = self`. Spawned and destroyed by `Churner`.
fn minion_module() -> Module {
    let mut m = Module::new("Minion");
    m.variables = vec![var("me", Type::Entity)];
    m.functions = vec![
        function("begin_play", vec![], vec![Type::Entity], vec![
            Instr::SelfEntity { dst: 0 },
            Instr::StoreVar { var: 0, src: 0 },
            Instr::Return { value: None },
        ]),
        function("end_play", vec![], vec![], vec![Instr::Return { value: None }]),
    ];
    m
}

/// A global script. `tick`: destroy last tick's `Minion`, spawn a new one.
fn churner_module() -> Module {
    let mut m = Module::new("Churner");
    m.variables = vec![var("last", Type::Entity)];
    m.constants = vec![Constant::Str("Minion".into()), Constant::Float(0.0)];
    m.imports = vec![
        import("world::spawn", vec![Type::Str, Type::Float, Type::Float, Type::Float], Type::Entity),
        import("world::destroy", vec![Type::Entity], Type::Unit),
    ];
    m.functions = vec![function("tick", vec![Type::Float], vec![Type::Entity, Type::Str, Type::Float], vec![
        Instr::LoadVar { dst: 1, var: 0 },
        Instr::CallNative { import: 1, args: vec![1], dst: None },
        Instr::Const { dst: 2, index: 0 },
        Instr::Const { dst: 3, index: 1 },
        Instr::CallNative { import: 0, args: vec![2, 3, 3, 3], dst: Some(1) },
        Instr::StoreVar { var: 0, src: 1 },
        Instr::Return { value: None },
    ])];
    m
}

/// `begin_play`: `loop { wait 0; frames += 1 }`, which suspends every tick
/// forever (how a Blueprint While loop runs).
fn waiter_module() -> Module {
    let mut m = Module::new("Waiter");
    m.variables = vec![var("frames", Type::Int)];
    m.constants = vec![Constant::Float(0.0), Constant::Int(1)];
    m.functions = vec![function("begin_play", vec![], vec![Type::Float, Type::Int, Type::Int], vec![
        Instr::Const { dst: 0, index: 0 },
        Instr::Wait { seconds: 0 },
        Instr::LoadVar { dst: 1, var: 0 },
        Instr::Const { dst: 2, index: 1 },
        Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
        Instr::StoreVar { var: 0, src: 1 },
        Instr::Jump { target: 1 },
    ])];
    m
}

/// Declares `Listener.Ping(amount)`. `tick` emits it to the `Listener`
/// class; handlers count pings and `KeyDown`s (published by the host every
/// tick).
fn listener_module() -> Module {
    let mut m = Module::new("Listener");
    m.variables = vec![var("pings", Type::Int), var("keys", Type::Int)];
    m.constants = vec![Constant::Str("Listener".into()), Constant::Str("Listener.Ping".into()), Constant::Int(1)];
    m.imports = vec![import("event::emit_to_class", vec![Type::Str, Type::Str, Type::Int], Type::Unit)];
    let count = |name: &str, var: u32| {
        let mut f = function(name, vec![Type::Int], vec![Type::Int, Type::Int], vec![
            Instr::LoadVar { dst: 1, var },
            Instr::Const { dst: 2, index: 2 },
            Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
            Instr::StoreVar { var, src: 1 },
            Instr::Return { value: None },
        ]);
        f.exported = false;
        f
    };
    m.functions = vec![
        function("tick", vec![Type::Float], vec![Type::Str, Type::Str, Type::Int], vec![
            Instr::Const { dst: 1, index: 0 },
            Instr::Const { dst: 2, index: 1 },
            Instr::Const { dst: 3, index: 2 },
            Instr::CallNative { import: 0, args: vec![1, 2, 3], dst: None },
            Instr::Return { value: None },
        ]),
        count("on_ping", 0),
        count("on_key", 1),
    ];
    m.events = vec![EventDecl { name: "Listener.Ping".into(), fields: vec![EventField::new("amount", Type::Int)] }];
    m.subscriptions = vec![
        Subscription { event: EventRef::Name("Listener.Ping".into()), handler: 1, scope: SubscriptionScope::Class },
        Subscription { event: EventRef::Name("KeyDown".into()), handler: 2, scope: SubscriptionScope::Global },
    ];
    m
}

/// `tick`: `LightComponent::of(self).set_enabled(random_bool())`, the
/// graph "on_tick -> Set Enabled(light, random_bool)".
fn light_toggler_module() -> Module {
    let mut m = Module::new("LightToggler");
    m.imports = vec![
        import("LightComponent::of", vec![Type::Entity], Type::Component("LightComponent".into())),
        import("std::random_bool", vec![], Type::Bool),
        import(
            "LightComponent::set_enabled",
            vec![Type::Component("LightComponent".into()), Type::Bool],
            Type::Unit,
        ),
    ];
    m.functions = vec![function(
        "tick",
        vec![Type::Float],
        vec![Type::Entity, Type::Component("LightComponent".into()), Type::Bool],
        vec![
            Instr::SelfEntity { dst: 1 },
            Instr::CallNative { import: 0, args: vec![1], dst: Some(2) },
            Instr::CallNative { import: 1, args: vec![], dst: Some(3) },
            Instr::CallNative { import: 2, args: vec![2, 3], dst: None },
            Instr::Return { value: None },
        ],
    )];
    m
}

// ---- project fixtures ------------------------------------------------------------

fn write_class(root: &Path, name: &str, module: Module) -> String {
    let guid = format!("{}-guid", name.to_lowercase());
    let dir = root.join("src").join("classes").join(name);
    std::fs::create_dir_all(dir.join("events").join(".build")).unwrap();
    std::fs::write(dir.join("class.json"), json!({ "class_id": guid }).to_string()).unwrap();
    std::fs::write(dir.join("events/.build/module.json"), module.to_json().unwrap()).unwrap();
    guid
}

/// A level with `count` objects of class `(guid, name)`.
fn write_level(root: &Path, guid: &str, class: &str, count: usize) -> std::path::PathBuf {
    let mut components = serde_json::Map::new();
    let objects: Vec<serde_json::Value> = (0..count)
        .map(|i| {
            let id = format!("{class}_{i}");
            components.insert(
                id.clone(),
                json!([{ "class_name": "ClassInstance", "enabled": true,
                         "data": { "class": guid, "class_name": class } }]),
            );
            json!({
                "id": id, "name": id, "object_type": "Empty", "parent": null,
                "visible": true, "locked": false, "props": {},
                "transform": { "position": [i as f32, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] }
            })
        })
        .collect();
    let path = root.join("test.level");
    std::fs::write(&path, json!({ "version": "2.1", "objects": objects, "components": components }).to_string())
        .unwrap();
    path
}

/// The standalone startup path: scripting on the loop's own world, then
/// the level loaded into it.
fn standalone(root: &Path, level: Option<&Path>) -> TickLoop {
    let mut game = TickLoop::new(TickMode::default(), 0);
    game.enable_scripting(root);
    if let Some(level) = level {
        let registry = ClassRegistry::scan(root);
        let mut store = game.scene_store.write();
        RuntimeLevel::load_into_with_classes(level, &mut store.world, &registry).unwrap();
    }
    game
}

/// Play-in-Editor: the game adopts an already loaded world.
fn pie(root: &Path, level: &Path) -> TickLoop {
    let editor = RuntimeLevel::load_with_classes(level, &ClassRegistry::scan(root)).unwrap();
    let mut game = TickLoop::with_scene_store(editor.scene(), TickMode::default(), 0);
    game.enable_scripting(root);
    game
}

fn global_scripts(root: &Path, names: &[&str]) {
    let path = scripting_config_path(root);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, json!({ "global_scripts": names }).to_string()).unwrap();
}

/// Variable `var` of every live instance of `class`.
fn vars(game: &TickLoop, class: &str, var: &str) -> Vec<pulsar_script_vm::Value> {
    let driver = game.scripts.as_ref().expect("scripting enabled").lock().unwrap();
    let runtime = driver.runtime();
    runtime
        .instance_ids()
        .iter()
        .filter(|id| runtime.class_of(id) == Some(class))
        .filter_map(|id| runtime.variable(id, var).cloned())
        .collect()
}

fn ticks_run() -> i64 {
    i64::from(WARMUP + (SAMPLES as u32 - 1) * WINDOW)
}

// ---- scenarios ---------------------------------------------------------------------

fn idle() -> Outcome {
    let mut game = TickLoop::new(TickMode::default(), 0);
    measure("idle tick loop", &mut game, |_| {})
}

fn scripted_ticks() -> Outcome {
    let project = tempfile::tempdir().unwrap();
    let guid = write_class(project.path(), "Ticker", ticker_module());
    let level = write_level(project.path(), &guid, "Ticker", 50);
    let mut game = standalone(project.path(), Some(&level));
    measure("50 instances with a tick handler building strings", &mut game, |_| {}).check(&game, |game| {
        let labels = vars(game, "Ticker", "label");
        match labels.first() {
            Some(pulsar_script_vm::Value::Str(label)) if labels.len() == 50 && label.starts_with("t=") => Ok(()),
            other => Err(format!("{} labels, first {other:?}", labels.len())),
        }
    })
}

fn spawn_destroy_churn() -> Outcome {
    let project = tempfile::tempdir().unwrap();
    write_class(project.path(), "Minion", minion_module());
    write_class(project.path(), "Churner", churner_module());
    global_scripts(project.path(), &["Churner"]);
    let mut game = standalone(project.path(), None);
    measure_after(JOURNAL_WARMUP, "spawn one class instance and destroy the last every tick", &mut game, |_| {}).check(&game, |game| {
        let stats = game.script_stats();
        let expected = (ticks_run() + i64::from(JOURNAL_WARMUP - WARMUP)) as u64;
        // Every tick spawns one Minion and destroys the previous one.
        if stats.spawned + 1 >= expected && stats.destroyed + 2 >= expected && stats.started >= expected {
            Ok(())
        } else {
            Err(format!("spawned {} destroyed {} started {} in {expected} ticks", stats.spawned, stats.destroyed, stats.started))
        }
    })
}

fn latent_waits() -> Outcome {
    let project = tempfile::tempdir().unwrap();
    let guid = write_class(project.path(), "Waiter", waiter_module());
    let level = write_level(project.path(), &guid, "Waiter", 50);
    let mut game = standalone(project.path(), Some(&level));
    measure("50 instances suspended in a per-frame wait loop", &mut game, |_| {}).check(&game, |game| {
        let frames = vars(game, "Waiter", "frames");
        let all_ran = frames.len() == 50
            && frames.iter().all(|f| matches!(f, pulsar_script_vm::Value::Int(n) if *n + 2 >= ticks_run()));
        if all_ran { Ok(()) } else { Err(format!("frames {:?}", frames.first())) }
    })
}

fn events() -> Outcome {
    let project = tempfile::tempdir().unwrap();
    let guid = write_class(project.path(), "Listener", listener_module());
    let level = write_level(project.path(), &guid, "Listener", 20);
    let mut game = standalone(project.path(), Some(&level));
    measure("20 instances emitting and handling events, a KeyDown every tick", &mut game, |game| {
        game.publish_input(pulsar_events::builtin::KeyDown { key: 7 });
    })
    .check(&game, |game| {
        // Each instance gets 20 class pings a tick (one per sender) and a key.
        let (pings, keys) = (vars(game, "Listener", "pings"), vars(game, "Listener", "keys"));
        let ok = |values: &[pulsar_script_vm::Value], per_tick: i64| {
            values.len() == 20
                && values.iter().all(|v| matches!(v, pulsar_script_vm::Value::Int(n) if *n + 3 * per_tick >= per_tick * ticks_run()))
        };
        if ok(&pings, 20) && ok(&keys, 1) { Ok(()) } else { Err(format!("pings {:?} keys {:?}", pings.first(), keys.first())) }
    })
}

fn light_toggling() -> Outcome {
    let project = tempfile::tempdir().unwrap();
    let guid = write_class(project.path(), "LightToggler", light_toggler_module());
    let level = write_level(project.path(), &guid, "LightToggler", 20);
    let mut game = standalone(project.path(), Some(&level));
    {
        let mut store = game.scene_store.write();
        let entities: Vec<_> = (0..20)
            .map(|i| store.world.entity_for(&format!("LightToggler_{i}")).expect("placed"))
            .collect();
        for entity in entities {
            store.world.insert(entity, pulsar_game::scene::LightComponent::default());
        }
    }
    let mut toggles = 0u32;
    let mut last: Option<Vec<bool>> = None;
    let outcome = measure("20 instances setting their light's enabled flag every tick", &mut game, |game| {
        // Watch the flags change between ticks (random, so almost always).
        let store = game.scene_store.read();
        let now: Vec<bool> = (0..20)
            .filter_map(|i| store.world.entity_for(&format!("LightToggler_{i}")))
            .filter_map(|e| store.world.get::<pulsar_game::scene::LightComponent>(e).map(|l| l.general.enabled))
            .collect();
        if last.as_ref().is_some_and(|last| *last != now) {
            toggles += 1;
        }
        last = Some(now);
    });
    outcome.check(&game, |_| {
        if toggles * 2 > WARMUP { Ok(()) } else { Err(format!("the lights changed on only {toggles} ticks")) }
    })
}

fn pie_everything() -> Outcome {
    let project = tempfile::tempdir().unwrap();
    let guid = write_class(project.path(), "Ticker", ticker_module());
    write_class(project.path(), "Minion", minion_module());
    write_class(project.path(), "Churner", churner_module());
    write_class(project.path(), "Waiter", waiter_module());
    global_scripts(project.path(), &["Churner", "Waiter"]);
    let level = write_level(project.path(), &guid, "Ticker", 20);
    let mut game = pie(project.path(), &level);
    measure_after(JOURNAL_WARMUP, "Play-in-Editor path: ticks, churn and waits together", &mut game, |game| {
        game.publish_input(pulsar_events::builtin::KeyDown { key: 7 });
    })
    .check(&game, |game| {
        let (labels, frames) = (vars(game, "Ticker", "label"), vars(game, "Waiter", "frames"));
        let spawned = game.script_stats().spawned;
        if labels.len() == 20 && frames.len() == 1 && spawned as i64 + 1 >= ticks_run() + i64::from(JOURNAL_WARMUP - WARMUP) {
            Ok(())
        } else {
            Err(format!("{} tickers, {} waiters, {spawned} spawns", labels.len(), frames.len()))
        }
    })
}

/// Negative control: the measurement flags a leak of one byte every
/// thousand ticks (9 blocks over the measured window).
fn control_leak() -> Outcome {
    let mut game = TickLoop::new(TickMode::default(), 0);
    measure("control: leaks one byte every 1000 ticks", &mut game, |game| {
        if game.ticks() % 1000 == 0 {
            Box::leak(Box::new(0u8));
        }
    })
}

#[test]
fn heap_does_not_grow_over_ticks() {
    let control = control_leak();
    assert!(control.leaked(), "the measurement must catch a slow leak:\n{}", control.report());
    let outcomes = [idle(), scripted_ticks(), spawn_destroy_churn(), latent_waits(), events(), light_toggling(), pie_everything()];
    let report: Vec<String> = outcomes.iter().map(Outcome::report).collect();
    println!("{}", report.join("\n"));
    let leaks: Vec<String> =
        outcomes.iter().filter(|o| o.leaked() || o.idle.is_some()).map(Outcome::report).collect();
    assert!(
        leaks.is_empty(),
        "heap grew over ticks, or a scenario did not run its scripts (tolerance {TOLERANCE_BYTES} bytes / {TOLERANCE_BLOCKS} blocks):\n{}",
        leaks.join("\n")
    );
}
