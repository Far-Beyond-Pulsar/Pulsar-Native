//! Scripts reaching the world: component references, component methods,
//! properties, value types with methods, and the frontend query API.

// `#[derive(Reflectable)]` names its type-info static after the type.
#![allow(non_upper_case_globals)]

mod common;

use common::{float, Asm, Harness};
use pulsar_reflection::{reflect_methods, Reflectable};
use pulsar_scenedb::{component_methods, ComponentRef, Entity, World};
use pulsar_script_vm::{
    script_component, script_value_type, Instr, NativeRegistry, Object, Param, ScriptErrorKind,
    Type, Value,
};

use Instr::*;

#[derive(Clone, Debug, Default, PartialEq, Reflectable)]
pub struct Health {
    pub value: f32,
    pub max: f32,
}

#[component_methods]
impl Health {
    #[reflect_method(pure)]
    fn fraction(&self) -> f32 {
        self.value / self.max
    }

    #[reflect_method]
    fn damage(&mut self, amount: f32) {
        self.value = (self.value - amount).max(0.0);
    }

    /// Heal `other` by `amount`, taking it from this entity.
    #[world_method]
    fn donate(world: &mut World, entity: Entity, other: Entity, amount: f32) -> Result<(), String> {
        if world.get::<Health>(other).is_none() {
            return Err("recipient has no Health".into());
        }
        world.get_mut::<Health>(entity).unwrap().value -= amount;
        world.get_mut::<Health>(other).unwrap().value += amount;
        Ok(())
    }
}

script_component!(Health);

#[derive(Clone, Debug, Default, PartialEq, Reflectable)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

#[reflect_methods]
impl V2 {
    #[reflect_method(pure)]
    fn new(x: f32, y: f32) -> V2 {
        V2 { x, y }
    }

    #[reflect_method(pure)]
    fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    #[reflect_method]
    fn scale(&mut self, k: f32) {
        self.x *= k;
        self.y *= k;
    }

    #[reflect_method]
    fn normalize_into(&self, out: &mut V2) {
        let len = self.length();
        *out = V2 { x: self.x / len, y: self.y / len };
    }
}

script_value_type!(V2);

fn health() -> Type {
    Type::component("Health")
}

fn v2() -> Type {
    Type::object("V2")
}

fn spawn_health(h: &mut Harness, value: f32) -> Entity {
    let e = h.world.spawn();
    h.world.insert(e, Health { value, max: 100.0 });
    e
}

#[test]
fn component_methods_through_references() {
    let registry = NativeRegistry::with_engine_natives();
    let mut asm = Asm::new();
    let of = asm.import("Health::of", vec![Param::new(Type::Entity)], health());
    let damage = asm.import("Health::damage", vec![Param::new(health()), Param::new(Type::Float)], Type::Unit);
    let fraction = asm.import("Health::fraction", vec![Param::new(health())], Type::Float);
    // self.Health.damage(amount); return self.Health.fraction()
    asm.function(
        "hit",
        vec![Type::Float],
        Type::Float,
        vec![Type::Entity, health(), Type::Float],
        vec![
            SelfEntity { dst: 1 },
            CallNative { import: of, args: vec![1], dst: Some(2) },
            CallNative { import: damage, args: vec![2, 0], dst: None },
            CallNative { import: fraction, args: vec![2], dst: Some(3) },
            Return { value: Some(3) },
        ],
    );
    let program = asm.link(&registry);

    let mut h = Harness::new();
    let me = h.entity;
    h.world.insert(me, Health { value: 50.0, max: 100.0 });
    assert_eq!(h.run(&program, "hit", &[float(25.0)]).unwrap(), float(0.25));
    assert_eq!(h.world.get::<Health>(me).unwrap().value, 25.0);

    // The reference is checked on every use: no Health, no call.
    h.world.remove::<Health>(me);
    let err = h.run(&program, "hit", &[float(1.0)]).unwrap_err();
    assert!(matches!(&err.kind, ScriptErrorKind::Native { name, .. } if name == "Health::damage"), "{err}");
}

#[test]
fn world_methods_and_liveness() {
    let registry = NativeRegistry::with_engine_natives();
    let mut asm = Asm::new();
    let donate = asm.import(
        "Health::donate",
        vec![Param::new(health()), Param::new(Type::Entity), Param::new(Type::Float)],
        Type::Unit,
    );
    let exists = asm.import("Health::exists", vec![Param::new(health())], Type::Bool);
    asm.function("give", vec![health(), Type::Entity, Type::Float], Type::Unit, vec![], vec![
        CallNative { import: donate, args: vec![0, 1, 2], dst: None },
        Return { value: None },
    ]);
    asm.function("exists", vec![health()], Type::Bool, vec![Type::Bool], vec![
        CallNative { import: exists, args: vec![0], dst: Some(1) },
        Return { value: Some(1) },
    ]);
    let program = asm.link(&registry);

    let mut h = Harness::new();
    let a = spawn_health(&mut h, 30.0);
    let b = spawn_health(&mut h, 5.0);
    let a_ref = Value::Component(ComponentRef::of::<Health>(a));
    h.run(&program, "give", &[a_ref.clone(), Value::Entity(b), float(10.0)]).unwrap();
    assert_eq!(h.world.get::<Health>(a).unwrap().value, 20.0);
    assert_eq!(h.world.get::<Health>(b).unwrap().value, 15.0);

    let nobody = h.world.spawn();
    let err = h.run(&program, "give", &[a_ref.clone(), Value::Entity(nobody), float(1.0)]).unwrap_err();
    assert!(err.to_string().contains("recipient has no Health"), "{err}");

    assert_eq!(h.run(&program, "exists", std::slice::from_ref(&a_ref)).unwrap(), Value::Bool(true));
    h.world.despawn(a);
    assert_eq!(h.run(&program, "exists", &[a_ref]).unwrap(), Value::Bool(false));
}

#[test]
fn component_properties() {
    let registry = NativeRegistry::with_engine_natives();
    let mut asm = Asm::new();
    let get = asm.import("Health::get_value", vec![Param::new(health())], Type::Float);
    let set = asm.import("Health::set_value", vec![Param::new(health()), Param::new(Type::Float)], Type::Unit);
    // c.value = c.value * 2
    asm.function("double", vec![health()], Type::Float, vec![Type::Float], vec![
        CallNative { import: get, args: vec![0], dst: Some(1) },
        Binary { op: pulsar_script_vm::BinOp::Add, dst: 1, a: 1, b: 1 },
        CallNative { import: set, args: vec![0, 1], dst: None },
        CallNative { import: get, args: vec![0], dst: Some(1) },
        Return { value: Some(1) },
    ]);
    let program = asm.link(&registry);

    let mut h = Harness::new();
    let e = spawn_health(&mut h, 21.0);
    h.world.subscribe::<Health>(e).unwrap();
    let out = h.run(&program, "double", &[Value::Component(ComponentRef::of::<Health>(e))]).unwrap();
    assert_eq!(out, float(42.0));
    assert_eq!(h.world.get::<Health>(e).unwrap().value, 42.0);
    // Property writes go through SceneDB's change hooks.
    assert_eq!(h.world.take_component_change_events().len(), 1);
}

#[test]
fn value_types_with_methods_and_fields() {
    let registry = NativeRegistry::with_engine_natives();
    let mut asm = Asm::new();
    let new = asm.import("V2::new", vec![Param::new(Type::Float), Param::new(Type::Float)], v2());
    let scale = asm.import("V2::scale", vec![Param::inout(v2()), Param::new(Type::Float)], Type::Unit);
    let length = asm.import("V2::length", vec![Param::new(v2())], Type::Float);
    let get_x = asm.import("V2::get_x", vec![Param::new(v2())], Type::Float);
    let set_y = asm.import("V2::set_y", vec![Param::inout(v2()), Param::new(Type::Float)], Type::Unit);
    let normalize = asm.import(
        "V2::normalize_into",
        vec![Param::new(v2()), Param::inout(v2())],
        Type::Unit,
    );
    // v = V2::new(a, b); w = v; v.scale(k); returns length(v) and leaves w untouched
    asm.function(
        "scaled_length",
        vec![Type::Float, Type::Float, Type::Float],
        Type::Float,
        vec![v2(), v2(), Type::Float],
        vec![
            CallNative { import: new, args: vec![0, 1], dst: Some(3) },
            Move { dst: 4, src: 3 },
            CallNative { import: scale, args: vec![3, 2], dst: None },
            CallNative { import: length, args: vec![3], dst: Some(5) },
            // w is an independent copy.
            CallNative { import: get_x, args: vec![4], dst: Some(0) },
            Binary { op: pulsar_script_vm::BinOp::Add, dst: 5, a: 5, b: 0 },
            Return { value: Some(5) },
        ],
    );
    asm.function("unit_x", vec![v2()], Type::Float, vec![v2(), Type::Float, Type::Float], vec![
        CallNative { import: normalize, args: vec![0, 1], dst: None },
        CallNative { import: set_y, args: vec![1, 2], dst: None },
        CallNative { import: get_x, args: vec![1], dst: Some(3) },
        Return { value: Some(3) },
    ]);
    let program = asm.link(&registry);

    let mut h = Harness::new();
    // |(3,4) * 2| = 10, plus the copy's x (3).
    assert_eq!(h.run(&program, "scaled_length", &[float(3.0), float(4.0), float(2.0)]).unwrap(), float(13.0));
    let v = Value::Object(Object::new("V2", V2 { x: 0.0, y: 5.0 }));
    assert_eq!(h.run(&program, "unit_x", &[v]).unwrap(), float(0.0));
}

#[test]
fn frontends_can_query_methods_by_reference_type() {
    let registry = NativeRegistry::with_engine_natives();
    let mut names: Vec<_> = registry.methods_for(&health()).map(|n| n.name.clone()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Health::damage",
            "Health::donate",
            "Health::entity",
            "Health::exists",
            "Health::fraction",
            "Health::get_max",
            "Health::get_value",
            "Health::set_max",
            "Health::set_value",
        ]
    );
    let donate = registry.get("Health::donate").unwrap();
    assert_eq!(donate.param_names, ["self", "other", "amount"]);
    assert_eq!(donate.doc, "Heal `other` by `amount`, taking it from this entity.");
    assert!(registry.get("Health::fraction").unwrap().flags.side_effect_free);
    // Static functions are global, not methods.
    assert!(registry.get("V2::new").unwrap().receiver.is_none());
    assert!(registry.get("Health::of").unwrap().receiver.is_none());
    assert_eq!(registry.get("V2::scale").unwrap().receiver, Some(v2()));
    // Everything is in the global list too.
    assert!(registry.functions().any(|n| n.name == "math::sin"));
}

#[test]
fn component_variables_default_to_an_unresolvable_reference() {
    let registry = NativeRegistry::with_engine_natives();
    let mut asm = Asm::new();
    let target = asm.var("target", health(), None);
    let exists = asm.import("Health::exists", vec![Param::new(health())], Type::Bool);
    asm.function("has_target", vec![], Type::Bool, vec![health(), Type::Bool], vec![
        LoadVar { dst: 0, var: target },
        CallNative { import: exists, args: vec![0], dst: Some(1) },
        Return { value: Some(1) },
    ]);
    let program = asm.link(&registry);
    let mut h = Harness::new();
    let mut instance = program.instantiate();
    assert_eq!(h.call(&program, &mut instance, "has_target", &[]).unwrap(), Value::Bool(false));

    let e = spawn_health(&mut h, 1.0);
    let index = program.variable("target").unwrap();
    program.set_var(&mut instance, index, Value::Component(ComponentRef::of::<Health>(e))).unwrap();
    assert_eq!(h.call(&program, &mut instance, "has_target", &[]).unwrap(), Value::Bool(true));
    // A reference to a different component type does not fit.
    assert!(program.set_var(&mut instance, index, Value::Component(ComponentRef::of::<V2>(e))).is_err());
}
