//! A second language on the bytecode, with no TypeScript in sight.
//!
//! The toy language is s-expressions over integers:
//!
//! ```text
//! (def name (param ...) expr)
//! expr := 123 | param | (+ a b) | (- a b) | (* a b) | (< a b) | (== a b)
//!       | (if cond then else)
//!       | (sum i n body)         ; body summed for i in 0..n (a loop)
//!       | (call name arg ...)    ; a function of the module, declared before or after
//! ```
//!
//! It is the whole of a front end: a parser, a type rule or two, and calls into
//! `ModuleBuilder`. What it compiles to is verified by the VM's verifier and run.

mod common;

use common::Harness;
use pulsar_script_vm::module_builder::{BuildError, FunctionBuilder, FunctionHandle, ModuleBuilder};
use pulsar_script_vm::{BinOp, Constant, Module, NativeRegistry, Reg, SourceLoc, Type, Value};

// ---- the front end: syntax ---------------------------------------------------------

#[derive(Debug, Clone)]
enum Sexp {
    Atom(String, usize),
    List(Vec<Sexp>, usize),
}

fn parse(source: &str) -> Result<Vec<Sexp>, String> {
    let mut stack: Vec<(Vec<Sexp>, usize)> = vec![(Vec::new(), 0)];
    let mut atom = String::new();
    let mut atom_at = 0;
    let flush = |atom: &mut String, at: usize, stack: &mut Vec<(Vec<Sexp>, usize)>| {
        if !atom.is_empty() {
            stack
                .last_mut()
                .expect("a list")
                .0
                .push(Sexp::Atom(std::mem::take(atom), at));
        }
    };
    for (at, ch) in source.char_indices() {
        match ch {
            '(' => {
                flush(&mut atom, atom_at, &mut stack);
                stack.push((Vec::new(), at));
            }
            ')' => {
                flush(&mut atom, atom_at, &mut stack);
                let (items, open) = stack.pop().ok_or("unbalanced )")?;
                let Some(parent) = stack.last_mut() else {
                    return Err("unbalanced )".into());
                };
                parent.0.push(Sexp::List(items, open));
            }
            c if c.is_whitespace() => flush(&mut atom, atom_at, &mut stack),
            c => {
                if atom.is_empty() {
                    atom_at = at;
                }
                atom.push(c);
            }
        }
    }
    flush(&mut atom, atom_at, &mut stack);
    if stack.len() != 1 {
        return Err("unbalanced (".into());
    }
    Ok(stack.pop().expect("one").0)
}

// ---- the front end: lowering ---------------------------------------------------------

struct Lower<'a> {
    module: ModuleBuilder<'a>,
    source: &'a str,
}

type Value3 = (Reg, Type);

fn built<T>(result: Result<T, BuildError>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

impl<'a> Lower<'a> {
    /// `line:column` of a byte offset, one-based.
    fn loc(&self, at: usize) -> SourceLoc {
        let before = &self.source[..at];
        let line = before.matches('\n').count() as u32 + 1;
        let column = (at - before.rfind('\n').map_or(0, |n| n + 1)) as u32 + 1;
        SourceLoc::at("toy.lisp", line, column)
    }

    fn expr(&mut self, f: &mut FunctionBuilder, e: &Sexp) -> Result<Value3, String> {
        match e {
            Sexp::Atom(text, at) => {
                f.set_loc(self.loc(*at));
                if let Ok(n) = text.parse::<i64>() {
                    return built(self.module.load_constant(f, Constant::Int(n)));
                }
                let local = f.lookup(text).ok_or(format!("unknown name `{text}`"))?;
                Ok((local.reg, local.ty.clone()))
            }
            Sexp::List(items, at) => {
                f.set_loc(self.loc(*at));
                let Some(Sexp::Atom(head, _)) = items.first() else {
                    return Err("a list starts with a name".into());
                };
                let args = &items[1..];
                match (head.as_str(), args) {
                    ("+" | "-" | "*" | "<" | "==", [a, b]) => {
                        let op = match head.as_str() {
                            "+" => BinOp::Add,
                            "-" => BinOp::Sub,
                            "*" => BinOp::Mul,
                            "<" => BinOp::Lt,
                            _ => BinOp::Eq,
                        };
                        let (a, _) = self.expr(f, a)?;
                        let (b, _) = self.expr(f, b)?;
                        f.set_loc(self.loc(*at));
                        let dst = built(f.binary(op, a, b))?
                            .ok_or(format!("`{head}` does not apply here"))?;
                        let ty = built(f.reg_type(dst).cloned())?;
                        Ok((dst, ty))
                    }
                    ("if", [cond, then, otherwise]) => {
                        let (cond, ty) = self.expr(f, cond)?;
                        if ty != Type::Bool {
                            return Err("`if` needs a condition".into());
                        }
                        let (yes, no, end) = (f.new_label(), f.new_label(), f.new_label());
                        f.branch(cond, yes, no);
                        f.bind(yes);
                        let (a, ty) = self.expr(f, then)?;
                        let result = built(f.new_reg(ty.clone()))?;
                        f.mov(result, a);
                        f.jump(end);
                        f.bind(no);
                        let (b, other) = self.expr(f, otherwise)?;
                        if other != ty {
                            return Err("the branches have different types".into());
                        }
                        f.mov(result, b);
                        f.bind(end);
                        Ok((result, ty))
                    }
                    ("sum", [Sexp::Atom(var, _), n, body]) => {
                        let (n, _) = self.expr(f, n)?;
                        let total = built(f.new_reg(Type::Int))?;
                        let i = built(f.new_reg(Type::Int))?;
                        f.push_scope();
                        f.bind_local(var.clone(), i, Type::Int, false);
                        let (top, end, next) = (f.label_here(), f.new_label(), f.new_label());
                        let more = built(f.binary(BinOp::Lt, i, n))?
                            .ok_or("cannot compare")?;
                        let step = f.new_label();
                        f.branch(more, step, end);
                        f.bind(step);
                        f.enter_loop(end, next);
                        let (value, _) = self.expr(f, body)?;
                        f.exit_loop();
                        f.bind(next);
                        let added = built(f.binary(BinOp::Add, total, value))?
                            .ok_or("cannot add")?;
                        f.mov(total, added);
                        let (one, _) =
                            built(self.module.load_constant(f, Constant::Int(1)))?;
                        let bumped = built(f.binary(BinOp::Add, i, one))?
                            .ok_or("cannot add")?;
                        f.mov(i, bumped);
                        f.jump(top);
                        f.bind(end);
                        f.pop_scope();
                        Ok((total, Type::Int))
                    }
                    ("call", [Sexp::Atom(name, _), rest @ ..]) => {
                        let callee = self
                            .module
                            .function(name)
                            .ok_or(format!("no function `{name}`"))?;
                        let mut regs = Vec::new();
                        for arg in rest {
                            regs.push(self.expr(f, arg)?.0);
                        }
                        let ret = self.module.signature(callee).1.clone();
                        let dst = built(self.module.call(f, callee, &regs))?;
                        Ok((dst.ok_or("a call that returns nothing")?, ret))
                    }
                    _ => Err(format!("cannot use `{head}` here")),
                }
            }
        }
    }
}

/// Compile a toy program to a verified module.
fn compile(source: &str, natives: &NativeRegistry) -> Result<Module, String> {
    let program = parse(source)?;
    let mut lower = Lower {
        module: ModuleBuilder::new("Toy", natives),
        source,
    };

    // Declare every function first, so a call can name one that comes later.
    let mut defs: Vec<(FunctionHandle, Vec<String>, &Sexp)> = Vec::new();
    for def in &program {
        let Sexp::List(items, _) = def else {
            return Err("a program is a list of definitions".into());
        };
        let [Sexp::Atom(kw, _), Sexp::Atom(name, _), Sexp::List(params, _), body] = &items[..]
        else {
            return Err("(def name (params) expr)".into());
        };
        if kw != "def" {
            return Err("(def name (params) expr)".into());
        }
        let names: Vec<String> = params
            .iter()
            .map(|p| match p {
                Sexp::Atom(n, _) => Ok(n.clone()),
                _ => Err("a parameter is a name".to_owned()),
            })
            .collect::<Result<_, _>>()?;
        let handle = built(lower.module.declare_function(
                name.clone(),
                vec![Type::Int; names.len()],
                Type::Int,
                true,
            ))?;
        defs.push((handle, names, body));
    }

    for (handle, names, body) in defs {
        let mut f = lower.module.begin_function(handle);
        for (i, name) in names.iter().enumerate() {
            f.bind_local(name.clone(), f.param_reg(i), Type::Int, false);
        }
        let (value, ty) = lower.expr(&mut f, body)?;
        if ty != Type::Int {
            return Err("a function returns an integer".into());
        }
        f.ret(Some(value));
        built(lower.module.finish_function(handle, f))?;
    }
    built(lower.module.finish())
}

// ---- running it ------------------------------------------------------------------------

fn run(module: &Module, natives: &NativeRegistry, entry: &str, args: &[i64]) -> i64 {
    let program = pulsar_script_vm::Program::link(std::sync::Arc::new(module.clone()), natives)
        .expect("a module the verifier accepted links");
    let args: Vec<Value> = args.iter().map(|n| Value::Int(*n)).collect();
    match Harness::new().run(&program, entry, &args).expect("runs") {
        Value::Int(n) => n,
        other => panic!("expected an int, got {other:?}"),
    }
}

#[test]
fn a_toy_language_compiles_to_a_module_that_verifies_and_runs() {
    let natives = NativeRegistry::with_engine_natives();
    let module = compile(
        "(def add (a b) (+ a b))
         (def max (a b) (if (< a b) b a))
         (def squares (n) (sum i n (* i i)))
         (def hypot2 (a b) (call add (* a a) (* b b)))
         (def twice_later (x) (call add (call later x) (call later x)))
         (def later (x) (* x 10))",
        &natives,
    )
    .expect("compiles");

    // The verifier accepts it, and so would the linker.
    pulsar_script_vm::verify(&module).expect("verifies");
    assert_eq!(module.name, "Toy");
    assert_eq!(module.functions.len(), 6);

    assert_eq!(run(&module, &natives, "add", &[2, 3]), 5);
    assert_eq!(run(&module, &natives, "max", &[2, 9]), 9);
    assert_eq!(run(&module, &natives, "max", &[9, 2]), 9);
    assert_eq!(run(&module, &natives, "squares", &[4]), 1 + 4 + 9);
    assert_eq!(run(&module, &natives, "squares", &[0]), 0);
    assert_eq!(run(&module, &natives, "hypot2", &[3, 4]), 25);
    // A call to a function declared later.
    assert_eq!(run(&module, &natives, "twice_later", &[2]), 40);
}

#[test]
fn the_toy_modules_carry_debug_info_and_survive_the_binary_format() {
    let natives = NativeRegistry::with_engine_natives();
    let module = compile("(def f (x)\n  (+ x 1))", &natives).expect("compiles");
    let function = &module.functions[0];
    let first = function.first_location().expect("a location");
    assert_eq!(first.file, "toy.lisp");
    assert_eq!(first.line, Some(2));
    assert!(function.location(0).is_some());

    let again = Module::decode(&module.to_binary()).expect("decodes");
    assert_eq!(again, module);
    assert_eq!(run(&again, &natives, "f", &[41]), 42);
}

#[test]
fn the_front_ends_own_mistakes_stay_its_own() {
    let natives = NativeRegistry::with_engine_natives();
    assert!(compile("(def f (x) (+ x y))", &natives)
        .unwrap_err()
        .contains("unknown name `y`"));
    assert!(compile("(def f (x) (< x 1))", &natives)
        .unwrap_err()
        .contains("returns an integer"));
    assert!(compile("(def f (x) (call g x))", &natives)
        .unwrap_err()
        .contains("no function `g`"));
    // The builder refuses a call of the wrong arity, naming the function.
    let error = compile("(def g (a b) a) (def f (x) (call g x))", &natives).unwrap_err();
    assert!(error.contains("`g` takes 2 argument(s), got 1"), "{error}");
}
