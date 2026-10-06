//! The `.d.ts` for a native registry.
//!
//! The same registry snapshot the compiler checks against produces the
//! declarations an editor uses, so autocomplete and the compiler cannot
//! disagree about which natives exist or what they take. Output is
//! deterministic (sorted), so it can be checked in and diffed.
//!
//! Every native `ns::name` becomes `ns.name(..)`. A native whose first
//! parameter is a component or value-type receiver is also a method of that
//! type (`v.add(w)`), and a method with no other parameters and no side
//! effects is a read-only property (`v.x`). Names that are reserved words in
//! TypeScript get a trailing underscore (`Vec3.new_`).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use pulsar_script_vm::{NativeFn, NativeRegistry, Type, TypeRegistry};

const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "let",
    "static",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "await",
    "async",
    // Predefined type names: TypeScript refuses them as namespace names.
    "any",
    "unknown",
    "never",
    "number",
    "string",
    "boolean",
    "symbol",
    "bigint",
    "object",
    "undefined",
];

/// A TypeScript identifier for `name`.
pub fn ts_name(name: &str) -> String {
    let mut ident: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if ident.starts_with(|c: char| c.is_ascii_digit()) {
        ident.insert(0, '_');
    }
    if RESERVED.contains(&ident.as_str()) {
        ident.push('_');
    }
    ident
}

/// The TypeScript spelling of a script type.
pub fn ts_type(ty: &Type) -> String {
    match ty {
        Type::Unit => "void".into(),
        Type::Bool => "boolean".into(),
        Type::Int => "int".into(),
        Type::Float => "number".into(),
        Type::Str => "string".into(),
        Type::Entity => "Entity".into(),
        Type::Component(name) | Type::Object(name) => ts_name(name),
        Type::List(element) => format!("{}[]", ts_type(element)),
        Type::Map(key, value) => format!("Map<{}, {}>", ts_type(key), ts_type(value)),
        Type::Tuple(items) => format!(
            "[{}]",
            items.iter().map(ts_type).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Split `ns::name` at the last `::`.
pub(crate) fn split_native(name: &str) -> Option<(&str, &str)> {
    name.rsplit_once("::")
}

/// Natives a script can call: every parameter by value, and a namespace.
pub(crate) fn callable(native: &NativeFn) -> bool {
    !native.sig.params.iter().any(|p| p.inout) && split_native(&native.name).is_some()
}

pub fn declarations(natives: &NativeRegistry) -> String {
    let types = TypeRegistry::global();
    let mut out = String::new();
    out.push_str("// Generated from the engine's native registry. Do not edit.\n\n");
    out.push_str(PRELUDE);

    // Nominal interfaces for every type a native can name.
    let mut named: BTreeMap<String, &'static str> = BTreeMap::new();
    for component in types.components() {
        named.insert(component.name.to_owned(), "component");
    }
    for value in types.value_types() {
        named.insert(value.to_owned(), "value type");
    }

    let mut functions: Vec<_> = natives.functions().filter(|n| callable(n)).collect();
    functions.sort_by(|a, b| a.name.cmp(&b.name));

    // Members of each receiver type, and the namespaces of every native.
    let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut namespaces: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for native in &functions {
        let (ns, name) = split_native(&native.name).expect("callable");
        let params: Vec<String> = native
            .sig
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                format!(
                    "{}: {}",
                    ts_name(native.param_names.get(i).map_or("arg", String::as_str)),
                    ts_type(&p.ty)
                )
            })
            .collect();
        let doc = if native.doc.is_empty() {
            String::new()
        } else {
            format!("    /** {} */\n", native.doc.replace("*/", "* /"))
        };
        namespaces
            .entry(ns.replace("::", "_"))
            .or_default()
            .push(format!(
                "{doc}    function {}({}): {};\n",
                ts_name(name),
                params.join(", "),
                ts_type(&native.sig.ret)
            ));
        if let (Some(receiver), Some(first)) = (&native.receiver, native.sig.params.first()) {
            if &first.ty == receiver {
                if let Type::Component(r) | Type::Object(r) = receiver {
                    let rest = params[1..].join(", ");
                    let member = if params.len() == 1
                        && native.sig.ret != Type::Unit
                        && native.flags.side_effect_free
                    {
                        format!(
                            "{doc}    readonly {}: {};\n",
                            ts_name(name),
                            ts_type(&native.sig.ret)
                        )
                    } else {
                        format!(
                            "{doc}    {}({rest}): {};\n",
                            ts_name(name),
                            ts_type(&native.sig.ret)
                        )
                    };
                    members.entry(r.clone()).or_default().push(member);
                    named.entry(r.clone()).or_insert("type");
                }
            }
        }
    }

    for (name, kind) in &named {
        let _ = writeln!(out, "/** A {kind} the engine provides. */");
        let _ = writeln!(out, "interface {} {{", ts_name(name));
        let _ = writeln!(out, "    readonly __pulsar_type: \"{name}\";");
        for member in members.get(name).into_iter().flatten() {
            out.push_str(member);
        }
        out.push_str("}\n\n");
    }
    for (ns, items) in &namespaces {
        let _ = writeln!(out, "declare namespace {} {{", ts_name(ns));
        for item in items {
            out.push_str(item);
        }
        out.push_str("}\n\n");
    }
    out
}

const PRELUDE: &str = "\
/** An engine integer (64-bit). A whole-number literal is an `int`; `x as number` converts. */
type int = number;

/** An entity handle. */
interface Entity {
    readonly __pulsar_type: \"Entity\";
}

/** Keep a field's identity across a rename: `@renamedFrom(\"oldName\") newName = 0;` */
declare function renamedFrom(oldName: string): (value: undefined, context: ClassFieldDecoratorContext) => void;

/** Suspend the calling method for `seconds` of game time. Use as `await wait(1.5)` in an `async` method. */
declare function wait(seconds: number): Promise<void>;

/** What a script class may rely on: the entity it is bound to and the game clock. */
declare abstract class ScriptClass {
    /** The entity this instance is bound to. */
    readonly entity: Entity;
    /** Game time in seconds. */
    readonly time: number;
}

";
