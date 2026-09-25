//! Declared events, subscriptions and the `event::*` natives (#924).

mod common;

use std::sync::{Arc, Mutex};

use common::Asm;
use pulsar_scenedb::Entity;
use pulsar_script_vm::{
    Budget, Constant, EventCatalog, EventDecl, EventField, EventRef, EventSignature, EventSink,
    EventTarget, Host, Instr, LinkError, Module, NativeRegistry, Param, Program, Subscription,
    SubscriptionScope, Type, Value, Vm, FORMAT_VERSION,
};

use Instr::*;

struct Catalog(Vec<EventSignature>);

impl EventCatalog for Catalog {
    fn event_by_name(&self, name: &str) -> Option<EventSignature> {
        self.0.iter().find(|e| e.name == name).cloned()
    }
    fn event_by_id(&self, id: u64) -> Option<EventSignature> {
        self.0.iter().find(|e| e.id == id).cloned()
    }
}

fn hit() -> EventSignature {
    EventSignature {
        id: 42,
        name: "Hit".into(),
        fields: vec![
            EventField::new("entity", Type::Entity),
            EventField::new("other", Type::Entity),
            EventField::new("impulse", Type::Float),
        ],
    }
}

fn handler(asm: &mut Asm, name: &str, params: Vec<Type>) -> u32 {
    asm.function(name, params, Type::Unit, vec![], vec![Return { value: None }])
}

fn subscribe(asm: &mut Asm, event: &str, handler: u32, scope: SubscriptionScope) {
    asm.module.subscriptions.push(Subscription { event: EventRef::Name(event.into()), handler, scope });
}

fn link(asm: &Asm, catalog: Option<&dyn EventCatalog>) -> Result<Program, LinkError> {
    Program::link_with_events(Arc::new(asm.module.clone()), &NativeRegistry::with_engine_natives(), catalog)
}

#[test]
fn handlers_are_checked_against_the_catalog() {
    let catalog = Catalog(vec![hit()]);
    let mut asm = Asm::new();
    let full = handler(&mut asm, "on_hit", vec![Type::Entity, Type::Entity, Type::Float]);
    let prefix = handler(&mut asm, "on_hit_short", vec![Type::Entity]);
    let none = handler(&mut asm, "on_hit_none", vec![]);
    subscribe(&mut asm, "Hit", full, SubscriptionScope::Self_);
    subscribe(&mut asm, "Hit", prefix, SubscriptionScope::Global);
    subscribe(&mut asm, "Hit", none, SubscriptionScope::Class);
    let program = link(&asm, Some(&catalog)).expect("links");
    let subs = program.subscriptions();
    assert_eq!(subs.len(), 3);
    assert_eq!(subs[0].event_id, Some(42));
    assert_eq!(subs[0].event_name.as_deref(), Some("Hit"));
    assert_eq!(subs[0].params, 3);
    assert_eq!(subs[1].params, 1);
    assert_eq!(subs[2].scope, SubscriptionScope::Class);

    // Wrong types, too many parameters, unknown events.
    let mut asm = Asm::new();
    let bad = handler(&mut asm, "on_hit", vec![Type::Float]);
    subscribe(&mut asm, "Hit", bad, SubscriptionScope::Self_);
    let err = link(&asm, Some(&catalog)).err().expect("rejected");
    assert!(matches!(err, LinkError::HandlerMismatch { .. }), "{err}");
    assert!(err.to_string().contains("parameter 0 is float"), "{err}");

    let mut asm = Asm::new();
    let bad = handler(&mut asm, "on_hit", vec![Type::Entity, Type::Entity, Type::Float, Type::Int]);
    subscribe(&mut asm, "Hit", bad, SubscriptionScope::Self_);
    assert!(link(&asm, Some(&catalog)).err().unwrap().to_string().contains("takes 4 parameters"));

    let mut asm = Asm::new();
    let h = handler(&mut asm, "on_nope", vec![]);
    subscribe(&mut asm, "Nope", h, SubscriptionScope::Global);
    assert!(matches!(link(&asm, Some(&catalog)), Err(LinkError::UnknownEvent { .. })));
    // Without a catalog, events the module does not declare are checked later.
    assert!(link(&asm, None).is_ok());
}

#[test]
fn declared_events_are_verified_locally() {
    let mut asm = Asm::new();
    asm.module.events.push(EventDecl {
        name: "Door.Opened".into(),
        fields: vec![EventField::new("by", Type::Entity), EventField::new("code", Type::Int)],
    });
    let good = handler(&mut asm, "on_open", vec![Type::Entity, Type::Int]);
    subscribe(&mut asm, "Door.Opened", good, SubscriptionScope::Self_);
    let program = link(&asm, None).expect("links without a catalog");
    assert_eq!(program.subscriptions()[0].event_id, None, "ids come from the engine");

    let bad = handler(&mut asm, "on_open_bad", vec![Type::Int]);
    subscribe(&mut asm, "Door.Opened", bad, SubscriptionScope::Self_);
    let err = link(&asm, None).err().unwrap().to_string();
    assert!(err.contains("on_open_bad") && err.contains("parameter 0 is int"), "{err}");

    // Bad declarations.
    let mut asm = Asm::new();
    asm.module.events.push(EventDecl { name: "E".into(), fields: vec![EventField::new("v", Type::object("Vec3"))] });
    assert!(link(&asm, None).err().unwrap().to_string().contains("event fields are"));
    let mut asm = Asm::new();
    asm.module.events.push(EventDecl { name: "E".into(), fields: vec![] });
    asm.module.events.push(EventDecl { name: "E".into(), fields: vec![] });
    assert!(link(&asm, None).err().unwrap().to_string().contains("declared twice"));

    // A handler must return unit and be in range.
    let mut asm = Asm::new();
    let f = asm.function("f", vec![], Type::Int, vec![Type::Int], vec![Const { dst: 0, index: 0 }, Return { value: Some(0) }]);
    asm.constant(Constant::Int(1));
    subscribe(&mut asm, "Hit", f, SubscriptionScope::Global);
    assert!(link(&asm, None).err().unwrap().to_string().contains("must return unit"));
    let mut asm = Asm::new();
    subscribe(&mut asm, "Hit", 9, SubscriptionScope::Global);
    assert!(link(&asm, None).err().unwrap().to_string().contains("out of range"));
}

#[test]
fn format_v1_modules_still_load_and_v2_round_trips() {
    let mut v1 = Module::new("old");
    v1.format_version = 1;
    let json = v1.to_json().unwrap();
    assert!(!json.contains("subscriptions"), "empty tables are not written");
    let back = Module::from_json(&json).unwrap();
    assert!(Program::link(Arc::new(back), &NativeRegistry::new()).is_ok());

    let mut asm = Asm::new();
    asm.module.events.push(EventDecl { name: "E".into(), fields: vec![EventField::new("x", Type::Float)] });
    let h = handler(&mut asm, "on_e", vec![Type::Float]);
    subscribe(&mut asm, "E", h, SubscriptionScope::Self_);
    assert_eq!(asm.module.format_version, FORMAT_VERSION);
    let json = asm.module.to_json().unwrap();
    assert!(json.contains("\"scope\": \"Self\""), "{json}");
    assert_eq!(Module::from_json(&json).unwrap(), asm.module);
    // Scope defaults to global.
    let json = json.replace(",\n      \"scope\": \"Self\"", "");
    assert_eq!(Module::from_json(&json).unwrap().subscriptions[0].scope, SubscriptionScope::Global);

    let mut future = Module::new("future");
    future.format_version = FORMAT_VERSION + 1;
    assert!(Program::link(Arc::new(future), &NativeRegistry::new()).is_err());
}

#[derive(Default)]
struct Recorder(Mutex<Vec<(EventTarget, String, Vec<Value>)>>);

impl EventSink for Recorder {
    fn emit(&self, target: EventTarget, name: &str, fields: &[Value]) -> Result<(), String> {
        if name == "Bad" {
            return Err("fields do not match `Bad`".into());
        }
        self.0.lock().unwrap().push((target, name.into(), fields.to_vec()));
        Ok(())
    }
}

#[test]
fn event_natives_are_polymorphic_and_fail_cleanly() {
    let mut asm = Asm::new();
    let emit = asm.import("event::emit", vec![Param::new(Type::Str), Param::new(Type::Int), Param::new(Type::Bool)], Type::Unit);
    let send = asm.import("event::send", vec![Param::new(Type::Entity), Param::new(Type::Str), Param::new(Type::Float)], Type::Unit);
    let class = asm.import("event::emit_to_class", vec![Param::new(Type::Str), Param::new(Type::Str)], Type::Unit);
    let name = asm.constant(Constant::Str("Ping".into()));
    let bad = asm.constant(Constant::Str("Bad".into()));
    let klass = asm.constant(Constant::Str("Enemy".into()));
    let seven = asm.constant(Constant::Int(7));
    let yes = asm.constant(Constant::Bool(true));
    let half = asm.constant(Constant::Float(0.5));
    asm.function(
        "go",
        vec![],
        Type::Unit,
        vec![Type::Str, Type::Int, Type::Bool, Type::Entity, Type::Float, Type::Str],
        vec![
            Const { dst: 0, index: name },
            Const { dst: 1, index: seven },
            Const { dst: 2, index: yes },
            CallNative { import: emit, args: vec![0, 1, 2], dst: None },
            SelfEntity { dst: 3 },
            Const { dst: 4, index: half },
            CallNative { import: send, args: vec![3, 0, 4], dst: None },
            Const { dst: 5, index: klass },
            CallNative { import: class, args: vec![5, 0], dst: None },
            Return { value: None },
        ],
    );
    asm.function(
        "bad",
        vec![],
        Type::Unit,
        vec![Type::Str, Type::Int, Type::Bool],
        vec![
            Const { dst: 0, index: bad },
            Const { dst: 1, index: seven },
            Const { dst: 2, index: yes },
            CallNative { import: emit, args: vec![0, 1, 2], dst: None },
            Return { value: None },
        ],
    );
    let program = asm.link(&NativeRegistry::with_engine_natives());
    let mut world = pulsar_scenedb::World::new();
    let me = world.spawn();
    let recorder = Recorder::default();
    let mut vm = Vm::new();
    let mut instance = program.instantiate();
    let mut host = Host::new(&mut world, me).with_events(Some(&recorder));
    vm.call(&program, &mut instance, program.entry("go").unwrap(), &[], &mut host, &mut Budget::new(1000)).unwrap();
    let got = recorder.0.lock().unwrap().clone();
    assert_eq!(got.len(), 3);
    assert_eq!(got[0].0, EventTarget::Global);
    assert_eq!(got[0].2, vec![Value::Int(7), Value::Bool(true)]);
    assert_eq!(got[1].0, EventTarget::Entity(me));
    assert_eq!(got[1].2, vec![Value::Float(0.5)]);
    assert_eq!(got[2].0, EventTarget::Class("Enemy".into()));
    assert!(got[2].2.is_empty());

    // A refused call is a script error.
    let mut host = Host::new(&mut world, me).with_events(Some(&recorder));
    let err = vm.call(&program, &mut instance, program.entry("bad").unwrap(), &[], &mut host, &mut Budget::new(1000)).unwrap_err();
    assert!(err.to_string().contains("event::emit") && err.to_string().contains("do not match"), "{err}");
    // No hub: also a script error.
    let mut host = Host::new(&mut world, Entity::DANGLING);
    let err = vm.call(&program, &mut instance, program.entry("go").unwrap(), &[], &mut host, &mut Budget::new(1000)).unwrap_err();
    assert!(err.to_string().contains("no event hub"), "{err}");
}

#[test]
fn poly_natives_reject_bad_import_signatures() {
    let registry = NativeRegistry::with_engine_natives();
    for (params, ret) in [
        (vec![Param::new(Type::Int)], Type::Unit),                              // fixed part wrong
        (vec![Param::new(Type::Str)], Type::Int),                               // return type
        (vec![Param::new(Type::Str), Param::new(Type::Unit)], Type::Unit), // not a field type
        (vec![Param::new(Type::Str), Param::inout(Type::Int)], Type::Unit),       // inout
    ] {
        let mut asm = Asm::new();
        asm.import("event::emit", params, ret);
        let err = Program::link(Arc::new(asm.module.clone()), &registry).err().expect("rejected");
        assert!(matches!(err, LinkError::PolyNative { .. }), "{err}");
    }
}
