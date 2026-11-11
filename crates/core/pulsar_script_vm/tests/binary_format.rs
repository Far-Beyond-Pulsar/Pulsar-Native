//! Binary module encoding (#852): round trips against JSON and header
//! checks.

use pulsar_script_vm::{
    BinOp, Constant, DebugInfo, EventDecl, EventField, EventRef, Function, Import, Instr, Module,
    ModuleDecodeError, Param, Signature, SourceLoc, Subscription, SubscriptionScope, Type, UnOp,
    Variable, BINARY_MAGIC, FORMAT_VERSION,
};

/// A module using every part of the format.
fn everything() -> Module {
    let mut module = Module::new("Everything");
    module.constants = vec![
        Constant::Bool(true),
        Constant::Int(i64::MIN),
        Constant::Float(-0.25),
        Constant::Float(f64::MAX),
        Constant::Str("héllo \"world\"".into()),
    ];
    module.imports = vec![Import {
        name: "math::sin".into(),
        sig: Signature::new(
            [Param::new(Type::Float), Param::inout(Type::object("Vec3"))],
            Type::Float,
        ),
    }];
    module.variables = vec![
        Variable {
            name: "speed".into(),
            ty: Type::Float,
            default: Some(Constant::Float(1.5)),
            id: None,
        },
        Variable {
            name: "target".into(),
            ty: Type::Entity,
            default: None,
            id: None,
        },
        Variable {
            name: "__slot:abc".into(),
            ty: Type::component("Health"),
            default: None,
            id: None,
        },
    ];
    let mut debug = DebugInfo::default();
    debug.record(0, &SourceLoc::node("graph_save.json", "node_1"));
    debug.record(
        1,
        &SourceLoc {
            file: "a.ts".into(),
            node: String::new(),
            line: Some(3),
            column: Some(7),
        },
    );
    module.functions = vec![
        Function {
            name: "begin_play".into(),
            exported: true,
            params: vec![],
            ret: Type::Unit,
            registers: vec![Type::Int, Type::Float, Type::Str, Type::Bool, Type::Entity],
            code: vec![
                Instr::Const { dst: 0, index: 1 },
                Instr::Move { dst: 1, src: 1 },
                Instr::Unary {
                    op: UnOp::IntToFloat,
                    dst: 1,
                    src: 0,
                },
                Instr::Binary {
                    op: BinOp::Mul,
                    dst: 0,
                    a: 0,
                    b: 0,
                },
                Instr::Branch {
                    cond: 3,
                    then: 5,
                    otherwise: 6,
                },
                Instr::Jump { target: 6 },
                Instr::CallNative {
                    import: 0,
                    args: vec![1, 1],
                    dst: Some(1),
                },
                Instr::Call {
                    func: 1,
                    args: vec![],
                    dst: None,
                },
                Instr::LoadVar { dst: 1, var: 0 },
                Instr::StoreVar { var: 0, src: 1 },
                Instr::SelfEntity { dst: 4 },
                Instr::Now { dst: 1 },
                Instr::Wait { seconds: 1 },
                Instr::Return { value: None },
            ],
            debug: Some(debug),
        },
        Function {
            name: "on_hit".into(),
            exported: false,
            params: vec![Type::Entity],
            ret: Type::Unit,
            registers: vec![Type::Entity],
            code: vec![Instr::Return { value: None }],
            debug: None,
        },
    ];
    module.events = vec![EventDecl {
        name: "Everything.Hit".into(),
        fields: vec![
            EventField::new("entity", Type::Entity),
            EventField::new("damage", Type::Float),
        ],
    }];
    module.subscriptions = vec![
        Subscription {
            event: EventRef::Name("Everything.Hit".into()),
            handler: 1,
            scope: SubscriptionScope::Self_,
        },
        Subscription {
            event: EventRef::Id(0xdead_beef),
            handler: 1,
            scope: SubscriptionScope::Class,
        },
    ];
    module
}

#[test]
fn binary_round_trips_and_matches_json() {
    let module = everything();
    let binary = module.to_binary();
    assert!(binary.starts_with(&BINARY_MAGIC));
    assert_eq!(&binary[4..8], &FORMAT_VERSION.to_le_bytes());
    assert_eq!(Module::from_binary(&binary).unwrap(), module);

    // JSON -> binary -> JSON is lossless, and `decode` tells them apart.
    let json = module.to_json().unwrap();
    let from_json = Module::decode(json.as_bytes()).unwrap();
    let from_binary = Module::decode(&from_json.to_binary()).unwrap();
    assert_eq!(from_binary, from_json);
    assert_eq!(from_binary.to_json().unwrap(), json);
    assert!(
        binary.len() < json.len() / 2,
        "binary {} vs json {}",
        binary.len(),
        json.len()
    );
}

/// A small deterministic generator (no extra dependencies): many random
/// modules must survive JSON -> binary -> JSON unchanged.
#[test]
fn random_modules_round_trip() {
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let types = [
        Type::Unit,
        Type::Bool,
        Type::Int,
        Type::Float,
        Type::Str,
        Type::Entity,
        Type::component("C"),
        Type::object("Vec3"),
    ];
    for round in 0..200 {
        let mut module = Module::new(format!("M{round}"));
        for _ in 0..next() % 6 {
            module.constants.push(match next() % 4 {
                0 => Constant::Bool(next() % 2 == 0),
                1 => Constant::Int(next() as i64),
                2 => Constant::Float(f64::from_bits(next() & 0x7fef_ffff_ffff_ffff)),
                _ => Constant::Str(format!("s{}", next())),
            });
        }
        for f in 0..next() % 4 {
            let ty = |n: u64| types[(n % types.len() as u64) as usize].clone();
            let code = (0..next() % 10)
                .map(|_| match next() % 5 {
                    0 => Instr::Const {
                        dst: (next() % 8) as u16,
                        index: (next() % 4) as u32,
                    },
                    1 => Instr::Binary {
                        op: BinOp::Add,
                        dst: 1,
                        a: 2,
                        b: (next() % 60000) as u16,
                    },
                    2 => Instr::Call {
                        func: 0,
                        args: (0..next() % 3).map(|r| r as u16).collect(),
                        dst: None,
                    },
                    3 => Instr::Return {
                        value: Some((next() % 3) as u16),
                    },
                    _ => Instr::Branch {
                        cond: 0,
                        then: next() as u32,
                        otherwise: 0,
                    },
                })
                .collect();
            module.functions.push(Function {
                name: format!("f{f}"),
                exported: next() % 2 == 0,
                params: (0..next() % 3).map(|_| ty(next())).collect(),
                ret: ty(next()),
                registers: (0..next() % 5).map(|_| ty(next())).collect(),
                code,
                debug: (next() % 2 == 0).then(|| {
                    let mut d = DebugInfo::default();
                    d.record(0, &SourceLoc::node("g", format!("n{}", next())));
                    d
                }),
            });
        }
        // Binary is exact (JSON float parsing may round the last digit, so
        // the JSON leg compares against the JSON-parsed module).
        assert_eq!(
            Module::decode(&module.to_binary()).unwrap(),
            module,
            "round {round}"
        );
        let from_json = Module::from_json(&module.to_json().unwrap()).unwrap();
        let decoded = Module::decode(&from_json.to_binary()).unwrap();
        assert_eq!(decoded, from_json, "round {round}");
        assert_eq!(
            decoded.to_json().unwrap(),
            from_json.to_json().unwrap(),
            "round {round}"
        );
    }
}

#[test]
fn bad_binary_modules_are_errors() {
    let binary = everything().to_binary();
    assert!(matches!(
        Module::from_binary(&binary[..6]),
        Err(ModuleDecodeError::Truncated)
    ));

    let mut other_version = binary.clone();
    other_version[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert!(matches!(
        Module::decode(&other_version),
        Err(ModuleDecodeError::UnsupportedVersion { found, expected }) if found == FORMAT_VERSION + 1 && expected == FORMAT_VERSION
    ));

    let mut trailing = binary.clone();
    trailing.push(0);
    assert!(matches!(
        Module::decode(&trailing),
        Err(ModuleDecodeError::Corrupt(_))
    ));
    assert!(matches!(
        Module::decode(&binary[..binary.len() - 3]),
        Err(ModuleDecodeError::Corrupt(_))
    ));
    assert!(matches!(
        Module::decode(b"{ not json"),
        Err(ModuleDecodeError::Json(_))
    ));
}

/// The binary header, then the start of a module body up to and including
/// `Module::name`'s length prefix: `format_version` (a one-byte varint),
/// then the length as bincode's u64 varint marker (`253`) and value.
fn module_prefix_claiming_name_len(len: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&BINARY_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.push(FORMAT_VERSION as u8);
    bytes.push(253);
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes
}

/// A length claim far past the input is refused before anything is
/// allocated for it: a hostile module must not make the engine reserve
/// gigabytes (or abort) on load.
#[test]
fn huge_length_claims_are_errors_not_allocations() {
    let bytes = module_prefix_claiming_name_len(1 << 40);
    assert!(matches!(
        Module::from_binary(&bytes),
        Err(ModuleDecodeError::Corrupt(_))
    ));
}

/// Types nest (`list<list<..>>`), and decoding one recurses per level: a
/// module claiming a million levels is corrupt, not a stack overflow.
#[test]
fn deeply_nested_types_are_errors_not_stack_overflows() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&BINARY_MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.push(FORMAT_VERSION as u8); // format_version
    bytes.push(0); // name: ""
    bytes.push(0); // constants: none
    bytes.push(0); // imports: none
    bytes.push(1); // variables: one
    bytes.push(0); // its name: ""
                   // Its type: `list<list<list<..` a million levels deep, never closed
                   // (8 is `Type::List`'s variant index).
    bytes.extend(std::iter::repeat(8u8).take(1 << 20));
    assert!(matches!(
        Module::from_binary(&bytes),
        Err(ModuleDecodeError::Corrupt(_))
    ));
}

/// The hand-written `Type` decoder reads exactly what the derived encoder
/// writes, for every variant and nesting.
#[test]
fn nested_types_round_trip() {
    let nested = Type::map(
        Type::Str,
        Type::list(Type::Tuple(vec![
            Type::Int,
            Type::Unit,
            Type::Bool,
            Type::Float,
            Type::Entity,
            Type::component("Health"),
            Type::object("Vec3"),
            Type::list(Type::map(Type::Int, Type::Tuple(vec![]))),
        ])),
    );
    let mut module = Module::new("Nested");
    module.variables = vec![Variable {
        name: "table".into(),
        ty: nested,
        default: None,
        id: None,
    }];
    assert_eq!(Module::from_binary(&module.to_binary()).unwrap(), module);
}
