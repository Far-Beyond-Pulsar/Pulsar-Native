use std::sync::Arc;

use glam::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use pulsar_script_math::literal;
use pulsar_script_vm::{
    Budget, Constant, Function, Host, Import, Instr, LinkError, Module, NativeRegistry, Param, Program, Signature,
    Type, TypeRegistry, Value, Variable, Vm,
};
use pulsar_scenedb::World;

fn obj(name: &str) -> Type {
    Type::object(name)
}

fn constant(ty: &str, json: impl Into<String>) -> Constant {
    Constant::Value { ty: ty.into(), json: json.into() }
}

fn function(name: &str, ret: Type, registers: Vec<Type>, code: Vec<Instr>) -> Function {
    Function { name: name.into(), exported: true, params: vec![], ret, registers, code, debug: None }
}

fn link(module: Module) -> Result<Program, LinkError> {
    Program::link(Arc::new(module), &NativeRegistry::with_engine_natives())
}

fn run(program: &Program, name: &str) -> Value {
    let mut world = World::new();
    let entity = world.spawn();
    let mut instance = program.instantiate();
    let func = program.entry(name).expect("entry");
    let mut host = Host::new(&mut world, entity);
    Vm::new().call(program, &mut instance, func, &[], &mut host, &mut Budget::new(10_000)).expect("run")
}

fn as_value<T: Clone + 'static>(value: &Value) -> T {
    match value {
        Value::Object(o) => o.downcast_ref::<T>().expect("object type").clone(),
        other => panic!("expected an object, got {other:?}"),
    }
}

#[test]
fn every_math_type_is_registered_with_a_default() {
    let types = TypeRegistry::global();
    for name in ["Vec2", "Vec3", "Vec4", "DVec3", "Quat", "Mat4"] {
        assert!(types.is_known(&obj(name)), "{name}");
        assert!(types.default_value(&obj(name)).is_some(), "{name}");
    }
    assert_eq!(as_value::<Quat>(&types.default_value(&obj("Quat")).unwrap()), Quat::IDENTITY);
    assert_eq!(as_value::<Mat4>(&types.default_value(&obj("Mat4")).unwrap()), Mat4::IDENTITY);
}

#[test]
fn literals_round_trip_through_every_type() {
    let types = TypeRegistry::global();
    let decode = |ty: &str, json: String| types.decode_value(ty, &json).unwrap();
    assert_eq!(as_value::<Vec2>(&decode("Vec2", literal::vec2(Vec2::new(1.0, -2.5)))), Vec2::new(1.0, -2.5));
    assert_eq!(as_value::<Vec3>(&decode("Vec3", literal::vec3(Vec3::new(0.0, 1.0, 0.0)))), Vec3::Y);
    assert_eq!(as_value::<Vec4>(&decode("Vec4", literal::vec4(Vec4::new(1.0, 2.0, 3.0, 4.0)))), Vec4::new(1.0, 2.0, 3.0, 4.0));
    assert_eq!(as_value::<DVec3>(&decode("DVec3", literal::dvec3(DVec3::new(1e10, 0.1, -3.0)))), DVec3::new(1e10, 0.1, -3.0));
    let q = Quat::from_rotation_y(0.5);
    assert_eq!(as_value::<Quat>(&decode("Quat", literal::quat(q))), q);
    let m = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(as_value::<Mat4>(&decode("Mat4", literal::mat4(m))), m);
}

#[test]
fn matrix_literals_are_column_major() {
    let mut elements: Vec<f64> = (0..16).map(f64::from).collect();
    elements[12] = 7.0; // translation x lives at column 3, row 0
    let value = TypeRegistry::global().decode_value("Mat4", &serde_json::to_string(&elements).unwrap()).unwrap();
    let m = as_value::<Mat4>(&value);
    assert_eq!(m.col(3).x, 7.0);
    assert_eq!(m.col(1).z, 6.0);
}

#[test]
fn malformed_literals_are_rejected_with_a_reason() {
    let types = TypeRegistry::global();
    for (ty, json, needle) in [
        ("Vec3", "[1, 2]", "expected 3 numbers, found 2"),
        ("Vec3", "[1, 2, \"x\"]", "invalid type"),
        ("Vec3", "{}", "invalid type"),
        ("Vec3", "", "EOF"),
        ("Mat4", "[1]", "expected 16"),
        ("Nope", "[1]", "unknown value type"),
    ] {
        let err = types.decode_value(ty, json).expect_err(json);
        assert!(err.contains(needle), "{ty} {json}: {err}");
    }
}

#[test]
fn a_constant_value_links_loads_and_is_typed() {
    let mut module = Module::new("literal");
    module.constants.push(constant("Vec3", "[0, 1, 0]"));
    module.functions.push(function(
        "up",
        obj("Vec3"),
        vec![obj("Vec3")],
        vec![Instr::Const { dst: 0, index: 0 }, Instr::Return { value: Some(0) }],
    ));
    let program = link(module).unwrap();
    assert_eq!(as_value::<Vec3>(&run(&program, "up")), Vec3::Y);
}

#[test]
fn a_literal_of_the_wrong_register_type_fails_verification() {
    let mut module = Module::new("wrong");
    module.constants.push(constant("Vec3", "[0, 1, 0]"));
    module.functions.push(function(
        "bad",
        obj("Vec2"),
        vec![obj("Vec2")],
        vec![Instr::Const { dst: 0, index: 0 }, Instr::Return { value: Some(0) }],
    ));
    assert!(matches!(link(module), Err(LinkError::Verify(_))));
}

#[test]
fn a_malformed_or_unknown_constant_is_a_link_error_with_a_site() {
    for (ty, json) in [("Vec3", "[1,2]"), ("Ghost", "[1]")] {
        let mut module = Module::new("bad");
        module.constants.push(constant(ty, json));
        module.functions.push(function(
            "f",
            obj(ty),
            vec![obj(ty)],
            vec![Instr::Const { dst: 0, index: 0 }, Instr::Return { value: Some(0) }],
        ));
        let err = link(module.clone()).err().expect("link must fail");
        assert!(matches!(&err, LinkError::BadConstant { .. }) || matches!(&err, LinkError::UnknownType { .. }), "{err}");
        if matches!(err, LinkError::BadConstant { .. }) {
            let site = module.locate_link_error(&err).expect("site");
            assert_eq!((site.function.as_str(), site.pc), ("f", Some(0)));
        }
    }
}

#[test]
fn a_variable_default_is_a_fresh_copy_per_instance() {
    let mut module = Module::new("vars");
    module.variables.push(Variable {
        name: "dir".into(),
        ty: obj("Vec3"),
        default: Some(constant("Vec3", "[0, 1, 0]")),
    });
    let with_x = module.imports.len() as u32;
    module.imports.push(Import {
        name: "Vec3::with_x".into(),
        sig: Signature::new(vec![Param::new(obj("Vec3")), Param::new(Type::Float)], obj("Vec3")),
    });
    module.constants.push(Constant::Float(9.0));
    module.functions.push(function(
        "poke",
        Type::Unit,
        vec![obj("Vec3"), Type::Float, obj("Vec3")],
        vec![
            Instr::LoadVar { dst: 0, var: 0 },
            Instr::Const { dst: 1, index: 0 },
            Instr::CallNative { import: with_x, args: vec![0, 1], dst: Some(2) },
            Instr::StoreVar { var: 0, src: 2 },
            Instr::Return { value: None },
        ],
    ));
    let program = link(module).unwrap();

    let mut world = World::new();
    let entity = world.spawn();
    let (mut a, b) = (program.instantiate(), program.instantiate());
    let func = program.entry("poke").unwrap();
    let mut host = Host::new(&mut world, entity);
    Vm::new().call(&program, &mut a, func, &[], &mut host, &mut Budget::new(10_000)).unwrap();

    let var = program.variable("dir").unwrap();
    assert_eq!(as_value::<Vec3>(program.var(&a, var).unwrap()), Vec3::new(9.0, 1.0, 0.0));
    // The other instance, and the module's own default, are untouched.
    assert_eq!(as_value::<Vec3>(program.var(&b, var).unwrap()), Vec3::Y);
    assert_eq!(as_value::<Vec3>(program.var(&program.instantiate(), var).unwrap()), Vec3::Y);
}

#[test]
fn modules_with_value_constants_round_trip_json_and_binary() {
    let mut module = Module::new("codec");
    module.constants.push(constant("Quat", literal::quat(Quat::from_rotation_x(1.0))));
    let json = serde_json::to_string(&module).unwrap();
    let from_json: Module = serde_json::from_str(&json).unwrap();
    assert_eq!(from_json.constants, module.constants);
    let from_binary = Module::decode(&module.to_binary()).unwrap();
    assert_eq!(from_binary.constants, module.constants);
}

/// Call `name` with `args`, returning the result.
fn native(name: &str, params: Vec<Type>, ret: Type, args: Vec<Constant>) -> Value {
    let mut module = Module::new("native");
    module.imports.push(Import {
        name: name.into(),
        sig: Signature::new(params.iter().cloned().map(Param::new).collect::<Vec<_>>(), ret.clone()),
    });
    let mut registers = params.clone();
    registers.push(ret.clone());
    let mut code = Vec::new();
    for (i, constant) in args.into_iter().enumerate() {
        module.constants.push(constant);
        code.push(Instr::Const { dst: i as u16, index: i as u32 });
    }
    let dst = params.len() as u16;
    code.push(Instr::CallNative { import: 0, args: (0..dst).collect(), dst: Some(dst) });
    code.push(Instr::Return { value: Some(dst) });
    module.functions.push(function("f", ret, registers, code));
    run(&link(module).unwrap(), "f")
}

fn v3(x: f32, y: f32, z: f32) -> Constant {
    constant("Vec3", literal::vec3(Vec3::new(x, y, z)))
}

#[test]
fn natives_construct_split_and_compute() {
    let f = Type::Float;
    let made = native("Vec3::new", vec![f.clone(); 3], obj("Vec3"), vec![Constant::Float(1.0), Constant::Float(2.0), Constant::Float(3.0)]);
    assert_eq!(as_value::<Vec3>(&made), Vec3::new(1.0, 2.0, 3.0));

    let y = native("Vec3::y", vec![obj("Vec3")], f.clone(), vec![v3(1.0, 2.0, 3.0)]);
    assert_eq!(y, Value::Float(2.0));

    let cross = native("Vec3::cross", vec![obj("Vec3"); 2], obj("Vec3"), vec![v3(1.0, 0.0, 0.0), v3(0.0, 1.0, 0.0)]);
    assert_eq!(as_value::<Vec3>(&cross), Vec3::Z);

    // Zero normalizes to zero, not NaN.
    let n = native("Vec3::normalize", vec![obj("Vec3")], obj("Vec3"), vec![v3(0.0, 0.0, 0.0)]);
    assert_eq!(as_value::<Vec3>(&n), Vec3::ZERO);

    let rotated = native(
        "Quat::rotate",
        vec![obj("Quat"), obj("Vec3")],
        obj("Vec3"),
        vec![constant("Quat", literal::quat(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))), v3(1.0, 0.0, 0.0)],
    );
    assert!(as_value::<Vec3>(&rotated).abs_diff_eq(Vec3::Y, 1e-6));
}

#[test]
fn equality_is_raw_unless_a_tolerance_is_given() {
    let eq = |a: Constant, b: Constant| native("Vec3::eq", vec![obj("Vec3"); 2], Type::Bool, vec![a, b]);
    assert_eq!(eq(v3(1.0, 2.0, 3.0), v3(1.0, 2.0, 3.0)), Value::Bool(true));
    assert_eq!(eq(v3(1.0, 2.0, 3.0), v3(1.0, 2.0, 3.001)), Value::Bool(false));
    let approx = native(
        "Vec3::approx_eq",
        vec![obj("Vec3"), obj("Vec3"), Type::Float],
        Type::Bool,
        vec![v3(1.0, 2.0, 3.0), v3(1.0, 2.0, 3.001), Constant::Float(0.01)],
    );
    assert_eq!(approx, Value::Bool(true));

    // q and -q are one rotation, but not raw-equal.
    let q = Quat::from_rotation_y(0.7);
    let (a, b) = (constant("Quat", literal::quat(q)), constant("Quat", literal::quat(-q)));
    let sig = vec![obj("Quat"), obj("Quat")];
    assert_eq!(native("Quat::eq", sig.clone(), Type::Bool, vec![a.clone(), b.clone()]), Value::Bool(false));
    let mut with_eps = sig;
    with_eps.push(Type::Float);
    assert_eq!(native("Quat::same_rotation", with_eps, Type::Bool, vec![a, b, Constant::Float(1e-6)]), Value::Bool(true));
}

#[test]
fn to_string_and_matrix_failures_are_reported() {
    let s = native("Vec3::to_string", vec![obj("Vec3")], Type::Str, vec![v3(1.0, 2.0, 3.0)]);
    assert_eq!(s.as_str(), Some("[1, 2, 3]"));

    let mut module = Module::new("singular");
    module.imports.push(Import {
        name: "Mat4::inverse".into(),
        sig: Signature::new(vec![Param::new(obj("Mat4"))], obj("Mat4")),
    });
    module.constants.push(constant("Mat4", literal::mat4(Mat4::ZERO)));
    module.functions.push(function(
        "f",
        obj("Mat4"),
        vec![obj("Mat4"), obj("Mat4")],
        vec![
            Instr::Const { dst: 0, index: 0 },
            Instr::CallNative { import: 0, args: vec![0], dst: Some(1) },
            Instr::Return { value: Some(1) },
        ],
    ));
    let program = link(module).unwrap();
    let mut world = World::new();
    let entity = world.spawn();
    let mut instance = program.instantiate();
    let func = program.entry("f").unwrap();
    let mut host = Host::new(&mut world, entity);
    let err = Vm::new()
        .call(&program, &mut instance, func, &[], &mut host, &mut Budget::new(1000))
        .expect_err("singular matrix");
    assert!(err.to_string().contains("not invertible"), "{err}");
}
