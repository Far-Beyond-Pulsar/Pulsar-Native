//! Conformance of generated Rust against the interpreter.
//!
//! The same module is run twice, interpreted and as the Rust
//! `pulsar_script_codegen` generated for it (built by `build.rs` in the
//! profile under test, so run this in debug and release), through the same
//! driver, natives, host and fake clock. The two runs produce a trace of
//! everything observable: each call's result or error with its call-stack
//! trace and source locations, every waiting call and when it resumes,
//! every instance's variables after every step, and every native call.
//! [`harness::run`] returns it; the tests assert the two traces are equal.

// Linked for its value-type and native registrations.
use pulsar_script_math as _;

pub mod fixtures;

/// The generated Rust for every fixture module.
#[allow(unused_imports)]
pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

pub mod harness {
    use std::sync::{Arc, Mutex};

    use pulsar_scenedb::{Entity, World};
    use pulsar_script_vm::{
        Budget, CapabilityPolicy, Completion, Continuation, Host, Instance, Module, NativeFn,
        NativeRegistry, Param, Program, ScriptError, Signature, Type, TypeRegistry, Value, Vm,
    };

    /// The limits a run applies, to both backends.
    #[derive(Clone, Copy, Debug)]
    pub struct Limits {
        pub budget: u64,
        pub max_depth: usize,
        /// Integer overflow is an error rather than wrapping.
        pub checked: bool,
    }

    impl Default for Limits {
        fn default() -> Self {
            Self {
                budget: 100_000,
                max_depth: 64,
                checked: false,
            }
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Backend {
        Interpreted,
        Generated,
    }

    #[derive(Clone, Debug)]
    pub enum Step {
        /// Call `function` of instance `instance`.
        Call {
            instance: usize,
            function: &'static str,
            args: Vec<Value>,
        },
        /// Move the fake clock on, resuming every waiting call that is due.
        Advance(f64),
    }

    #[derive(Clone, Debug)]
    pub struct Scenario {
        pub module: Module,
        pub instances: usize,
        pub limits: Limits,
        pub steps: Vec<Step>,
    }

    impl Scenario {
        pub fn new(module: Module, steps: Vec<Step>) -> Self {
            Self {
                module,
                instances: 1,
                limits: Limits::default(),
                steps,
            }
        }

        pub fn instances(mut self, instances: usize) -> Self {
            self.instances = instances;
            self
        }

        pub fn limits(mut self, limits: Limits) -> Self {
            self.limits = limits;
            self
        }
    }

    pub fn call(function: &'static str, args: Vec<Value>) -> Step {
        Step::Call {
            instance: 0,
            function,
            args,
        }
    }

    pub fn call_on(instance: usize, function: &'static str, args: Vec<Value>) -> Step {
        Step::Call {
            instance,
            function,
            args,
        }
    }

    type Log = Arc<Mutex<Vec<String>>>;

    /// The natives every run links against: the engine's (standard library,
    /// math types, ..) plus test natives that record into `log`.
    pub fn registry(log: &Log) -> NativeRegistry {
        let mut registry = NativeRegistry::with_engine_natives();
        let mut add = |native: NativeFn| {
            registry
                .register(native)
                .expect("test native names are unique")
        };

        let recorder = Arc::clone(log);
        add(NativeFn::builder("test::note").params(["text", "n"]).build(
            move |text: Arc<str>, n: i64| {
                recorder
                    .lock()
                    .unwrap()
                    .push(format!("note({text:?}, {n})"));
            },
        ));
        let recorder = Arc::clone(log);
        add(
            NativeFn::builder("test::fail").build(move || -> Result<(), String> {
                recorder.lock().unwrap().push("fail()".into());
                Err("deliberate failure".into())
            }),
        );
        let recorder = Arc::clone(log);
        add(NativeFn::builder("test::boom").build(move || -> () {
            recorder.lock().unwrap().push("boom()".into());
            panic!("deliberate panic");
        }));
        add(NativeFn::builder("test::liar").build_raw(
            Signature::new([], Type::Int),
            Box::new(|_, _| Ok(Value::Str("not an int".into()))),
        ));
        let recorder = Arc::clone(log);
        add(NativeFn::builder("test::bump").build_raw(
            Signature::new([Param::inout(Type::Int)], Type::Unit),
            Box::new(move |_, args| {
                let Value::Int(n) = &mut args[0] else {
                    unreachable!("checked by the VM")
                };
                *n += 1;
                recorder.lock().unwrap().push(format!("bump -> {n}"));
                Ok(Value::Unit)
            }),
        ));
        add(NativeFn::builder("test::twice")
            .pure()
            .build(|n: i64| n.wrapping_mul(2)));

        // The natives the Blueprint fixtures import (see `blueprint_compiler`'s
        // `conformance_fixtures`).
        add(NativeFn::builder("std::add")
            .pure()
            .params(["a", "b"])
            .build(|a: i64, b: i64| a.wrapping_add(b)));
        add(NativeFn::builder("std::append")
            .pure()
            .params(["a", "b"])
            .build(|a: String, b: String| a + &b));
        add(NativeFn::builder("std::less")
            .pure()
            .params(["a", "b"])
            .build(|a: i64, b: i64| a < b));
        add(NativeFn::builder("std::roll").build(|| 4i64));
        add(NativeFn::builder("std::to_int")
            .pure()
            .params(["x"])
            .build(|x: f64| x.round() as i64));
        add(NativeFn::builder("std::pick")
            .attr("exec_outputs", "X,Y,Z")
            .params(["n", "result"])
            .build_raw(
                Signature::new([Param::new(Type::Int), Param::inout(Type::Int)], Type::Int),
                Box::new(|_, args| {
                    let n = args[0].as_int().unwrap();
                    args[1] = Value::Int(n * 10);
                    Ok(Value::Int(n % 3))
                }),
            ));
        registry
    }

    /// A value as the trace shows it: objects through their literal form
    /// (the VM's own `Debug` hides their contents).
    pub fn show(value: &Value) -> String {
        match value {
            Value::Object(object) => {
                let literal = TypeRegistry::global()
                    .encode_value(object)
                    .unwrap_or_else(|e| e);
                format!("{}{literal}", object.type_name())
            }
            other => format!("{other:?}"),
        }
    }

    fn show_error(error: &ScriptError) -> String {
        format!(
            "{:?} trace={:?} locations={:?}",
            error.kind, error.trace, error.locations
        )
    }

    struct Instances {
        states: Vec<Instance>,
        entities: Vec<Entity>,
        /// Waiting calls: `(instance, wake time, sequence, continuation)`.
        waiting: Vec<(usize, f64, u64, Continuation)>,
        sequence: u64,
    }

    /// Run `scenario` on `backend`; the trace is one line per observation.
    pub fn run(scenario: &Scenario, backend: Backend) -> Vec<String> {
        let log: Log = Arc::default();
        let registry = registry(&log);
        let program = match backend {
            Backend::Interpreted => Program::link(Arc::new(scenario.module.clone()), &registry),
            Backend::Generated => crate::generated::link(
                &scenario.module.name,
                &registry,
                None,
                &CapabilityPolicy::allow_all(),
            )
            .unwrap_or_else(|| panic!("no generated code for fixture `{}`", scenario.module.name)),
        }
        .expect("the fixture links");

        let mut trace = vec![format!(
            "program: functions={:?} variables={:?} subscriptions={:?}",
            scenario
                .module
                .functions
                .iter()
                .map(|f| (&f.name, f.exported))
                .collect::<Vec<_>>(),
            scenario
                .module
                .variables
                .iter()
                .map(|v| &v.name)
                .collect::<Vec<_>>(),
            program.subscriptions(),
        )];
        let mut world = World::new();
        let entities: Vec<Entity> = (0..scenario.instances).map(|_| world.spawn()).collect();
        let mut instances = Instances {
            states: (0..scenario.instances)
                .map(|_| program.instantiate())
                .collect(),
            entities,
            waiting: Vec::new(),
            sequence: 0,
        };
        let mut vm = Vm::new();
        vm.max_depth = scenario.limits.max_depth;
        vm.checked_arithmetic = scenario.limits.checked;
        let mut now = 0.0f64;

        for step in &scenario.steps {
            match step {
                Step::Call {
                    instance,
                    function,
                    args,
                } => {
                    let func = program
                        .entry(function)
                        .unwrap_or_else(|| panic!("no exported function `{function}`"));
                    let mut host = Host::at_time(&mut world, instances.entities[*instance], now);
                    let result = vm.start(
                        &program,
                        &mut instances.states[*instance],
                        func,
                        args,
                        &mut host,
                        &mut Budget::new(scenario.limits.budget),
                    );
                    let shown_args = args.iter().map(show).collect::<Vec<_>>().join(", ");
                    trace.push(format!(
                        "call #{instance} {function}({shown_args}) -> {}",
                        outcome(&mut instances, *instance, now, result)
                    ));
                }
                Step::Advance(seconds) => {
                    now += seconds;
                    trace.push(format!("advance {seconds} -> t={now}"));
                    // Resume everything that is due, earliest first, in the
                    // order the calls started waiting.
                    loop {
                        let due = instances
                            .waiting
                            .iter()
                            .enumerate()
                            .filter(|(_, (_, wake, _, _))| *wake <= now)
                            .min_by(|(_, (_, wa, sa, _)), (_, (_, wb, sb, _))| {
                                wa.partial_cmp(wb).unwrap().then(sa.cmp(sb))
                            })
                            .map(|(index, _)| index);
                        let Some(index) = due else { break };
                        let (instance, wake, _, continuation) = instances.waiting.remove(index);
                        let names = continuation.functions().join(">");
                        let mut host = Host::at_time(&mut world, instances.entities[instance], now);
                        let result = vm.resume(
                            &program,
                            &mut instances.states[instance],
                            continuation,
                            &mut host,
                            &mut Budget::new(scenario.limits.budget),
                        );
                        trace.push(format!(
                            "resume #{instance} {names} (due {wake}) -> {}",
                            outcome(&mut instances, instance, now, result)
                        ));
                    }
                }
            }
            for (index, state) in instances.states.iter().enumerate() {
                let vars = scenario
                    .module
                    .variables
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        format!(
                            "{}={}",
                            v.name,
                            show(program.var(state, i).expect("variable"))
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                trace.push(format!("vars #{index}: {vars}"));
            }
            let mut waiting: Vec<_> = instances
                .waiting
                .iter()
                .map(|(i, wake, _, c)| (*i, *wake, c.functions().join(">")))
                .collect();
            waiting.sort_by(|a, b| a.partial_cmp(b).unwrap());
            trace.push(format!("waiting: {waiting:?}"));
            trace.push(format!("natives: {:?}", log.lock().unwrap()));
        }
        trace
    }

    fn outcome(
        instances: &mut Instances,
        instance: usize,
        now: f64,
        result: Result<Completion, ScriptError>,
    ) -> String {
        match result {
            Ok(Completion::Returned(value)) => format!("returned {}", show(&value)),
            Ok(Completion::Waiting {
                seconds,
                continuation,
            }) => {
                instances.sequence += 1;
                let names = continuation.functions().join(">");
                instances
                    .waiting
                    .push((instance, now + seconds, instances.sequence, continuation));
                format!("waiting {seconds}s in {names}")
            }
            Ok(Completion::Paused { snapshot, .. }) => {
                let frame = snapshot
                    .call_stack
                    .last()
                    .map(|frame| format!("{}@{}", frame.function, frame.pc))
                    .unwrap_or_default();
                format!("paused at {frame}")
            }
            Err(error) => format!("error {}", show_error(&error)),
        }
    }
}
