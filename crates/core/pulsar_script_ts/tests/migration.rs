//! Hot reload and saved state across versions of a TypeScript class.

use pulsar_script_runtime::{ChangeKind, SavedState, ScriptRuntime};
use pulsar_script_ts::{compile_class, ClassSchema, ClassSource};
use pulsar_script_vm::{Module, NativeRegistry, Value};

use pulsar_script_math as _;

fn compile(source: &str, schema: Option<&ClassSchema>) -> (Module, ClassSchema) {
    let compiled = compile_class(
        &ClassSource {
            class_name: "Hero",
            file: "class.ts",
            source,
            schema,
        },
        &NativeRegistry::with_engine_natives(),
    );
    assert!(
        compiled.diagnostics.is_empty(),
        "{:#?}",
        compiled.diagnostics
    );
    (
        compiled.module.expect("a module"),
        compiled.schema.expect("a schema"),
    )
}

const V1: &str = r#"
export default class Hero extends ScriptClass {
    hp: int = 10;
    position: Vec3 = Vec3.new_(1, 2, 3);
    name: string = "hero";
}
"#;

// `hp` became `health` (keeping its value), `shield` is new and is filled from
// the old `hp` by `migrate`, and `name` is gone.
const V2: &str = r#"
export default class Hero extends ScriptClass {
    @renamedFrom("hp")
    health: int = 10;
    position: Vec3 = Vec3.new_(1, 2, 3);
    shield: number = 0;

    migrate(from: int): void {
        this.shield = (migration.old_int("hp") as number) / 100;
    }
}
"#;

fn runtime() -> ScriptRuntime {
    ScriptRuntime::new(
        std::env::temp_dir().join(format!("pulsar_ts_migration_{}", std::process::id())),
    )
}

fn kinds(changes: &[pulsar_script_runtime::VariableChange], variable: &str) -> Vec<ChangeKind> {
    changes
        .iter()
        .filter(|c| c.variable == variable)
        .map(|c| c.kind.clone())
        .collect()
}

#[test]
fn a_new_version_of_a_class_reloads_into_running_instances() {
    let (v1, schema1) = compile(V1, None);
    let mut rt = runtime();
    rt.load_class(v1).unwrap();
    rt.spawn("a", "Hero", None, &[]).unwrap();
    rt.set_variable("a", "hp", Value::Int(7)).unwrap();
    let moved = pulsar_script_vm::TypeRegistry::global()
        .decode_value("Vec3", "[9, 8, 7]")
        .unwrap();
    rt.set_variable("a", "position", moved).unwrap();

    let (v2, schema2) = compile(V2, Some(&schema1));
    assert_eq!(
        schema2.version,
        schema1.version + 1,
        "the field set changed"
    );
    let report = rt.reload_class(v2).unwrap();

    assert_eq!(
        rt.variable("a", "health"),
        Some(&Value::Int(7)),
        "the renamed field kept its value"
    );
    assert_eq!(
        rt.variable("a", "shield"),
        Some(&Value::Float(0.07)),
        "migrate read the old value"
    );
    let Some(Value::Object(position)) = rt.variable("a", "position") else {
        panic!("position")
    };
    assert_eq!(
        pulsar_script_vm::TypeRegistry::global()
            .encode_value(position)
            .unwrap(),
        "[9.0,8.0,7.0]",
        "the Vec3 carried over"
    );
    assert_eq!(rt.variable("a", "name"), None, "the removed field is gone");

    assert_eq!(
        kinds(&report.variables, "health"),
        [ChangeKind::Renamed { from: "hp".into() }]
    );
    assert_eq!(kinds(&report.variables, "shield"), [ChangeKind::Defaulted]);
    assert_eq!(kinds(&report.variables, "name"), [ChangeKind::Removed]);
    assert_eq!(
        kinds(&report.variables, "migrate"),
        [ChangeKind::MigrateRan {
            from_version: schema1.version
        }]
    );
}

#[test]
fn without_renamed_from_a_rename_does_not_carry_the_value() {
    let (v1, schema1) = compile(V1, None);
    let mut rt = runtime();
    rt.load_class(v1).unwrap();
    rt.spawn("a", "Hero", None, &[]).unwrap();
    rt.set_variable("a", "hp", Value::Int(7)).unwrap();

    let renamed = V1.replace("hp: int", "health: int");
    let (v2, _) = compile(&renamed, Some(&schema1));
    rt.reload_class(v2).unwrap();
    assert_eq!(
        rt.variable("a", "health"),
        Some(&Value::Int(10)),
        "a different field: it starts at its default"
    );
}

#[test]
fn an_old_save_loads_into_the_new_version() {
    let (v1, schema1) = compile(V1, None);
    let mut rt = runtime();
    rt.load_class(v1).unwrap();
    rt.spawn("a", "Hero", None, &[]).unwrap();
    rt.set_variable("a", "hp", Value::Int(42)).unwrap();
    let saved = rt.save_state("a").unwrap();
    let saved: SavedState = serde_json::from_str(&serde_json::to_string(&saved).unwrap()).unwrap();
    assert_eq!(saved.class_version, schema1.version);

    let (v2, _) = compile(V2, Some(&schema1));
    let mut later = runtime();
    later.load_class(v2).unwrap();
    later.spawn("b", "Hero", None, &[]).unwrap();
    let report = later.restore_state("b", &saved).unwrap();
    assert_eq!(later.variable("b", "health"), Some(&Value::Int(42)));
    assert_eq!(
        later.variable("b", "shield"),
        Some(&Value::Float(0.42)),
        "migrate ran on the restored state"
    );
    assert!(report.unreadable.is_empty(), "{report:?}");
}

#[test]
fn a_migrate_that_cannot_run_refuses_the_reload() {
    let (v1, schema1) = compile(V1, None);
    let mut rt = runtime();
    rt.load_class(v1).unwrap();
    rt.spawn("a", "Hero", None, &[]).unwrap();
    // The old class had no `mana`: reading it fails, so the reload is refused.
    let bad = V2.replace("old_int(\"hp\")", "old_int(\"mana\")");
    let (v2, _) = compile(&bad, Some(&schema1));
    assert!(rt.reload_class(v2).is_err());
    assert_eq!(
        rt.variable("a", "hp"),
        Some(&Value::Int(10)),
        "the old class keeps running"
    );
}
