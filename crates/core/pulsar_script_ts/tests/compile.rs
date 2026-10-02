//! TypeScript classes compiled and run on the script VM.

use std::sync::Arc;

use pulsar_scenedb::World;
use pulsar_script_ts::{compile_class, declarations, ClassSchema, ClassSource, Compiled, DeclaredField};
use pulsar_script_vm::{
    Budget, Completion, Continuation, Host, Instance, Module, NativeRegistry, Program, ScriptError, Value, Vm,
};

use pulsar_script_math as _;

fn registry() -> NativeRegistry {
    NativeRegistry::with_engine_natives()
}

fn compile(source: &str) -> Compiled {
    compile_class(&ClassSource { class_name: "Test", file: "class.ts", source, schema: None }, &registry())
}

fn module(source: &str) -> Module {
    let compiled = compile(source);
    assert!(compiled.diagnostics.is_empty(), "unexpected diagnostics: {:#?}", compiled.diagnostics);
    compiled.module.expect("a module")
}

fn errors(source: &str) -> Vec<String> {
    let compiled = compile(source);
    assert!(compiled.module.is_none(), "this source must not compile");
    compiled.diagnostics.iter().map(ToString::to_string).collect()
}

/// A class instance on a world, driven by hand.
struct Run {
    program: Program,
    instance: Instance,
    vm: Vm,
    world: World,
    entity: pulsar_scenedb::Entity,
    time: f64,
}

impl Run {
    fn new(module: Module) -> Self {
        let program = Program::link(Arc::new(module), &registry()).expect("links");
        let instance = program.instantiate();
        let mut world = World::new();
        let entity = world.spawn();
        Self { program, instance, vm: Vm::new(), world, entity, time: 0.0 }
    }

    fn start(&mut self, name: &str, args: &[Value]) -> Result<Completion, ScriptError> {
        let func = self.program.entry(name).unwrap_or_else(|| panic!("no exported `{name}`"));
        let mut host = Host::at_time(&mut self.world, self.entity, self.time);
        self.vm.start(&self.program, &mut self.instance, func, args, &mut host, &mut Budget::new(100_000))
    }

    fn call(&mut self, name: &str, args: &[Value]) -> Value {
        match self.start(name, args).unwrap_or_else(|e| panic!("{name} failed: {e}")) {
            Completion::Returned(value) => value,
            Completion::Waiting { .. } => panic!("{name} waited"),
        }
    }

    fn resume(&mut self, continuation: Continuation) -> Result<Completion, ScriptError> {
        let mut host = Host::at_time(&mut self.world, self.entity, self.time);
        self.vm.resume(&self.program, &mut self.instance, continuation, &mut host, &mut Budget::new(100_000))
    }

    fn var(&self, name: &str) -> Value {
        self.program.var(&self.instance, self.program.variable(name).expect("variable")).expect("value").clone()
    }
}

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn float(f: f64) -> Value {
    Value::Float(f)
}

// ---- behaviour ----------------------------------------------------------------

#[test]
fn fields_arithmetic_loops_and_calls_run_correctly() {
    let mut run = Run::new(module(
        r#"
        export default class Test extends ScriptClass {
            total: int = 0;
            scale: number = 1.5;

            sum_to(n: int): int {
                let acc = 0;
                for (let i = 1; i <= n; i++) {
                    if (i % 2 == 0) { continue; }
                    acc += i;
                }
                this.total = acc;
                return acc;
            }

            fib(n: int): int {
                if (n < 2) { return n; }
                return this.fib(n - 1) + this.fib(n - 2);
            }

            scaled(x: number): number { return x * this.scale + 2; }

            first_over(limit: int): int {
                let i = 0;
                while (true) {
                    i += 1;
                    if (i * i > limit) { break; }
                }
                return i;
            }

            classify(x: int): string {
                return x < 0 ? "negative" : x == 0 ? "zero" : "positive";
            }

            both(a: boolean, b: boolean): boolean { return a && !b || b && !a; }
        }
        "#,
    ));
    assert_eq!(run.call("sum_to", &[int(9)]), int(25));
    assert_eq!(run.var("total"), int(25));
    assert_eq!(run.call("fib", &[int(12)]), int(144));
    assert_eq!(run.call("scaled", &[float(4.0)]), float(8.0));
    assert_eq!(run.call("first_over", &[int(50)]), int(8));
    assert_eq!(run.call("classify", &[int(-3)]), Value::from("negative"));
    assert_eq!(run.call("classify", &[int(0)]), Value::from("zero"));
    assert_eq!(run.call("classify", &[int(7)]), Value::from("positive"));
    assert_eq!(run.call("both", &[Value::Bool(true), Value::Bool(false)]), Value::Bool(true));
    assert_eq!(run.call("both", &[Value::Bool(true), Value::Bool(true)]), Value::Bool(false));
}

#[test]
fn logical_operators_short_circuit() {
    let mut run = Run::new(module(
        r#"
        export default class Test {
            calls: int = 0;
            bump(): boolean { this.calls += 1; return true; }
            and_false(): boolean { return false && this.bump(); }
            or_true(): boolean { return true || this.bump(); }
            and_true(): boolean { return true && this.bump(); }
        }
        "#,
    ));
    assert_eq!(run.call("and_false", &[]), Value::Bool(false));
    assert_eq!(run.call("or_true", &[]), Value::Bool(true));
    assert_eq!(run.var("calls"), int(0), "the right side did not run");
    assert_eq!(run.call("and_true", &[]), Value::Bool(true));
    assert_eq!(run.var("calls"), int(1));
}

#[test]
fn numbers_are_ints_or_floats_by_syntax_and_context_and_convert_explicitly() {
    let mut run = Run::new(module(
        r#"
        export default class Test {
            half(x: int): number { return (x as number) / 2; }
            truncated(x: number): int { return x as int; }
            labelled(x: int): string { return "n=" + (x as string); }
            literal_as_float(): number { let x: number = 5; return x * 2; }
            mixed_literal(x: number): number { return x * 2 + 1; }
            int_math(): int { return 7 / 2; }
        }
        "#,
    ));
    assert_eq!(run.call("half", &[int(7)]), float(3.5));
    assert_eq!(run.call("truncated", &[float(-2.9)]), int(-2));
    assert_eq!(run.call("labelled", &[int(4)]), Value::from("n=4"));
    assert_eq!(run.call("literal_as_float", &[]), float(10.0));
    assert_eq!(run.call("mixed_literal", &[float(1.5)]), float(4.0));
    assert_eq!(run.call("int_math", &[]), int(3), "integer division");
}

#[test]
fn async_methods_wait_in_game_time_and_nested_waits_resume() {
    let mut run = Run::new(module(
        r#"
        export default class Test {
            count: int = 0;
            async delayed(): Promise<void> {
                this.count += 1;
                await wait(1.5);
                this.count += 10;
                await this.inner();
                this.count += 1000;
            }
            async inner(): Promise<void> { await wait(0.5); this.count += 100; }
        }
        "#,
    ));
    let Completion::Waiting { seconds, continuation } = run.start("delayed", &[]).unwrap() else { panic!("should wait") };
    assert_eq!(seconds, 1.5);
    assert_eq!(run.var("count"), int(1));
    run.time = 1.5;
    let Completion::Waiting { seconds, continuation } = run.resume(continuation).unwrap() else { panic!("waits again, in inner") };
    assert_eq!(seconds, 0.5);
    assert_eq!(continuation.functions(), ["delayed", "inner"]);
    run.time = 2.0;
    assert!(matches!(run.resume(continuation).unwrap(), Completion::Returned(_)));
    assert_eq!(run.var("count"), int(1111));
}

#[test]
fn natives_value_types_and_properties_work() {
    let mut run = Run::new(module(
        r#"
        export default class Test {
            position: Vec3 = Vec3.new_(1, 2, 3);
            hits: int = 0;

            length_after_move(): number {
                this.position = this.position.add(Vec3.new_(0, 4, 0));
                return this.position.length();
            }
            height(): number { return this.position.y; }
            root(x: number): number { return math.sqrt(x); }
            name_length(s: string): int { return string_.len(s); }
            me(): Entity { return this.entity; }
            clock(): number { return this.time; }
        }
        "#,
    ));
    assert_eq!(run.call("height", &[]), float(2.0));
    let Value::Float(length) = run.call("length_after_move", &[]) else { panic!() };
    assert!((length - (1.0f64 + 36.0 + 9.0).sqrt()).abs() < 1e-5, "{length}");
    assert_eq!(run.call("height", &[]), float(6.0), "the field was updated with value semantics");
    assert_eq!(run.call("root", &[float(16.0)]), float(4.0));
    assert_eq!(run.call("name_length", &[Value::from("héllo")]), int(5));
    assert_eq!(run.call("me", &[]), Value::Entity(run.entity));
    run.time = 3.25;
    assert_eq!(run.call("clock", &[]), float(3.25));
}

#[test]
fn runtime_errors_report_the_typescript_line_and_column() {
    let mut run = Run::new(module(
        "export default class Test {\n    divide(a: int, b: int): int {\n        return a / b;\n    }\n}\n",
    ));
    let error = run.start("divide", &[int(1), int(0)]).unwrap_err();
    let location = error.location().expect("a source location");
    assert_eq!((location.file.as_str(), location.line, location.column), ("class.ts", Some(3), Some(16)), "{error}");
}

#[test]
fn compound_assignment_and_updates_follow_javascript() {
    let mut run = Run::new(module(
        r#"
        export default class Test {
            x: int = 5;
            ops(): int {
                let a = 10;
                a -= 3; a *= 2; a %= 5;
                const post = this.x++;
                const pre = ++this.x;
                return a * 1000 + post * 100 + pre * 10 + this.x;
            }
        }
        "#,
    ));
    // a: 10 -> 7 -> 14 -> 4; post = 5 (x=6); pre = 7 (x=7)
    assert_eq!(run.call("ops", &[]), int(4000 + 500 + 70 + 7));
}

// ---- diagnostics -----------------------------------------------------------------

#[test]
fn unsupported_constructs_are_rejected_with_a_position_and_a_reason() {
    for (source, needle) in [
        ("import x from 'y';\nexport default class Test {}", "imports are not supported"),
        ("export default class Test { f(): void { const g = () => 1; } }", "this expression is not supported"),
        ("export default class Test { f(): string { return `a${1}`; } }", "template literals"),
        ("export default class Test { f(x: int): void { switch (x) {} } }", "`switch`"),
        ("export default class Test { f(): void { try {} catch (e) {} } }", "`try`"),
        ("export default class Test { constructor() {} }", "constructor"),
        ("export default class Test { static s = 1; }", "static"),
        ("export default class Test { f(x: any): void {} }", "not supported"),
        ("export default class Test { get x(): int { return 1; } }", "accessors"),
        ("export default class Test { f(): void { for (const a of []) {} } }", "for ... of"),
        ("export default class Test { f(): void { var x = 1; } }", "var"),
        ("export default class Test extends Other {}", "may only extend `ScriptClass`"),
    ] {
        let found = errors(source);
        assert!(found.iter().any(|m| m.contains(needle)), "`{source}` should say `{needle}`, got {found:?}");
        assert!(found.iter().all(|m| m.contains(':') || m.starts_with("error")), "{found:?}");
    }
}

#[test]
fn type_errors_name_the_types_and_suggest_the_fix() {
    let found = errors("export default class Test { f(a: int, b: number): number { return a + b; } }");
    assert!(found[0].contains("different types (`int` and `number`)") && found[0].contains("as number"), "{found:?}");

    let found = errors("export default class Test { f(): int { return 1.5; } }");
    assert!(found[0].contains("expected `int`, found `number`"), "{found:?}");

    let found = errors("export default class Test { f(): void { let s: string = 1; } }");
    assert!(found[0].contains("expected `string`, found `int`"), "{found:?}");

    let found = errors("export default class Test { f(): void { this.nope = 1; } }");
    assert!(found[0].contains("no field `nope`"), "{found:?}");

    let found = errors("export default class Test { f(): void { Vec3.nope(1); } }");
    assert!(found[0].contains("no native `Vec3.nope`"), "{found:?}");

    let found = errors("export default class Test { f(): void { math.sqrt(1, 2); } }");
    assert!(found[0].contains("takes 1 argument(s), got 2"), "{found:?}");

    let found = errors("export default class Test { f(v: Vec3, w: Vec3): boolean { return v == w; } }");
    assert!(found[0].contains("does not apply to `Vec3`") && found[0].contains("a.eq(b)"), "{found:?}");
}

#[test]
fn control_flow_and_async_rules_are_enforced() {
    assert!(errors("export default class Test { f(): int { if (true) { return 1; } } }")[0].contains("not all code paths"));
    // The parser itself refuses a top-level `await` in a non-async method.
    assert!(errors("export default class Test { f(): void { await wait(1); } }")[0].contains("await"));
    assert!(errors("export default class Test { async f(): Promise<void> { wait(1); } }")[0].contains("must be awaited"));
    assert!(errors("export default class Test { async g(): Promise<void> {} f(): void { this.g(); } }")[0].contains("is async"));
    assert!(errors("export default class Test { f(): void { break; } }")[0].contains("outside a loop"));
    assert!(errors("export default class Test { async f(): Promise<int> { return 1; } }")[0].contains("Promise<void>"));
}

#[test]
fn lifecycle_methods_have_fixed_signatures() {
    assert!(errors("export default class Test { tick(): void {} }")[0].contains("`tick` must be declared"));
    assert!(errors("export default class Test { begin_play(x: int): void {} }")[0].contains("`begin_play` must be declared"));
    // The right shapes compile.
    module("export default class Test { begin_play(): void {} tick(delta: number): void {} end_play(): void {} migrate(from: int): void {} }");
}

#[test]
fn the_class_must_be_named_after_its_directory() {
    let found = errors("export default class Other {}");
    assert!(found[0].contains("named `Other` but its directory is `Test`"), "{found:?}");
    assert!(errors("// nothing here")[0].contains("no class found"));
}

#[test]
fn syntax_errors_come_from_the_parser_with_a_position() {
    let found = errors("export default class Test {\n  f(: void {}\n}");
    assert!(found[0].starts_with("error 2:"), "{found:?}");
}

// ---- schema and identity -------------------------------------------------------------

fn field(name: &str, ty: &str, renamed_from: Option<&str>) -> DeclaredField {
    DeclaredField { name: name.into(), ty: ty.into(), renamed_from: renamed_from.map(Into::into) }
}

#[test]
fn a_fields_id_survives_reordering_and_its_version_only_moves_when_the_schema_does() {
    let first = ClassSchema::reconcile(None, &[field("a", "int", None), field("b", "string", None)]).unwrap();
    assert_eq!(first.version, 1);
    let (id_a, id_b) = (first.id_of("a").unwrap().to_owned(), first.id_of("b").unwrap().to_owned());
    assert_ne!(id_a, id_b);

    let reordered = ClassSchema::reconcile(Some(&first), &[field("b", "string", None), field("a", "int", None)]).unwrap();
    assert_eq!((reordered.id_of("a"), reordered.id_of("b")), (Some(id_a.as_str()), Some(id_b.as_str())));
    assert_eq!(reordered.version, 1, "reordering is not a schema change");

    let added = ClassSchema::reconcile(Some(&reordered), &[field("a", "int", None), field("b", "string", None), field("c", "bool", None)]).unwrap();
    assert_eq!(added.version, 2);
    assert_eq!(added.id_of("a"), Some(id_a.as_str()));

    let retyped = ClassSchema::reconcile(Some(&added), &[field("a", "number", None), field("b", "string", None), field("c", "bool", None)]).unwrap();
    assert_eq!(retyped.version, 3, "a type change raises the version");
    assert_eq!(retyped.id_of("a"), Some(id_a.as_str()), "and keeps the identity");
}

#[test]
fn renamed_from_keeps_the_identity_and_a_plain_rename_does_not() {
    let first = ClassSchema::reconcile(None, &[field("old", "int", None)]).unwrap();
    let id = first.id_of("old").unwrap().to_owned();

    let renamed = ClassSchema::reconcile(Some(&first), &[field("new", "int", Some("old"))]).unwrap();
    assert_eq!(renamed.id_of("new"), Some(id.as_str()));
    assert_eq!(renamed.version, 2);

    let plain = ClassSchema::reconcile(Some(&first), &[field("new", "int", None)]).unwrap();
    assert_ne!(plain.id_of("new"), Some(id.as_str()), "without `@renamedFrom` it is a new field");

    assert!(ClassSchema::reconcile(Some(&first), &[field("x", "int", Some("never_existed"))]).is_err());
    assert!(ClassSchema::reconcile(Some(&first), &[field("a", "int", Some("old")), field("b", "int", Some("old"))]).is_err());
    assert!(ClassSchema::reconcile(None, &[field("x", "int", Some("old"))]).is_err());
}

#[test]
fn compiling_applies_the_schema_to_the_module() {
    let source = "export default class Test { @renamedFrom(\"hp\") health: int = 3; mana: number = 1.5; }";
    let previous = ClassSchema::reconcile(None, &[field("hp", "int", None), field("mana", "number", None)]).unwrap();
    let compiled = compile_class(&ClassSource { class_name: "Test", file: "class.ts", source, schema: Some(&previous) }, &registry());
    assert!(compiled.diagnostics.is_empty(), "{:?}", compiled.diagnostics);
    let (module, schema) = (compiled.module.unwrap(), compiled.schema.unwrap());
    assert_eq!(module.class_version, schema.version);
    assert_eq!(module.class_version, 2);
    let health = module.variables.iter().find(|v| v.name == "health").unwrap();
    assert_eq!(health.id.as_deref(), previous.id_of("hp"), "the module's variable carries the old field's id");
}

// ---- declarations -----------------------------------------------------------------------

#[test]
fn declarations_cover_natives_value_types_and_are_deterministic() {
    let registry = registry();
    let dts = declarations(&registry);
    assert_eq!(dts, declarations(&registry), "the same registry produces the same text");
    for expected in [
        "declare function wait(seconds: number): Promise<void>;",
        "declare namespace math {",
        "function sqrt(x: number): number;",
        "declare namespace Vec3 {",
        "function new_(x: number, y: number, z: number): Vec3;",
        "interface Vec3 {",
        "add(b: Vec3): Vec3;",
        "readonly x: number;",
        "type int = number;",
        "declare abstract class ScriptClass",
    ] {
        assert!(dts.contains(expected), "missing `{expected}` in:\n{dts}");
    }
    assert!(!dts.contains("function new("), "reserved words are sanitised");
}

#[test]
fn every_declared_native_is_callable_from_the_compiler() {
    // What the declarations promise, the compiler accepts.
    let call = "export default class Test { f(): number { return math.sqrt(4.0); } g(v: Vec3): Vec3 { return v.add(Vec3.new_(1, 2, 3)); } }";
    module(call);
}
