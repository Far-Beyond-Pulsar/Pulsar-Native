use super::*;
use crate::module::{BinOp, SourceLoc, UnOp};
use crate::native::NativeRegistry;

fn builder() -> (NativeRegistry, ()) {
    (NativeRegistry::with_engine_natives(), ())
}

fn one_function(
    module: &mut ModuleBuilder<'_>,
    name: &str,
    params: Vec<Type>,
    ret: Type,
) -> (FunctionHandle, FunctionBuilder) {
    let handle = module
        .declare_function(name, params, ret, true)
        .expect("declared");
    let f = module.begin_function(handle);
    (handle, f)
}

#[test]
fn constants_and_imports_are_deduplicated() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let a = module.constant(Constant::Int(7));
    let b = module.constant(Constant::Str("x".into()));
    assert_eq!(module.constant(Constant::Int(7)), a);
    assert_eq!(module.constant(Constant::Str("x".into())), b);
    assert_ne!(a, b);

    let first = module.import("math::sqrt").expect("a registered native");
    assert_eq!(module.import("math::sqrt"), Ok(first));
    assert_eq!(
        module.import("math::no_such"),
        Err(BuildError::UnknownNative("math::no_such".into()))
    );
}

#[test]
fn parameters_are_the_first_registers_and_registers_are_typed() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (_, mut f) = one_function(&mut module, "f", vec![Type::Int, Type::Str], Type::Unit);
    assert_eq!(f.param_reg(0), 0);
    assert_eq!(f.param_reg(1), 1);
    assert_eq!(f.reg_type(1), Ok(&Type::Str));
    let r = f.new_reg(Type::Float).expect("a register");
    assert_eq!(r, 2);
    assert_eq!(f.reg_type(r), Ok(&Type::Float));
    assert_eq!(f.reg_type(9), Err(BuildError::NoSuchRegister(9)));
}

#[test]
fn registers_run_out_with_an_error() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (_, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
    let mut result = Ok(0);
    for _ in 0..=usize::from(Reg::MAX) {
        result = f.new_reg(Type::Int);
        if result.is_err() {
            break;
        }
    }
    assert_eq!(result, Err(BuildError::TooManyRegisters));
}

#[test]
fn a_forward_jump_is_patched_when_its_label_is_bound() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (handle, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
    let end = f.new_label();
    let jump = f.jump(end);
    f.emit(Instr::Return { value: None });
    f.bind(end);
    f.ret(None);
    assert!(matches!(f.code[jump], Instr::Jump { target: 2 }));
    module.finish_function(handle, f).expect("finishes");
    module.finish().expect("verifies");
}

#[test]
fn a_backward_jump_to_a_bound_label_needs_no_patching() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (_, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
    f.ret(None);
    let top = f.label_here();
    let at = f.jump(top);
    assert!(matches!(f.code[at], Instr::Jump { target: 1 }));
}

#[test]
fn a_branch_patches_each_of_its_targets() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (handle, mut f) = one_function(&mut module, "f", vec![Type::Bool], Type::Unit);
    let (then, otherwise) = (f.new_label(), f.new_label());
    let cond = f.param_reg(0);
    let branch = f.branch(cond, then, otherwise);
    f.bind(then);
    f.ret(None);
    f.bind(otherwise);
    f.ret(None);
    assert!(matches!(
        f.code[branch],
        Instr::Branch {
            then: 1,
            otherwise: 2,
            ..
        }
    ));
    module.finish_function(handle, f).expect("finishes");
}

#[test]
fn loops_send_break_and_continue_to_their_labels() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (handle, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
    assert_eq!(f.break_jump(), Err(BuildError::NotInLoop("break")));
    assert_eq!(f.continue_jump(), Err(BuildError::NotInLoop("continue")));
    let (end, top) = (f.new_label(), f.label_here());
    f.enter_loop(end, top);
    assert!(f.in_loop());
    let brk = f.break_jump().expect("in a loop");
    let cont = f.continue_jump().expect("in a loop");
    f.exit_loop();
    f.jump(top);
    f.bind(end);
    f.ret(None);
    assert!(matches!(f.code[brk], Instr::Jump { target: 3 }));
    assert!(matches!(f.code[cont], Instr::Jump { target: 0 }));
    module.finish_function(handle, f).expect("finishes");
}

#[test]
fn an_unbound_label_an_open_loop_and_a_jump_past_the_end_are_refused() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);

    let (handle, mut f) = one_function(&mut module, "unbound", vec![], Type::Unit);
    let nowhere = f.new_label();
    f.jump(nowhere);
    f.ret(None);
    assert!(matches!(
        module.finish_function(handle, f),
        Err(BuildError::UnboundLabel { .. })
    ));

    let (handle, mut f) = one_function(&mut module, "open", vec![], Type::Unit);
    let (a, b) = (f.label_here(), f.label_here());
    f.enter_loop(a, b);
    f.ret(None);
    assert!(matches!(
        module.finish_function(handle, f),
        Err(BuildError::UnclosedLoop { .. })
    ));

    let (handle, mut f) = one_function(&mut module, "past", vec![], Type::Unit);
    let end = f.new_label();
    f.jump(end);
    f.bind(end);
    assert!(matches!(
        module.finish_function(handle, f),
        Err(BuildError::JumpPastEnd { pc: 0, .. })
    ));

    let (handle, mut f) = one_function(&mut module, "twice", vec![], Type::Unit);
    let label = f.label_here();
    f.bind(label);
    f.ret(None);
    assert!(matches!(
        module.finish_function(handle, f),
        Err(BuildError::LabelBoundTwice { .. })
    ));
}

#[test]
fn unreachable_gives_a_function_that_always_returns_an_end() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (handle, mut f) = one_function(&mut module, "f", vec![Type::Bool], Type::Int);
    let (yes, no, end) = (f.new_label(), f.new_label(), f.new_label());
    let cond = f.param_reg(0);
    f.branch(cond, yes, no);
    f.bind(yes);
    let (one, _) = module
        .load_constant(&mut f, Constant::Int(1))
        .expect("loads");
    f.ret(Some(one));
    f.jump(end);
    f.bind(no);
    let (two, _) = module
        .load_constant(&mut f, Constant::Int(2))
        .expect("loads");
    f.ret(Some(two));
    f.bind(end);
    f.unreachable();
    module.finish_function(handle, f).expect("finishes");
    module.finish().expect("verifies");
}

#[test]
fn native_calls_are_checked_against_the_registry_signature() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (_, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
    let (x, _) = module
        .load_constant(&mut f, Constant::Float(4.0))
        .expect("loads");
    let (s, _) = module
        .load_constant(&mut f, Constant::Str("s".into()))
        .expect("loads");
    let root = module
        .call_native(&mut f, "math::sqrt", &[x])
        .expect("a good call")
        .expect("a result");
    assert_eq!(f.reg_type(root), Ok(&Type::Float));
    assert!(matches!(
        module.call_native(&mut f, "math::sqrt", &[]),
        Err(BuildError::ArgumentCount {
            expected: 1,
            found: 0,
            ..
        })
    ));
    assert!(matches!(
        module.call_native(&mut f, "math::sqrt", &[s]),
        Err(BuildError::ArgumentType { index: 0, .. })
    ));
    assert!(matches!(
        module.call_native(&mut f, "math::nope", &[x]),
        Err(BuildError::UnknownNative(_))
    ));
    assert_eq!(module.imports.len(), 1);
}

#[test]
fn module_calls_are_checked_and_can_name_a_later_function() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let caller = module
        .declare_function("caller", vec![], Type::Int, true)
        .expect("declared");
    let callee = module
        .declare_function("callee", vec![Type::Int], Type::Int, false)
        .expect("declared");
    assert_eq!(
        module.declare_function("callee", vec![], Type::Unit, false),
        Err(BuildError::Duplicate {
            kind: "function",
            name: "callee".into()
        })
    );
    assert_eq!(module.function("callee"), Some(callee));
    assert_eq!(module.signature(callee).0, &[Type::Int]);

    let mut f = module.begin_function(caller);
    let (n, _) = module
        .load_constant(&mut f, Constant::Int(2))
        .expect("loads");
    assert!(matches!(
        module.call(&mut f, callee, &[]),
        Err(BuildError::ArgumentCount { .. })
    ));
    let result = module
        .call(&mut f, callee, &[n])
        .expect("a good call")
        .expect("a result");
    f.ret(Some(result));
    module.finish_function(caller, f).expect("finishes");

    // `finish` refuses a function with no body.
    assert_eq!(
        module.finish().err(),
        Some(BuildError::FunctionNotDefined("callee".into()))
    );
}

#[test]
fn variables_have_stable_ids_and_typed_access() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    module.set_class_version(3);
    let count = module
        .add_variable(
            "count",
            Type::Int,
            Some(Constant::Int(5)),
            Some("id-1".into()),
        )
        .expect("added");
    assert_eq!(
        module.add_variable("count", Type::Int, None, None),
        Err(BuildError::Duplicate {
            kind: "variable",
            name: "count".into()
        })
    );
    assert_eq!(module.variable("count"), Some((count, &Type::Int)));

    let get = module
        .declare_function("get", vec![], Type::Int, true)
        .expect("declared");
    let mut f = module.begin_function(get);
    let value = module.load_var(&mut f, count).expect("loads");
    let (text, _) = module
        .load_constant(&mut f, Constant::Str("x".into()))
        .expect("loads");
    assert!(matches!(
        module.store_var(&mut f, count, text),
        Err(BuildError::VariableType { .. })
    ));
    module.store_var(&mut f, count, value).expect("stores");
    f.ret(Some(value));
    module.finish_function(get, f).expect("finishes");

    let module = module.finish().expect("verifies");
    assert_eq!(module.class_version, 3);
    assert_eq!(module.variables[0].id.as_deref(), Some("id-1"));
    assert_eq!(module.variables[0].default, Some(Constant::Int(5)));
}

#[test]
fn events_and_subscriptions_are_recorded() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let event = EventDecl {
        name: "T.Ping".into(),
        fields: vec![crate::module::EventField::new("n", Type::Int)],
    };
    module.declare_event(event.clone()).expect("declared");
    assert!(matches!(
        module.declare_event(event),
        Err(BuildError::Duplicate { kind: "event", .. })
    ));
    let on_ping = module
        .declare_function("on_ping", vec![Type::Int], Type::Unit, true)
        .expect("declared");
    let mut f = module.begin_function(on_ping);
    f.ret(None);
    module.finish_function(on_ping, f).expect("finishes");
    module.subscribe(
        EventRef::Name("T.Ping".into()),
        on_ping,
        SubscriptionScope::Self_,
    );
    let module = module.finish().expect("verifies");
    assert_eq!(module.events.len(), 1);
    assert_eq!(module.subscriptions[0].handler, on_ping.index());
}

#[test]
fn debug_info_follows_the_current_location() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let handle = module
        .declare_function("f", vec![], Type::Unit, true)
        .expect("declared");
    let mut f2 = module.begin_function(handle);
    let a = SourceLoc {
        file: "a.x".into(),
        line: Some(1),
        column: Some(1),
        ..SourceLoc::default()
    };
    let b = SourceLoc {
        line: Some(2),
        ..a.clone()
    };
    let t = f2.new_reg(Type::Float).expect("a register");
    f2.set_loc(a.clone());
    f2.emit(Instr::Now { dst: t });
    f2.emit(Instr::Now { dst: t });
    f2.set_loc(b.clone());
    f2.ret(None);
    module.finish_function(handle, f2).expect("finishes");
    let module = module.finish().expect("verifies");
    let function = &module.functions[0];
    assert_eq!(function.location(0), Some(&a));
    assert_eq!(function.location(1), Some(&a));
    assert_eq!(function.location(2), Some(&b));
    assert_eq!(function.debug.as_ref().expect("debug").ranges.len(), 2);
}

#[test]
fn a_function_without_locations_has_no_debug_info() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (handle, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
    f.ret(None);
    module.finish_function(handle, f).expect("finishes");
    assert!(module.finish().expect("verifies").functions[0]
        .debug
        .is_none());
}

#[test]
fn scopes_shadow_and_close() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (_, mut f) = one_function(&mut module, "f", vec![Type::Int], Type::Unit);
    f.bind_local("p", f.param_reg(0), Type::Int, true);
    assert!(f.declared_in_scope("p"));
    f.push_scope();
    assert!(!f.declared_in_scope("p"));
    let inner = f.declare_local("p", Type::Str, false).expect("declared");
    assert_eq!(
        f.lookup("p").map(|l| (l.reg, l.mutable)),
        Some((inner, false))
    );
    f.pop_scope();
    assert_eq!(f.lookup("p").map(|l| l.ty.clone()), Some(Type::Int));
    assert!(f.lookup("q").is_none());
}

#[test]
fn operand_rules_are_the_verifiers() {
    assert_eq!(binary_result(BinOp::Add, &Type::Str), Some(Type::Str));
    assert_eq!(binary_result(BinOp::Sub, &Type::Str), None);
    assert_eq!(binary_result(BinOp::Lt, &Type::Float), Some(Type::Bool));
    assert_eq!(binary_result(BinOp::Eq, &Type::Entity), Some(Type::Bool));
    assert_eq!(binary_result(BinOp::And, &Type::Bool), Some(Type::Bool));
    assert_eq!(binary_result(BinOp::And, &Type::Int), None);
    assert_eq!(unary_result(UnOp::Neg, &Type::Float), Some(Type::Float));
    assert_eq!(unary_result(UnOp::Not, &Type::Int), None);
    assert_eq!(unary_result(UnOp::ToStr, &Type::Bool), Some(Type::Str));
}

#[test]
fn typed_unary_and_binary_helpers_allocate_the_result() {
    let (natives, _) = builder();
    let mut module = ModuleBuilder::new("T", &natives);
    let (_, mut f) = one_function(
        &mut module,
        "f",
        vec![Type::Int, Type::Int, Type::Str],
        Type::Unit,
    );
    let (a, b, s) = (f.param_reg(0), f.param_reg(1), f.param_reg(2));
    let sum = f.binary(BinOp::Add, a, b).expect("ok").expect("applies");
    assert_eq!(f.reg_type(sum), Ok(&Type::Int));
    let lt = f.binary(BinOp::Lt, a, b).expect("ok").expect("applies");
    assert_eq!(f.reg_type(lt), Ok(&Type::Bool));
    assert_eq!(f.binary(BinOp::Add, a, s), Ok(None));
    let wide = f.unary(UnOp::IntToFloat, a).expect("ok").expect("applies");
    assert_eq!(f.reg_type(wide), Ok(&Type::Float));
    assert_eq!(f.unary(UnOp::Not, a), Ok(None));
}

#[test]
fn engine_types_resolve_by_name() {
    let (natives, _) = builder();
    let module = ModuleBuilder::new("T", &natives);
    assert_eq!(module.engine_type("No_Such_Type"), None);
}
