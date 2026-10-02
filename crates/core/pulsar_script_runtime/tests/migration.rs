//! State migration: stable variable ids, versions, the `migrate` hook and
//! saved state.

use glam::Vec3;
use pulsar_scenedb::World;
use pulsar_script_math as _;
use pulsar_script_runtime::{ChangeKind, RuntimeError, SavedState, ScriptRuntime, VariableChange};
use pulsar_script_vm::{
    Constant, Function, Import, Instr, Module, Param, Signature, Type, UnOp, Value, Variable,
};

use Instr::*;

fn runtime() -> ScriptRuntime {
    ScriptRuntime::new(std::env::temp_dir().join(format!("pulsar_script_migration_{}", std::process::id())))
}

fn var(id: Option<&str>, name: &str, ty: Type) -> Variable {
    Variable { name: name.into(), ty, default: None, id: id.map(Into::into) }
}

fn class(name: &str, version: u32, variables: Vec<Variable>, functions: Vec<Function>) -> Module {
    let mut m = Module::new(name);
    m.class_version = version;
    m.variables = variables;
    m.functions = functions;
    m
}

fn function(name: &str, params: Vec<Type>, extra: Vec<Type>, code: Vec<Instr>) -> Function {
    let mut registers = params.clone();
    registers.extend(extra);
    Function { name: name.into(), exported: true, params, ret: Type::Unit, registers, code, debug: None }
}

fn reload(rt: &mut ScriptRuntime, module: Module) -> pulsar_script_runtime::ReloadReport {
    rt.reload_class(module).expect("reload")
}

fn kinds(changes: &[VariableChange], variable: &str) -> Vec<ChangeKind> {
    changes.iter().filter(|c| c.variable == variable).map(|c| c.kind.clone()).collect()
}

/// A class with one instance `a` whose int variable `hp` is 7.
fn with_hp(id: Option<&str>) -> ScriptRuntime {
    let mut rt = runtime();
    rt.load_class(class("Unit", 0, vec![var(id, "hp", Type::Int), var(None, "keep", Type::Int)], vec![])).unwrap();
    rt.spawn("a", "Unit", None, &[]).unwrap();
    rt.set_variable("a", "hp", Value::Int(7)).unwrap();
    rt.set_variable("a", "keep", Value::Int(3)).unwrap();
    rt
}

#[test]
fn a_rename_keeps_the_value_by_id() {
    let mut rt = with_hp(Some("v-hp"));
    let report = reload(
        &mut rt,
        class("Unit", 0, vec![var(Some("v-hp"), "health", Type::Int), var(None, "keep", Type::Int)], vec![]),
    );
    assert_eq!(rt.variable("a", "health"), Some(&Value::Int(7)));
    assert_eq!(rt.variable("a", "keep"), Some(&Value::Int(3)));
    assert_eq!(rt.variable("a", "hp"), None);
    assert_eq!(kinds(&report.variables, "health"), [ChangeKind::Renamed { from: "hp".into() }]);
    assert_eq!(report.variables_kept, 1);
}

#[test]
fn reordering_adding_and_removing_keep_the_rest() {
    let mut rt = with_hp(Some("v-hp"));
    let report = reload(
        &mut rt,
        class(
            "Unit",
            0,
            vec![var(Some("v-new"), "fresh", Type::Int), var(None, "keep", Type::Int), var(Some("v-hp"), "hp", Type::Int)],
            vec![],
        ),
    );
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Int(7)));
    assert_eq!(rt.variable("a", "keep"), Some(&Value::Int(3)));
    assert_eq!(rt.variable("a", "fresh"), Some(&Value::Int(0)));
    assert_eq!(kinds(&report.variables, "fresh"), [ChangeKind::Defaulted]);

    // Removing a variable discards its value and says so.
    let report = reload(&mut rt, class("Unit", 0, vec![var(None, "keep", Type::Int)], vec![]));
    assert_eq!(kinds(&report.variables, "hp"), [ChangeKind::Removed]);
    assert_eq!(rt.variable("a", "keep"), Some(&Value::Int(3)));
}

#[test]
fn a_reused_name_with_another_id_does_not_inherit_the_value() {
    let mut rt = with_hp(Some("v-hp"));
    let report = reload(
        &mut rt,
        class("Unit", 0, vec![var(Some("v-other"), "hp", Type::Int), var(None, "keep", Type::Int)], vec![]),
    );
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Int(0)), "a different variable is not the old hp");
    assert_eq!(kinds(&report.variables, "hp"), [ChangeKind::Defaulted, ChangeKind::Removed]);
}

#[test]
fn data_from_before_ids_matches_by_name() {
    let mut rt = with_hp(None);
    reload(&mut rt, class("Unit", 0, vec![var(Some("v-hp"), "hp", Type::Int), var(None, "keep", Type::Int)], vec![]));
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Int(7)));
}

#[test]
fn a_retyped_variable_starts_at_its_default_and_is_reported() {
    let mut rt = with_hp(Some("v-hp"));
    let report = reload(
        &mut rt,
        class("Unit", 0, vec![var(Some("v-hp"), "hp", Type::Float), var(None, "keep", Type::Int)], vec![]),
    );
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Float(0.0)));
    assert_eq!(kinds(&report.variables, "hp"), [ChangeKind::Incompatible { from: Type::Int, to: Type::Float }]);
}

#[test]
fn ambiguous_ids_are_refused_and_the_old_class_keeps_running() {
    let mut rt = with_hp(Some("v-hp"));
    let err = rt
        .reload_class(class(
            "Unit",
            0,
            vec![var(Some("same"), "hp", Type::Int), var(Some("same"), "keep", Type::Int)],
            vec![],
        ))
        .unwrap_err();
    assert!(matches!(err, RuntimeError::Link { .. }), "{err}");
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Int(7)));
    assert_eq!(rt.variable("a", "keep"), Some(&Value::Int(3)));
}

/// `migrate(from)`: `hp = to_float(old("hp"))`, `was = from`.
fn migrating_class(version: u32, body: Vec<Instr>, registers: Vec<Type>) -> Module {
    let mut m = class(
        "Unit",
        version,
        vec![var(Some("v-hp"), "hp", Type::Float), var(None, "was", Type::Int), var(None, "keep", Type::Int)],
        vec![function("migrate", vec![Type::Int], registers, body)],
    );
    m.imports.push(Import {
        name: "migration::old_int".into(),
        sig: Signature::new(vec![Param::new(Type::Str)], Type::Int),
    });
    m.constants = vec![Constant::Str("hp".into()), Constant::Str("missing".into()), Constant::Float(1.0)];
    m
}

#[test]
fn migrate_converts_old_values_when_the_class_version_rises() {
    let mut rt = with_hp(Some("v-hp"));
    let module = migrating_class(
        2,
        vec![
            Const { dst: 1, index: 0 },
            CallNative { import: 0, args: vec![1], dst: Some(2) },
            Unary { op: UnOp::IntToFloat, dst: 3, src: 2 },
            StoreVar { var: 0, src: 3 },
            StoreVar { var: 1, src: 0 },
            Return { value: None },
        ],
        vec![Type::Str, Type::Int, Type::Float],
    );
    let report = reload(&mut rt, module);
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Float(7.0)));
    assert_eq!(rt.variable("a", "was"), Some(&Value::Int(0)), "from_version is the old class version");
    assert_eq!(rt.variable("a", "keep"), Some(&Value::Int(3)));
    assert_eq!(kinds(&report.variables, "migrate"), [ChangeKind::MigrateRan { from_version: 0 }]);
}

#[test]
fn migrate_does_not_run_unless_the_version_rises() {
    let mut rt = with_hp(Some("v-hp"));
    let module = migrating_class(
        0,
        vec![Const { dst: 1, index: 0 }, CallNative { import: 0, args: vec![1], dst: Some(2) }, Return { value: None }],
        vec![Type::Str, Type::Int],
    );
    let report = reload(&mut rt, module);
    assert!(kinds(&report.variables, "migrate").is_empty());
}

#[test]
fn a_failing_migrate_refuses_the_whole_reload() {
    let mut rt = with_hp(Some("v-hp"));
    rt.spawn("b", "Unit", None, &[]).unwrap();
    let module = migrating_class(
        2,
        vec![
            Const { dst: 1, index: 1 }, // `missing`: the old class had no such variable
            CallNative { import: 0, args: vec![1], dst: Some(2) },
            Return { value: None },
        ],
        vec![Type::Str, Type::Int],
    );
    let err = rt.reload_class(module).unwrap_err();
    assert!(matches!(err, RuntimeError::Migration { .. }), "{err}");
    // Nothing changed: still the old class, with both instances intact.
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Int(7)));
    assert_eq!(rt.variable("b", "hp"), Some(&Value::Int(0)));
    assert_eq!(rt.variable("a", "was"), None);
}

#[test]
fn migrate_cannot_suspend() {
    let mut rt = with_hp(Some("v-hp"));
    let mut module = migrating_class(2, vec![Const { dst: 1, index: 2 }, Wait { seconds: 1 }, Return { value: None }], vec![Type::Float, Type::Float]);
    module.functions[0].registers = vec![Type::Int, Type::Float];
    module.functions[0].code = vec![Const { dst: 1, index: 2 }, Wait { seconds: 1 }, Return { value: None }];
    let err = rt.reload_class(module).unwrap_err();
    assert!(matches!(err, RuntimeError::Migration { .. }), "{err}");
    assert_eq!(rt.variable("a", "hp"), Some(&Value::Int(7)));
}

#[test]
fn a_migrate_with_the_wrong_signature_is_an_entry_point_error() {
    let mut rt = with_hp(Some("v-hp"));
    let mut module = migrating_class(2, vec![Return { value: None }], vec![]);
    module.functions[0].params = vec![Type::Float];
    module.functions[0].registers = vec![Type::Float];
    let err = rt.reload_class(module).unwrap_err();
    assert!(matches!(err, RuntimeError::BadEntryPoint { name: "migrate", .. }), "{err}");
}

#[test]
fn overrides_name_a_variable_by_id_or_name_and_refuse_ambiguity() {
    let mut rt = with_hp(Some("v-hp"));
    rt.spawn("by_id", "Unit", None, &[("v-hp".into(), Value::Int(11))]).unwrap();
    rt.spawn("by_name", "Unit", None, &[("hp".into(), Value::Int(12))]).unwrap();
    assert_eq!(rt.variable("by_id", "hp"), Some(&Value::Int(11)));
    assert_eq!(rt.variable("by_name", "hp"), Some(&Value::Int(12)));

    // `x` is the id of one variable and the name of another.
    let mut rt = runtime();
    rt.load_class(class("Odd", 0, vec![var(Some("x"), "a", Type::Int), var(Some("y"), "x", Type::Int)], vec![]))
        .unwrap();
    let err = rt.spawn("o", "Odd", None, &[("x".into(), Value::Int(1))]).unwrap_err();
    assert!(matches!(err, RuntimeError::BadVariable { .. }), "{err}");
}

// ---- saved state ----------------------------------------------------------

fn pose(id_pos: Option<&str>, name: &str) -> Module {
    class("Pose", 0, vec![var(id_pos, name, Type::object("Vec3")), var(None, "count", Type::Int)], vec![])
}

#[test]
fn saved_state_round_trips_including_value_types() {
    let mut rt = runtime();
    rt.load_class(pose(Some("v-pos"), "position")).unwrap();
    rt.spawn("p", "Pose", None, &[]).unwrap();
    let value = pulsar_script_vm::TypeRegistry::global()
        .decode_value("Vec3", "[1.5, 2.0, -3.0]")
        .expect("decode");
    rt.set_variable("p", "position", value).unwrap();
    rt.set_variable("p", "count", Value::Int(4)).unwrap();

    let saved = rt.save_state("p").unwrap();
    let text = serde_json::to_string(&saved).unwrap();
    let saved: SavedState = serde_json::from_str(&text).unwrap();

    rt.spawn("q", "Pose", None, &[]).unwrap();
    let report = rt.restore_state("q", &saved).unwrap();
    assert_eq!(report.variables_kept, 2);
    assert!(report.changes.is_empty() && report.unreadable.is_empty(), "{report:?}");
    let Some(Value::Object(restored)) = rt.variable("q", "position") else { panic!("not an object") };
    assert_eq!(restored.downcast_ref::<Vec3>(), Some(&Vec3::new(1.5, 2.0, -3.0)));
    assert_eq!(rt.variable("q", "count"), Some(&Value::Int(4)));
}

#[test]
fn an_old_save_loads_into_a_renamed_and_extended_class() {
    let mut rt = runtime();
    rt.load_class(pose(Some("v-pos"), "position")).unwrap();
    rt.spawn("p", "Pose", None, &[]).unwrap();
    rt.set_variable("p", "count", Value::Int(9)).unwrap();
    let saved = rt.save_state("p").unwrap();

    // Later: `position` was renamed and a variable added.
    let mut newer = class(
        "Pose",
        1,
        vec![var(Some("v-pos"), "location", Type::object("Vec3")), var(None, "count", Type::Int), var(None, "extra", Type::Int)],
        vec![],
    );
    newer.variables[1].default = Some(Constant::Int(0));
    rt.reload_class(newer).unwrap();
    let report = rt.restore_state("p", &saved).unwrap();
    assert_eq!(rt.variable("p", "count"), Some(&Value::Int(9)));
    assert_eq!(kinds(&report.changes, "location"), [ChangeKind::Renamed { from: "position".into() }]);
    assert_eq!(kinds(&report.changes, "extra"), [ChangeKind::Defaulted]);
}

#[test]
fn handles_are_not_saved_and_survive_a_restore() {
    let mut rt = runtime();
    rt.load_class(class("Holder", 0, vec![var(None, "target", Type::Entity), var(None, "n", Type::Int)], vec![])).unwrap();
    let mut world = World::new();
    let entity = world.spawn();
    rt.spawn("h", "Holder", None, &[("target".into(), Value::Entity(entity))]).unwrap();
    rt.set_variable("h", "n", Value::Int(5)).unwrap();

    let saved = rt.save_state("h").unwrap();
    assert!(saved.variables.iter().all(|v| v.name != "target"), "entities are process-local");

    rt.set_variable("h", "n", Value::Int(0)).unwrap();
    let report = rt.restore_state("h", &saved).unwrap();
    assert_eq!(rt.variable("h", "n"), Some(&Value::Int(5)));
    assert_eq!(rt.variable("h", "target"), Some(&Value::Entity(entity)), "the host's binding is kept");
    assert!(report.changes.is_empty(), "{report:?}");
}

#[test]
fn conflicting_saved_identities_and_unreadable_values_are_reported() {
    let mut rt = runtime();
    rt.load_class(pose(Some("v-pos"), "position")).unwrap();
    rt.spawn("p", "Pose", None, &[]).unwrap();
    let mut saved = rt.save_state("p").unwrap();

    let mut dup = saved.clone();
    dup.variables[1].id = dup.variables[0].id.clone();
    assert!(matches!(rt.restore_state("p", &dup), Err(RuntimeError::State { .. })));

    let mut wrong_class = saved.clone();
    wrong_class.class = "Other".into();
    assert!(matches!(rt.restore_state("p", &wrong_class), Err(RuntimeError::State { .. })));

    saved.variables[0].value = serde_json::json!("not a vector");
    let report = rt.restore_state("p", &saved).unwrap();
    assert_eq!(report.unreadable.len(), 1);
    assert_eq!(report.unreadable[0].0, "position");
}
