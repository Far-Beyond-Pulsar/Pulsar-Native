//! A Blueprint and a TypeScript version of one class behave the same.
//!
//! The Blueprint is `bp_beacon` (compiled by `blueprint_compiler`, checked in
//! under `pulsar_script_conformance/fixtures`); the TypeScript is written
//! here. Both run the same scenario on the same VM, with the same fake
//! clock, and everything a game could observe is compared: what each call
//! returns or whether it waits (and for how long), the `log` variable after
//! every step, and the waiting calls.

use std::sync::{Arc, Mutex};

use pulsar_scenedb::{Entity, World};
use pulsar_script_conformance::harness::registry;
use pulsar_script_ts::{compile_class, ClassSource};
use pulsar_script_vm::{Budget, Completion, Continuation, Host, Instance, Module, Program, Value, Vm};

const BEACON_TS: &str = r#"
export default class Beacon extends ScriptClass {
    log: string = "";
    done: boolean = false;      // do_once
    flip: boolean = false;      // flip_flop
    runs: int = 0;              // do_n
    pending: boolean = false;   // delay

    async on_fire(): Promise<void> {
        if (!this.done) { this.done = true; this.log += "o"; }
        if (this.flip) { this.log += "a"; } else { this.log += "b"; }
        this.flip = !this.flip;
        if (this.runs < 2) { this.runs += 1; this.log += "n"; }
        if (!this.pending) {
            this.pending = true;
            await wait(0.5);
            this.pending = false;
            this.log += "d";
        }
    }
}
"#;

fn typescript() -> Module {
    let compiled = compile_class(
        &ClassSource { class_name: "bp_beacon", file: "class.ts", source: &BEACON_TS.replace("Beacon", "bp_beacon"), schema: None },
        &registry(&Arc::default()),
    );
    assert!(compiled.diagnostics.is_empty(), "{:#?}", compiled.diagnostics);
    compiled.module.expect("compiles")
}

fn blueprint() -> Module {
    let path = format!("{}/../pulsar_script_conformance/fixtures/bp_beacon.module.json", env!("CARGO_MANIFEST_DIR"));
    Module::from_json(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).expect("fixture parses")
}

enum Step {
    Fire,
    Advance(f64),
}

/// Run the scenario; one line per observation.
fn observe(module: Module, steps: &[Step]) -> Vec<String> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let program = Program::link(Arc::new(module), &registry(&log)).expect("links");
    let mut instance: Instance = program.instantiate();
    let mut world = World::new();
    let entity: Entity = world.spawn();
    let mut vm = Vm::new();
    let mut now = 0.0;
    let mut waiting: Vec<(f64, Continuation)> = Vec::new();
    let mut seen = Vec::new();
    let log_var = program.variable("log").expect("both classes have a `log`");
    let fire = program.entry("on_fire").expect("on_fire");

    let mut note = |what: String, program: &Program, instance: &Instance, waiting: &[(f64, Continuation)]| {
        let log = program.var(instance, log_var).cloned().unwrap_or(Value::Unit);
        let waits: Vec<f64> = waiting.iter().map(|(wake, _)| *wake).collect();
        seen.push(format!("{what} | log={log:?} | waiting={waits:?}"));
    };
    let settle = |result: Result<Completion, pulsar_script_vm::ScriptError>, now: f64, waiting: &mut Vec<(f64, Continuation)>| match result {
        Ok(Completion::Returned(_)) => "returned".to_owned(),
        Ok(Completion::Waiting { seconds, continuation }) => {
            waiting.push((now + seconds, continuation));
            format!("waiting {seconds}s")
        }
        Ok(Completion::Paused { .. }) => "paused".to_owned(),
        Err(error) => format!("error {}", error.kind),
    };

    for step in steps {
        match step {
            Step::Fire => {
                let mut host = Host::at_time(&mut world, entity, now);
                let result = vm.start(&program, &mut instance, fire, &[], &mut host, &mut Budget::new(10_000));
                let what = format!("fire@{now}: {}", settle(result, now, &mut waiting));
                note(what, &program, &instance, &waiting);
            }
            Step::Advance(seconds) => {
                now += seconds;
                waiting.sort_by(|a, b| a.0.total_cmp(&b.0));
                let due = waiting.iter().take_while(|(wake, _)| *wake <= now).count();
                let mut outcomes = Vec::new();
                for (_, continuation) in waiting.drain(..due).collect::<Vec<_>>() {
                    let mut host = Host::at_time(&mut world, entity, now);
                    let result = vm.resume(&program, &mut instance, continuation, &mut host, &mut Budget::new(10_000));
                    outcomes.push(settle(result, now, &mut waiting));
                }
                note(format!("advance to {now}: resumed {outcomes:?}"), &program, &instance, &waiting);
            }
        }
    }
    seen
}

#[test]
fn the_blueprint_and_the_typescript_version_behave_identically() {
    let steps = [
        Step::Fire,
        Step::Advance(0.25),
        Step::Fire, // the delay is still counting down: ignored
        Step::Advance(0.25),
        Step::Advance(1.0),
        Step::Fire,
        Step::Advance(0.5),
        Step::Fire, // do_n is spent
        Step::Fire,
        Step::Advance(2.0),
    ];
    let blueprint = observe(blueprint(), &steps);
    let typescript = observe(typescript(), &steps);
    assert_eq!(blueprint, typescript, "the languages diverge:\n--- blueprint\n{}\n--- typescript\n{}", blueprint.join("\n"), typescript.join("\n"));

    // The scenario really exercises the nodes (not two empty traces).
    assert!(blueprint[0].contains("waiting 0.5s") && blueprint[0].contains("log=Str(\"obn\")") || blueprint[0].contains("\"obn\""), "{}", blueprint[0]);
    assert!(blueprint.iter().any(|l| l.contains("resumed [\"returned\"]")), "a wait completed");
    assert!(blueprint.last().unwrap().contains('d'));
}
