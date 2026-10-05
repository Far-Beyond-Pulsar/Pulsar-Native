//! Runtime support for classes exported as Rust (`events.rs`).
//!
//! The Blueprint editor's Rust export (`pulsar_script_codegen`) turns a
//! compiled class into an [`Actor`](pulsar_scenedb::Actor): the class's
//! functions become generated Rust that runs on the same VM as an
//! interpreted class (same instances, same natives, same instruction
//! budget, call-depth limit and waiting), and [`ExportedScript`] is the
//! part of the actor that is not generated: the instance's variables, its
//! waiting calls, and the lifecycle (`begin_play`, `tick`, `end_play`).
//!
//! # Time
//!
//! The pinned `Actor::tick` is deliberately time-free, so an exported actor
//! reads time from a [`ScriptClock`]. The default is wall-clock seconds
//! since the actor was created, which differs from the interpreted path's
//! game time in one respect: a paused or slowed game does not pause or slow
//! an exported actor's waits. Hosts that want game time can supply their
//! own clock ([`ExportedScript::with_clock`]).
//!
//! # Not supported
//!
//! Classes that declare or subscribe to custom events cannot be exported:
//! delivery is the engine event hub's job, and an `Actor` has no way to
//! subscribe. The exporter refuses such a class rather than silently
//! dropping its handlers.

use std::sync::Arc;
use std::time::Instant;

use pulsar_scenedb::{Entity, World};
use pulsar_script_runtime::DEFAULT_BUDGET;
pub use pulsar_script_vm as vm;
use pulsar_script_vm::{Budget, Completion, Continuation, Host, Instance, Program, Type, Value, Vm};

/// Seconds, from any fixed origin, that never go backwards.
pub trait ScriptClock: Send + Sync {
    fn now(&self) -> f64;
}

/// Wall-clock seconds since creation.
pub struct WallClock(Instant);

impl Default for WallClock {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl ScriptClock for WallClock {
    fn now(&self) -> f64 {
        self.0.elapsed().as_secs_f64()
    }
}

/// The per-actor state of an exported class.
pub struct ExportedScript {
    class: &'static str,
    instance: Instance,
    vm: Vm,
    waiting: Vec<(f64, Continuation)>,
    paused: Vec<(pulsar_script_vm::DebugSnapshot, Continuation)>,
    clock: Arc<dyn ScriptClock>,
    last_tick: Option<f64>,
}

impl ExportedScript {
    /// A fresh instance of the class `program` was linked from, on the
    /// wall clock.
    pub fn new(class: &'static str, program: &Program) -> Self {
        Self::with_clock(class, program, Arc::new(WallClock::default()))
    }

    pub fn with_clock(class: &'static str, program: &Program, clock: Arc<dyn ScriptClock>) -> Self {
        Self { class, instance: program.instantiate(), vm: Vm::new(), waiting: Vec::new(), paused: Vec::new(), clock, last_tick: None }
    }

    /// Number of suspended calls.
    pub fn waiting_calls(&self) -> usize {
        self.waiting.len()
    }

    /// Debugger-stopped calls retained until the host issues a debugger command.
    pub fn paused_calls(&self) -> &[(pulsar_script_vm::DebugSnapshot, Continuation)] {
        &self.paused
    }

    /// The value of variable `name`.
    pub fn variable<'a>(&'a self, program: &Program, name: &str) -> Option<&'a Value> {
        program.var(&self.instance, program.variable(name)?)
    }

    pub fn begin_play(&mut self, program: &Program, entity: Entity, world: &mut World) {
        self.call(program, "begin_play", &[], entity, world);
    }

    pub fn end_play(&mut self, program: &Program, entity: Entity, world: &mut World) {
        self.call(program, "end_play", &[], entity, world);
        self.waiting.clear();
    }

    /// Resume the waiting calls that are due, then run the class's `tick`
    /// (if it has one) with the time since the last tick.
    pub fn tick(&mut self, program: &Program, entity: Entity, world: &mut World) {
        let now = self.clock.now();
        let delta = self.last_tick.map_or(0.0, |last| (now - last).max(0.0));
        self.last_tick = Some(now);
        self.resume_due(program, now, entity, world);
        if program.module().function("tick").is_some_and(|(_, f)| f.params == [Type::Float]) {
            self.call(program, "tick", &[Value::Float(delta)], entity, world);
        }
    }

    /// Call exported function `name`; a class without it is not an error
    /// (a Blueprint need not handle every lifecycle event). Script errors
    /// are logged with the class and call stack, not propagated: actor
    /// callbacks cannot return them.
    pub fn call(&mut self, program: &Program, name: &str, args: &[Value], entity: Entity, world: &mut World) {
        let Some(func) = program.entry(name) else { return };
        let now = self.clock.now();
        let mut host = Host::at_time(world, entity, now);
        let result = self.vm.start(program, &mut self.instance, func, args, &mut host, &mut Budget::new(DEFAULT_BUDGET));
        self.settle(result, now);
    }

    fn resume_due(&mut self, program: &Program, now: f64, entity: Entity, world: &mut World) {
        // Earliest first; a call that waits again is parked for a later tick
        // (never resumed twice in one tick, so a zero-second wait yields).
        let mut due: Vec<(f64, Continuation)> = Vec::new();
        let mut index = 0;
        while index < self.waiting.len() {
            if self.waiting[index].0 <= now {
                due.push(self.waiting.remove(index));
            } else {
                index += 1;
            }
        }
        due.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, continuation) in due {
            let mut host = Host::at_time(world, entity, now);
            let result = self.vm.resume(program, &mut self.instance, continuation, &mut host, &mut Budget::new(DEFAULT_BUDGET));
            self.settle(result, now);
        }
    }

    fn settle(&mut self, result: Result<Completion, pulsar_script_vm::ScriptError>, now: f64) {
        match result {
            Ok(Completion::Returned(_)) => {}
            Ok(Completion::Waiting { seconds, continuation }) => self.waiting.push((now + seconds, continuation)),
            Ok(Completion::Paused { snapshot, continuation }) => self.paused.push((snapshot, continuation)),
            Err(error) => tracing::error!(class = self.class, "script error: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use pulsar_script_vm::{BinOp, Constant, Function, Instr, Module, NativeRegistry};

    use super::*;

    /// A clock the test moves by hand.
    #[derive(Default)]
    struct FakeClock(AtomicU64);

    impl FakeClock {
        fn set(&self, seconds: f64) {
            self.0.store(seconds.to_bits(), Ordering::SeqCst);
        }
    }

    impl ScriptClock for FakeClock {
        fn now(&self) -> f64 {
            f64::from_bits(self.0.load(Ordering::SeqCst))
        }
    }

    fn counter() -> Module {
        let mut m = Module::new("Counter");
        m.variables = vec![
            pulsar_script_vm::Variable { name: "beats".into(), ty: Type::Int, default: None, id: None },
            pulsar_script_vm::Variable { name: "waited".into(), ty: Type::Int, default: None, id: None },
            pulsar_script_vm::Variable { name: "dt".into(), ty: Type::Float, default: None, id: None },
        ];
        m.constants = vec![Constant::Int(1), Constant::Float(2.0)];
        let func = |name: &str, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>| {
            let mut registers = params.clone();
            registers.extend(extra);
            Function { name: name.into(), exported: true, params, ret: Type::Unit, registers, code, debug: None }
        };
        // begin_play: waited += 1 after a 2s wait
        m.functions.push(func(
            "begin_play",
            vec![],
            vec![Type::Float, Type::Int, Type::Int],
            vec![
                Instr::Const { dst: 0, index: 1 },
                Instr::Wait { seconds: 0 },
                Instr::LoadVar { dst: 1, var: 1 },
                Instr::Const { dst: 2, index: 0 },
                Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
                Instr::StoreVar { var: 1, src: 1 },
                Instr::Return { value: None },
            ],
        ));
        // tick(dt): beats += 1; dt = dt
        m.functions.push(func(
            "tick",
            vec![Type::Float],
            vec![Type::Int, Type::Int],
            vec![
                Instr::LoadVar { dst: 1, var: 0 },
                Instr::Const { dst: 2, index: 0 },
                Instr::Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
                Instr::StoreVar { var: 0, src: 1 },
                Instr::StoreVar { var: 2, src: 0 },
                Instr::Return { value: None },
            ],
        ));
        m
    }

    #[test]
    fn lifecycle_waits_and_tick_follow_the_clock() {
        let program = Program::link(Arc::new(counter()), &NativeRegistry::with_engine_natives()).unwrap();
        let clock = Arc::new(FakeClock::default());
        let mut script = ExportedScript::with_clock("Counter", &program, clock.clone());
        let mut world = World::new();
        let entity = world.spawn();

        script.begin_play(&program, entity, &mut world);
        assert_eq!(script.waiting_calls(), 1, "begin_play is waiting for two seconds");

        clock.set(1.0);
        script.tick(&program, entity, &mut world);
        assert_eq!(script.variable(&program, "waited"), Some(&Value::Int(0)), "not due yet");
        assert_eq!(script.variable(&program, "beats"), Some(&Value::Int(1)));

        clock.set(2.5);
        script.tick(&program, entity, &mut world);
        assert_eq!(script.waiting_calls(), 0);
        assert_eq!(script.variable(&program, "waited"), Some(&Value::Int(1)), "resumed once the clock passed the wake time");
        assert_eq!(script.variable(&program, "dt"), Some(&Value::Float(1.5)), "tick receives the time since the last tick");

        script.end_play(&program, entity, &mut world);
        assert_eq!(script.waiting_calls(), 0);
    }

    #[test]
    fn instances_of_one_program_do_not_share_state() {
        let program = Program::link(Arc::new(counter()), &NativeRegistry::with_engine_natives()).unwrap();
        let clock = Arc::new(FakeClock::default());
        let mut a = ExportedScript::with_clock("Counter", &program, clock.clone());
        let mut b = ExportedScript::with_clock("Counter", &program, clock);
        let mut world = World::new();
        let (ea, eb) = (world.spawn(), world.spawn());
        for _ in 0..3 {
            a.tick(&program, ea, &mut world);
        }
        b.tick(&program, eb, &mut world);
        assert_eq!(a.variable(&program, "beats"), Some(&Value::Int(3)));
        assert_eq!(b.variable(&program, "beats"), Some(&Value::Int(1)), "per-instance state, not process-global");
    }

    #[test]
    fn a_missing_lifecycle_function_is_not_an_error_and_errors_do_not_escape() {
        let mut module = counter();
        module.functions.retain(|f| f.name != "begin_play");
        let program = Program::link(Arc::new(module), &NativeRegistry::with_engine_natives()).unwrap();
        let mut script = ExportedScript::new("Counter", &program);
        let mut world = World::new();
        let entity = world.spawn();
        script.begin_play(&program, entity, &mut world); // no begin_play: nothing happens
        script.call(&program, "nope", &[], entity, &mut world);
        // A bad call (wrong argument count) is logged, not raised.
        script.call(&program, "tick", &[], entity, &mut world);
        assert_eq!(script.variable(&program, "beats"), Some(&Value::Int(0)));
    }
}
