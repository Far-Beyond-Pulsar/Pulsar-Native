//! Hand-assembled fixture modules. Used by `build.rs` (to generate Rust for
//! them) and by the tests (to interpret and drive them), so this file may use
//! only `pulsar_script_vm`.

use pulsar_script_vm::{
    BinOp, CollOp, Constant, DebugInfo, DebugRange, EventDecl, EventField, EventRef, Function, Import, Instr, Module, Param,
    Signature, SourceLoc, Subscription, SubscriptionScope, Type, UnOp, Variable,
};

use Instr::*;

fn func(name: &str, params: Vec<Type>, ret: Type, extra: Vec<Type>, code: Vec<Instr>) -> Function {
    let mut registers = params.clone();
    registers.extend(extra);
    Function { name: name.into(), exported: true, params, ret, registers, code, debug: None }
}

fn var(name: &str, ty: Type, default: Option<Constant>) -> Variable {
    Variable { name: name.into(), ty, default, id: None }
}

fn import(name: &str, params: Vec<Param>, ret: Type) -> Import {
    Import { name: name.into(), sig: Signature::new(params, ret) }
}

/// Every module that is written by hand. JSON fixtures (compiled from
/// Blueprint graphs) are added by the caller.
pub fn hand_written() -> Vec<Module> {
    vec![arith(), flow(), waits(), natives(), events_decl(), collections()]
}

/// Binary and unary operators on every type they accept.
fn arith() -> Module {
    let mut m = Module::new("arith");
    let int2 = || vec![Type::Int, Type::Int];
    let float2 = || vec![Type::Float, Type::Float];
    let binary = |name: &str, params: Vec<Type>, ret: Type, op: BinOp| {
        let result = ret.clone();
        func(name, params, ret, vec![result], vec![Binary { op, dst: 2, a: 0, b: 1 }, Return { value: Some(2) }])
    };
    for (name, op) in [("add", BinOp::Add), ("sub", BinOp::Sub), ("mul", BinOp::Mul), ("div", BinOp::Div), ("rem", BinOp::Rem)] {
        m.functions.push(binary(&format!("int_{name}"), int2(), Type::Int, op));
        m.functions.push(binary(&format!("float_{name}"), float2(), Type::Float, op));
    }
    for (name, op) in [("lt", BinOp::Lt), ("le", BinOp::Le), ("gt", BinOp::Gt), ("ge", BinOp::Ge), ("eq", BinOp::Eq), ("ne", BinOp::Ne)] {
        m.functions.push(binary(&format!("int_{name}"), int2(), Type::Bool, op));
        m.functions.push(binary(&format!("float_{name}"), float2(), Type::Bool, op));
        m.functions.push(binary(&format!("str_{name}"), vec![Type::Str, Type::Str], Type::Bool, op));
    }
    m.functions.push(binary("concat", vec![Type::Str, Type::Str], Type::Str, BinOp::Add));
    m.functions.push(binary("and", vec![Type::Bool, Type::Bool], Type::Bool, BinOp::And));
    m.functions.push(binary("or", vec![Type::Bool, Type::Bool], Type::Bool, BinOp::Or));
    let unary = |name: &str, from: Type, to: Type, op: UnOp| {
        let result = to.clone();
        func(name, vec![from], to, vec![result], vec![Unary { op, dst: 1, src: 0 }, Return { value: Some(1) }])
    };
    m.functions.push(unary("int_neg", Type::Int, Type::Int, UnOp::Neg));
    m.functions.push(unary("float_neg", Type::Float, Type::Float, UnOp::Neg));
    m.functions.push(unary("not", Type::Bool, Type::Bool, UnOp::Not));
    m.functions.push(unary("int_to_float", Type::Int, Type::Float, UnOp::IntToFloat));
    m.functions.push(unary("float_to_int", Type::Float, Type::Int, UnOp::FloatToInt));
    m.functions.push(unary("int_to_str", Type::Int, Type::Str, UnOp::ToStr));
    m.functions.push(unary("float_to_str", Type::Float, Type::Str, UnOp::ToStr));
    m.functions.push(unary("bool_to_str", Type::Bool, Type::Str, UnOp::ToStr));
    m.functions.push(unary("str_to_str", Type::Str, Type::Str, UnOp::ToStr));

    // A function with debug info, so error traces carry source locations.
    let mut located = binary("located_div", int2(), Type::Int, BinOp::Div);
    let loc = |node: &str| SourceLoc { file: "graph.json".into(), node: node.into(), line: None, column: None };
    located.debug = Some(DebugInfo {
        ranges: vec![
            DebugRange { start: 0, end: 1, loc: loc("divide-node") },
            DebugRange { start: 1, end: 2, loc: loc("return-node") },
        ],
    });
    m.functions.push(located);
    m
}

/// Loops, branches, calls, recursion, and the limits that stop runaway code.
fn flow() -> Module {
    let mut m = Module::new("flow");
    m.constants = vec![
        Constant::Int(0),
        Constant::Int(1),
        Constant::Int(2),
        Constant::Str("negative".into()),
        Constant::Str("zero".into()),
        Constant::Str("positive".into()),
    ];
    // 0: sum_to(n) = 1 + 2 + .. + n
    m.functions.push(func(
        "sum_to",
        vec![Type::Int],
        Type::Int,
        vec![Type::Int, Type::Int, Type::Int, Type::Bool],
        vec![
            Const { dst: 1, index: 0 },
            Const { dst: 2, index: 1 },
            Const { dst: 3, index: 1 },
            Binary { op: BinOp::Le, dst: 4, a: 2, b: 0 },
            Branch { cond: 4, then: 5, otherwise: 8 },
            Binary { op: BinOp::Add, dst: 1, a: 1, b: 2 },
            Binary { op: BinOp::Add, dst: 2, a: 2, b: 3 },
            Jump { target: 3 },
            Return { value: Some(1) },
        ],
    ));
    // 1: fib(n), by recursion
    m.functions.push(func(
        "fib",
        vec![Type::Int],
        Type::Int,
        vec![Type::Int, Type::Bool, Type::Int, Type::Int, Type::Int, Type::Int, Type::Int, Type::Int],
        vec![
            Const { dst: 7, index: 1 },
            Const { dst: 1, index: 2 },
            Binary { op: BinOp::Lt, dst: 2, a: 0, b: 1 },
            Branch { cond: 2, then: 4, otherwise: 5 },
            Return { value: Some(0) },
            Binary { op: BinOp::Sub, dst: 3, a: 0, b: 7 },
            Call { func: 1, args: vec![3], dst: Some(5) },
            Binary { op: BinOp::Sub, dst: 4, a: 0, b: 1 },
            Call { func: 1, args: vec![4], dst: Some(6) },
            Binary { op: BinOp::Add, dst: 3, a: 5, b: 6 },
            Return { value: Some(3) },
        ],
    ));
    // 2: forever(): ends only by the instruction budget
    m.functions.push(func("forever", vec![], Type::Unit, vec![], vec![Jump { target: 0 }]));
    // 3: deep(n): unbounded recursion, ends only by the call-depth limit
    m.functions.push(func(
        "deep",
        vec![Type::Int],
        Type::Unit,
        vec![],
        vec![Call { func: 3, args: vec![0], dst: None }, Return { value: None }],
    ));
    // 4: classify(x)
    m.functions.push(func(
        "classify",
        vec![Type::Int],
        Type::Str,
        vec![Type::Int, Type::Bool, Type::Str],
        vec![
            Const { dst: 1, index: 0 },
            Binary { op: BinOp::Lt, dst: 2, a: 0, b: 1 },
            Branch { cond: 2, then: 3, otherwise: 5 },
            Const { dst: 3, index: 3 },
            Return { value: Some(3) },
            Binary { op: BinOp::Eq, dst: 2, a: 0, b: 1 },
            Branch { cond: 2, then: 7, otherwise: 9 },
            Const { dst: 3, index: 4 },
            Return { value: Some(3) },
            Const { dst: 3, index: 5 },
            Return { value: Some(3) },
        ],
    ));
    // 5: double(x), and 6: quad(x) = double(double(x)) + double(0)
    m.functions.push(func(
        "double",
        vec![Type::Int],
        Type::Int,
        vec![Type::Int, Type::Int],
        vec![Const { dst: 1, index: 2 }, Binary { op: BinOp::Mul, dst: 2, a: 0, b: 1 }, Return { value: Some(2) }],
    ));
    m.functions.push(func(
        "quad",
        vec![Type::Int],
        Type::Int,
        vec![Type::Int, Type::Int, Type::Int, Type::Int],
        vec![
            Call { func: 5, args: vec![0], dst: Some(1) },
            Call { func: 5, args: vec![1], dst: Some(2) },
            Const { dst: 3, index: 0 },
            Call { func: 5, args: vec![3], dst: Some(4) },
            Binary { op: BinOp::Add, dst: 2, a: 2, b: 4 },
            Return { value: Some(2) },
        ],
    ));
    // Private (not exported) helper, reachable only through calls.
    let mut helper = func("helper", vec![Type::Int], Type::Int, vec![], vec![Return { value: Some(0) }]);
    helper.exported = false;
    m.functions.push(helper);
    m
}

/// Waiting: sequential, nested, in a loop, several at once, odd durations.
fn waits() -> Module {
    let mut m = Module::new("waits");
    m.variables = vec![var("count", Type::Int, None), var("log", Type::Str, Some(Constant::Str(String::new())))];
    m.constants = vec![
        Constant::Float(1.5),
        Constant::Float(0.5),
        Constant::Int(1),
        Constant::Int(10),
        Constant::Int(100),
        Constant::Float(1.0),
        Constant::Int(1000),
        Constant::Float(-5.0),
        Constant::Float(0.0),
        Constant::Int(0),
    ];
    // count += k (constant index), through registers 0 = tmp, 1 = k
    let add = |k: u32| {
        vec![
            LoadVar { dst: 0, var: 0 },
            Const { dst: 1, index: k },
            Binary { op: BinOp::Add, dst: 0, a: 0, b: 1 },
            StoreVar { var: 0, src: 0 },
        ]
    };
    let wait = |seconds_const: u32| vec![Const { dst: 2, index: seconds_const }, Wait { seconds: 2 }];
    let regs3 = || vec![Type::Int, Type::Int, Type::Float];

    // 0: delayed(): count += 1; wait 1.5; count += 10; wait 0.5; count += 100
    let mut code = add(2);
    code.extend(wait(0));
    code.extend(add(3));
    code.extend(wait(1));
    code.extend(add(4));
    code.push(Return { value: None });
    m.functions.push(func("delayed", vec![], Type::Unit, regs3(), code));
    // 1: inner(): wait 1.0; count += 1
    let mut code = wait(5);
    code.extend(add(2));
    code.push(Return { value: None });
    m.functions.push(func("inner", vec![], Type::Unit, regs3(), code));
    // 2: nested(): inner(); count += 1000   (the wait is two frames deep)
    let mut code = vec![Call { func: 1, args: vec![], dst: None }];
    code.extend(add(6));
    code.push(Return { value: None });
    m.functions.push(func("nested", vec![], Type::Unit, regs3(), code));
    // 3: looped(n): n times { wait 1.0; count += 1 }
    m.functions.push(func(
        "looped",
        vec![Type::Int],
        Type::Unit,
        vec![Type::Int, Type::Int, Type::Float, Type::Int, Type::Bool],
        vec![
            Const { dst: 1, index: 9 },
            Const { dst: 4, index: 2 },
            Binary { op: BinOp::Lt, dst: 5, a: 1, b: 0 },
            Branch { cond: 5, then: 4, otherwise: 11 },
            Const { dst: 3, index: 5 },
            Wait { seconds: 3 },
            LoadVar { dst: 2, var: 0 },
            Binary { op: BinOp::Add, dst: 2, a: 2, b: 4 },
            StoreVar { var: 0, src: 2 },
            Binary { op: BinOp::Add, dst: 1, a: 1, b: 4 },
            Jump { target: 2 },
            Return { value: None },
        ],
    ));
    // 4: odd_waits(): wait -5, wait 0, wait NaN; each followed by count += 1
    let mut code = wait(7);
    code.extend(add(2));
    code.extend(wait(8));
    code.extend(add(2));
    code.extend([
        Const { dst: 2, index: 8 },
        Binary { op: BinOp::Div, dst: 2, a: 2, b: 2 },
        Wait { seconds: 2 },
    ]);
    code.extend(add(2));
    code.push(Return { value: None });
    m.functions.push(func("odd_waits", vec![], Type::Unit, regs3(), code));
    // 5: get_count()
    m.functions.push(func(
        "get_count",
        vec![],
        Type::Int,
        vec![Type::Int],
        vec![LoadVar { dst: 0, var: 0 }, Return { value: Some(0) }],
    ));
    // 6: wait_in_callee_then_fail(): waits, then divides by zero
    m.functions.push(func(
        "wait_then_fail",
        vec![],
        Type::Int,
        vec![Type::Int, Type::Int, Type::Float],
        vec![
            Const { dst: 2, index: 5 },
            Wait { seconds: 2 },
            Const { dst: 0, index: 2 },
            Const { dst: 1, index: 9 },
            Binary { op: BinOp::Div, dst: 0, a: 0, b: 1 },
            Return { value: Some(0) },
        ],
    ));
    m
}

/// Natives of every shape: results, failures, panics, a wrong result type,
/// `inout` arguments, the standard library, entities, time, and value types.
fn natives() -> Module {
    let mut m = Module::new("natives");
    m.imports = vec![
        import("test::note", vec![Param::new(Type::Str), Param::new(Type::Int)], Type::Unit), // 0
        import("test::fail", vec![], Type::Unit),                                          // 1
        import("test::boom", vec![], Type::Unit),                                          // 2
        import("test::liar", vec![], Type::Int),                                           // 3
        import("test::bump", vec![Param::inout(Type::Int)], Type::Unit),                   // 4
        import("test::twice", vec![Param::new(Type::Int)], Type::Int),                     // 5
        import("math::sqrt", vec![Param::new(Type::Float)], Type::Float),                  // 6
        import("string::len", vec![Param::new(Type::Str)], Type::Int),                     // 7
        import("Vec3::add", vec![Param::new(Type::object("Vec3")), Param::new(Type::object("Vec3"))], Type::object("Vec3")), // 8
        import("Vec3::length", vec![Param::new(Type::object("Vec3"))], Type::Float),       // 9
    ];
    m.variables = vec![
        var("last", Type::Int, None),
        var("pos", Type::object("Vec3"), Some(Constant::Value { ty: "Vec3".into(), json: "[1.0,2.0,3.0]".into() })),
    ];
    m.constants = vec![Constant::Str("hello".into()), Constant::Int(7), Constant::Int(5), Constant::Float(2.0)];
    // 0..3: one call each
    m.functions.push(func(
        "call_note",
        vec![],
        Type::Unit,
        vec![Type::Str, Type::Int],
        vec![
            Const { dst: 0, index: 0 },
            Const { dst: 1, index: 1 },
            CallNative { import: 0, args: vec![0, 1], dst: None },
            Return { value: None },
        ],
    ));
    m.functions.push(func(
        "call_fail",
        vec![],
        Type::Unit,
        vec![],
        vec![CallNative { import: 1, args: vec![], dst: None }, Return { value: None }],
    ));
    m.functions.push(func(
        "call_boom",
        vec![],
        Type::Unit,
        vec![],
        vec![CallNative { import: 2, args: vec![], dst: None }, Return { value: None }],
    ));
    m.functions.push(func(
        "call_liar",
        vec![],
        Type::Int,
        vec![Type::Int],
        vec![CallNative { import: 3, args: vec![], dst: Some(0) }, Return { value: Some(0) }],
    ));
    // 4: inout_bump(): x = 5; bump(&mut x); return x
    m.functions.push(func(
        "inout_bump",
        vec![],
        Type::Int,
        vec![Type::Int],
        vec![
            Const { dst: 0, index: 2 },
            CallNative { import: 4, args: vec![0], dst: None },
            Return { value: Some(0) },
        ],
    ));
    // 5: twice_chain(x) = twice(twice(x))
    m.functions.push(func(
        "twice_chain",
        vec![Type::Int],
        Type::Int,
        vec![Type::Int],
        vec![
            CallNative { import: 5, args: vec![0], dst: Some(1) },
            CallNative { import: 5, args: vec![1], dst: Some(1) },
            Return { value: Some(1) },
        ],
    ));
    // 6: stdlib(x) = string::len(to_str(sqrt(x)))
    m.functions.push(func(
        "stdlib",
        vec![Type::Float],
        Type::Int,
        vec![Type::Float, Type::Str, Type::Int],
        vec![
            CallNative { import: 6, args: vec![0], dst: Some(1) },
            Unary { op: UnOp::ToStr, dst: 2, src: 1 },
            CallNative { import: 7, args: vec![2], dst: Some(3) },
            Return { value: Some(3) },
        ],
    ));
    // 7..8: the bound entity and the clock
    m.functions.push(func(
        "me",
        vec![],
        Type::Entity,
        vec![Type::Entity],
        vec![SelfEntity { dst: 0 }, Return { value: Some(0) }],
    ));
    m.functions.push(func(
        "now",
        vec![],
        Type::Float,
        vec![Type::Float],
        vec![Now { dst: 0 }, Return { value: Some(0) }],
    ));
    // 9: grow(): pos = pos + pos; return length(pos). A value type with
    // value semantics and a literal default.
    m.functions.push(func(
        "grow",
        vec![],
        Type::Float,
        vec![Type::object("Vec3"), Type::object("Vec3"), Type::Float],
        vec![
            LoadVar { dst: 0, var: 1 },
            CallNative { import: 8, args: vec![0, 0], dst: Some(1) },
            StoreVar { var: 1, src: 1 },
            CallNative { import: 9, args: vec![1], dst: Some(2) },
            Return { value: Some(2) },
        ],
    ));
    // 10: remember(x): last = x
    m.functions.push(func(
        "remember",
        vec![Type::Int],
        Type::Unit,
        vec![],
        vec![StoreVar { var: 0, src: 0 }, Return { value: None }],
    ));
    m
}

/// Declared events and a subscription, linked identically by both backends.
fn events_decl() -> Module {
    let mut m = Module::new("events_decl");
    m.variables = vec![var("seen", Type::Int, None)];
    m.events = vec![EventDecl {
        name: "events_decl.Ping".into(),
        fields: vec![EventField { name: "amount".into(), ty: Type::Int }],
    }];
    m.functions.push(func(
        "on_ping",
        vec![Type::Int],
        Type::Unit,
        vec![],
        vec![StoreVar { var: 0, src: 0 }, Return { value: None }],
    ));
    m.subscriptions = vec![Subscription {
        event: EventRef::Name("events_decl.Ping".into()),
        handler: 0,
        scope: SubscriptionScope::Global,
    }];
    m
}

fn coll(op: CollOp, dst: u16, args: &[u16]) -> Instr {
    Collection { op, dst, args: args.to_vec() }
}

/// Lists, maps and tuples held in variables and registers.
fn collections() -> Module {
    let mut m = Module::new("collections");
    let ints = || Type::list(Type::Int);
    let names = || Type::map(Type::Str, Type::Int);
    m.variables = vec![var("log", ints(), None), var("names", names(), None)];
    m.constants = vec![Constant::Int(0), Constant::Int(1), Constant::Int(10)];

    // log.push(x); log.len()
    m.functions.push(func(
        "push",
        vec![Type::Int],
        Type::Int,
        vec![ints(), Type::Int],
        vec![
            LoadVar { dst: 1, var: 0 },
            coll(CollOp::ListPush, 1, &[1, 0]),
            StoreVar { var: 0, src: 1 },
            coll(CollOp::ListLen, 2, &[1]),
            Return { value: Some(2) },
        ],
    ));
    m.functions.push(func(
        "at",
        vec![Type::Int],
        Type::Int,
        vec![ints(), Type::Int],
        vec![
            LoadVar { dst: 1, var: 0 },
            coll(CollOp::ListGet, 2, &[1, 0]),
            Return { value: Some(2) },
        ],
    ));
    m.functions.push(func(
        "set_at",
        vec![Type::Int, Type::Int],
        Type::Unit,
        vec![ints()],
        vec![
            LoadVar { dst: 2, var: 0 },
            coll(CollOp::ListSet, 2, &[2, 0, 1]),
            StoreVar { var: 0, src: 2 },
            Return { value: None },
        ],
    ));
    m.functions.push(func(
        "insert_at",
        vec![Type::Int, Type::Int],
        Type::Unit,
        vec![ints()],
        vec![
            LoadVar { dst: 2, var: 0 },
            coll(CollOp::ListInsert, 2, &[2, 0, 1]),
            StoreVar { var: 0, src: 2 },
            Return { value: None },
        ],
    ));
    m.functions.push(func(
        "remove_at",
        vec![Type::Int],
        Type::Unit,
        vec![ints()],
        vec![
            LoadVar { dst: 1, var: 0 },
            coll(CollOp::ListRemove, 1, &[1, 0]),
            StoreVar { var: 0, src: 1 },
            Return { value: None },
        ],
    ));
    // sum of the log, by index
    m.functions.push(func(
        "sum",
        vec![],
        Type::Int,
        // r0 log, r1 total, r2 i, r3 len, r4 cond, r5 item, r6 one
        vec![ints(), Type::Int, Type::Int, Type::Int, Type::Bool, Type::Int, Type::Int],
        vec![
            LoadVar { dst: 0, var: 0 },
            Const { dst: 1, index: 0 },
            Const { dst: 2, index: 0 },
            Const { dst: 6, index: 1 },
            coll(CollOp::ListLen, 3, &[0]),
            /* 5 */ Binary { op: BinOp::Lt, dst: 4, a: 2, b: 3 },
            Branch { cond: 4, then: 7, otherwise: 11 },
            /* 7 */ coll(CollOp::ListGet, 5, &[0, 2]),
            Binary { op: BinOp::Add, dst: 1, a: 1, b: 5 },
            Binary { op: BinOp::Add, dst: 2, a: 2, b: 6 },
            Jump { target: 5 },
            /* 11 */ Return { value: Some(1) },
        ],
    ));
    // b = log; b[0] = 10; log[0] * 100 + b[0]: the copy does not alias.
    m.functions.push(func(
        "copy_is_independent",
        vec![],
        Type::Int,
        vec![ints(), ints(), Type::Int, Type::Int, Type::Int, Type::Int, Type::Int],
        vec![
            LoadVar { dst: 0, var: 0 },
            Move { dst: 1, src: 0 },
            Const { dst: 2, index: 0 },
            Const { dst: 3, index: 2 },
            coll(CollOp::ListSet, 1, &[1, 2, 3]),
            coll(CollOp::ListGet, 4, &[0, 2]),
            coll(CollOp::ListGet, 5, &[1, 2]),
            Const { dst: 6, index: 2 },
            Binary { op: BinOp::Mul, dst: 4, a: 4, b: 6 },
            Binary { op: BinOp::Mul, dst: 4, a: 4, b: 6 },
            Binary { op: BinOp::Add, dst: 4, a: 4, b: 5 },
            Return { value: Some(4) },
        ],
    ));
    m.functions.push(func(
        "bind",
        vec![Type::Str, Type::Int],
        Type::Int,
        vec![names(), Type::Int],
        vec![
            LoadVar { dst: 2, var: 1 },
            coll(CollOp::MapSet, 2, &[2, 0, 1]),
            StoreVar { var: 1, src: 2 },
            coll(CollOp::MapLen, 3, &[2]),
            Return { value: Some(3) },
        ],
    ));
    m.functions.push(func(
        "unbind",
        vec![Type::Str],
        Type::Unit,
        vec![names()],
        vec![
            LoadVar { dst: 1, var: 1 },
            coll(CollOp::MapRemove, 1, &[1, 0]),
            StoreVar { var: 1, src: 1 },
            Return { value: None },
        ],
    ));
    m.functions.push(func(
        "lookup",
        vec![Type::Str],
        Type::Int,
        vec![names(), Type::Int],
        vec![
            LoadVar { dst: 1, var: 1 },
            coll(CollOp::MapGet, 2, &[1, 0]),
            Return { value: Some(2) },
        ],
    ));
    m.functions.push(func(
        "has",
        vec![Type::Str],
        Type::Bool,
        vec![names(), Type::Bool],
        vec![
            LoadVar { dst: 1, var: 1 },
            coll(CollOp::MapHas, 2, &[1, 0]),
            Return { value: Some(2) },
        ],
    ));
    // the key at position `i`, in key order
    m.functions.push(func(
        "key_at",
        vec![Type::Int],
        Type::Str,
        vec![names(), Type::list(Type::Str), Type::Str],
        vec![
            LoadVar { dst: 1, var: 1 },
            coll(CollOp::MapKeys, 2, &[1]),
            coll(CollOp::ListGet, 3, &[2, 0]),
            Return { value: Some(3) },
        ],
    ));
    let pair = || Type::Tuple(vec![Type::Int, Type::Str]);
    m.functions.push(func(
        "pair",
        vec![Type::Int, Type::Str],
        pair(),
        vec![pair()],
        vec![coll(CollOp::MakeTuple, 2, &[0, 1]), Return { value: Some(2) }],
    ));
    m.functions.push(func(
        "swap",
        vec![Type::Int, Type::Str],
        Type::Tuple(vec![Type::Str, Type::Int]),
        vec![pair(), Type::Str, Type::Int, Type::Tuple(vec![Type::Str, Type::Int])],
        vec![
            coll(CollOp::MakeTuple, 2, &[0, 1]),
            coll(CollOp::TupleGet(1), 3, &[2]),
            coll(CollOp::TupleGet(0), 4, &[2]),
            coll(CollOp::MakeTuple, 5, &[3, 4]),
            Return { value: Some(5) },
        ],
    ));
    // a list literal and equality / display of collections
    m.functions.push(func(
        "literal_equals_log",
        vec![Type::Int, Type::Int],
        Type::Bool,
        vec![ints(), ints(), Type::Bool],
        vec![
            coll(CollOp::MakeList, 2, &[0, 1]),
            LoadVar { dst: 3, var: 0 },
            Binary { op: BinOp::Eq, dst: 4, a: 2, b: 3 },
            Return { value: Some(4) },
        ],
    ));
    m.functions.push(func(
        "log_text",
        vec![],
        Type::Str,
        vec![ints(), Type::Str],
        vec![LoadVar { dst: 0, var: 0 }, Unary { op: UnOp::ToStr, dst: 1, src: 0 }, Return { value: Some(1) }],
    ));
    m
}
