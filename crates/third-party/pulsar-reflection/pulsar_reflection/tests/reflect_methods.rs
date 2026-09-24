use std::any::{Any, TypeId};
use std::fmt;

use pulsar_reflection::{
    find_method, methods_for, reflect_methods, CallError, PassMode, Receiver, ReceiverKind,
};

#[derive(Debug, Default, PartialEq)]
struct Health {
    value: f32,
    log: Vec<String>,
}

#[derive(Debug)]
struct Dead;

impl fmt::Display for Dead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("already dead")
    }
}

#[reflect_methods]
impl Health {
    /// Remaining hit points.
    #[reflect_method(pure, category = "Combat")]
    pub fn current(&self) -> f32 {
        self.value
    }

    #[reflect_method]
    pub fn damage(&mut self, amount: f32) {
        self.value -= amount;
    }

    #[reflect_method(name = "make")]
    pub fn with_value(value: f32) -> Health {
        Health {
            value,
            log: Vec::new(),
        }
    }

    #[reflect_method]
    pub fn note(&mut self, message: &str, tags: &[String]) -> usize {
        self.log.push(format!("{message} {}", tags.join(",")));
        self.log.len()
    }

    #[reflect_method]
    pub fn take_log(&mut self, out: &mut Vec<String>) {
        out.append(&mut self.log);
    }

    #[reflect_method]
    pub fn heal(&mut self, amount: f32) -> Result<f32, Dead> {
        if self.value <= 0.0 {
            return Err(Dead);
        }
        self.value += amount;
        Ok(self.value)
    }

    /// Not marked: must not be registered.
    #[allow(dead_code)]
    pub fn hidden(&self) {}
}

// A second block for the same type merges into the same entry.
#[reflect_methods]
impl Health {
    #[reflect_method(deterministic)]
    fn is_alive(&self) -> bool {
        self.value > 0.0
    }
}

fn args(values: Vec<Box<dyn Any>>) -> Vec<Box<dyn Any>> {
    values
}

#[test]
fn registers_marked_methods_by_type_id() {
    let mut names: Vec<_> = methods_for::<Health>().iter().map(|m| m.name()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["current", "damage", "heal", "is_alive", "make", "note", "take_log"]
    );
    assert!(pulsar_reflection::methods_of(TypeId::of::<Dead>()).is_empty());
}

#[test]
fn signature_metadata() {
    let current = find_method(TypeId::of::<Health>(), "current").unwrap();
    assert_eq!(current.receiver, ReceiverKind::Ref);
    assert!(current.info.flags.side_effect_free && current.info.flags.deterministic);
    assert_eq!(current.info.attr("category"), Some("Combat"));
    assert_eq!(current.info.doc, "Remaining hit points.");
    assert!(current.info.ret.unwrap().is::<f32>());

    let damage = find_method(TypeId::of::<Health>(), "damage").unwrap();
    assert_eq!(damage.receiver, ReceiverKind::Mut);
    assert!(damage.info.ret.is_none());
    assert_eq!(damage.info.params[0].name, "amount");
    assert!(damage.info.params[0].ty.is::<f32>());

    let make = find_method(TypeId::of::<Health>(), "make").unwrap();
    assert_eq!(make.receiver, ReceiverKind::None);

    let note = find_method(TypeId::of::<Health>(), "note").unwrap();
    assert!(note.info.params[0].ty.is::<String>());
    assert_eq!(note.info.params[0].mode, PassMode::Ref);
    assert!(note.info.params[1].ty.is::<Vec<String>>());

    let heal = find_method(TypeId::of::<Health>(), "heal").unwrap();
    assert!(heal.info.ret.unwrap().is::<f32>());

    let alive = find_method(TypeId::of::<Health>(), "is_alive").unwrap();
    assert!(alive.info.flags.deterministic && !alive.info.flags.side_effect_free);
}

#[test]
fn invoke_calls_through() {
    let mut health = Health {
        value: 10.0,
        log: Vec::new(),
    };
    let damage = find_method(TypeId::of::<Health>(), "damage").unwrap();
    let out = damage
        .call(
            Receiver::Mut(&mut health),
            &mut args(vec![Box::new(3.0f32)]),
        )
        .unwrap();
    assert!(out.is_none());
    assert_eq!(health.value, 7.0);

    // `&self` methods accept a mutable receiver too.
    let current = find_method(TypeId::of::<Health>(), "current").unwrap();
    let out = current
        .call(Receiver::Ref(&health), &mut [])
        .unwrap()
        .unwrap();
    assert_eq!(*out.downcast::<f32>().unwrap(), 7.0);
    let out = current
        .call(Receiver::Mut(&mut health), &mut [])
        .unwrap()
        .unwrap();
    assert_eq!(*out.downcast::<f32>().unwrap(), 7.0);

    let make = find_method(TypeId::of::<Health>(), "make").unwrap();
    let out = make
        .call(Receiver::None, &mut args(vec![Box::new(5.0f32)]))
        .unwrap()
        .unwrap();
    assert_eq!(out.downcast::<Health>().unwrap().value, 5.0);
}

#[test]
fn borrowed_and_out_params() {
    let mut health = Health::default();
    let note = find_method(TypeId::of::<Health>(), "note").unwrap();
    let mut call_args = args(vec![
        Box::new("hit".to_string()),
        Box::new(vec!["a".to_string(), "b".to_string()]),
    ]);
    let out = note
        .call(Receiver::Mut(&mut health), &mut call_args)
        .unwrap()
        .unwrap();
    assert_eq!(*out.downcast::<usize>().unwrap(), 1);
    // Borrowed arguments stay in their slots.
    assert_eq!(call_args[0].downcast_ref::<String>().unwrap(), "hit");

    let take_log = find_method(TypeId::of::<Health>(), "take_log").unwrap();
    let mut call_args = args(vec![Box::new(Vec::<String>::new())]);
    take_log
        .call(Receiver::Mut(&mut health), &mut call_args)
        .unwrap();
    assert_eq!(
        call_args[0].downcast_ref::<Vec<String>>().unwrap(),
        &["hit a,b".to_string()]
    );
    assert!(health.log.is_empty());
}

#[test]
fn fallible_methods_report_err() {
    let heal = find_method(TypeId::of::<Health>(), "heal").unwrap();
    let mut dead = Health::default();
    let err = heal
        .call(Receiver::Mut(&mut dead), &mut args(vec![Box::new(1.0f32)]))
        .unwrap_err();
    assert_eq!(err, CallError::Failed("already dead".into()));

    let mut alive = Health {
        value: 1.0,
        log: Vec::new(),
    };
    let out = heal
        .call(Receiver::Mut(&mut alive), &mut args(vec![Box::new(1.0f32)]))
        .unwrap();
    assert_eq!(*out.unwrap().downcast::<f32>().unwrap(), 2.0);
}

#[test]
fn rejects_bad_calls_without_side_effects() {
    let mut health = Health {
        value: 10.0,
        log: Vec::new(),
    };
    let damage = find_method(TypeId::of::<Health>(), "damage").unwrap();

    let err = damage
        .call(Receiver::Mut(&mut health), &mut [])
        .unwrap_err();
    assert_eq!(
        err,
        CallError::ArgCount {
            expected: 1,
            found: 0
        }
    );

    let mut wrong = args(vec![Box::new(3.0f64)]);
    let err = damage
        .call(Receiver::Mut(&mut health), &mut wrong)
        .unwrap_err();
    assert!(matches!(err, CallError::ArgType { index: 0, .. }));
    // A rejected by-value argument is not consumed.
    assert!(wrong[0].is::<f64>());

    let err = damage
        .call(Receiver::Ref(&health), &mut args(vec![Box::new(1.0f32)]))
        .unwrap_err();
    assert_eq!(
        err,
        CallError::ReceiverMissing {
            needed: ReceiverKind::Mut
        }
    );

    let mut not_health = Dead;
    let err = damage
        .call(
            Receiver::Mut(&mut not_health),
            &mut args(vec![Box::new(1.0f32)]),
        )
        .unwrap_err();
    assert!(matches!(err, CallError::ReceiverType { .. }));

    assert_eq!(health.value, 10.0);
}
