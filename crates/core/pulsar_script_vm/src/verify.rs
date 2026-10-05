//! Structural and type verification of a [`Module`], independent of any
//! native registry: every index is in range, every instruction's operand
//! types agree, every call matches its callee's declared signature, and
//! control cannot fall off the end of a function. A verified module cannot
//! make the interpreter index out of bounds or see a value of the wrong
//! type. (Imports are checked against the actual natives at link time.)

use std::collections::HashSet;

use crate::error::VerifyError;
use crate::events::{check_handler, is_event_field_type};
use crate::module::{
    BinOp, CollOp, EventRef, Instr, Module, Reg, UnOp, FORMAT_VERSION, MIN_FORMAT_VERSION,
};
use crate::types::Type;

pub fn verify(module: &Module) -> Result<(), VerifyError> {
    if !(MIN_FORMAT_VERSION..=FORMAT_VERSION).contains(&module.format_version) {
        return Err(VerifyError::module(format!(
            "format version {} (this VM reads {MIN_FORMAT_VERSION} to {FORMAT_VERSION})",
            module.format_version
        )));
    }

    let mut names = HashSet::new();
    for function in &module.functions {
        if !names.insert(function.name.as_str()) {
            return Err(VerifyError::module(format!(
                "function `{}` defined twice",
                function.name
            )));
        }
    }
    let mut imports = HashSet::new();
    for import in &module.imports {
        if !imports.insert(import.name.as_str()) {
            return Err(VerifyError::module(format!(
                "`{}` imported twice",
                import.name
            )));
        }
    }
    let mut vars = HashSet::new();
    let mut ids = HashSet::new();
    for var in &module.variables {
        if !vars.insert(var.name.as_str()) {
            return Err(VerifyError::module(format!(
                "variable `{}` declared twice",
                var.name
            )));
        }
        // Identity must be unambiguous: state is matched by it on reload.
        if let Some(id) = &var.id {
            if id.is_empty() {
                return Err(VerifyError::module(format!(
                    "variable `{}` has an empty id",
                    var.name
                )));
            }
            if !ids.insert(id.as_str()) {
                return Err(VerifyError::module(format!(
                    "variable id `{id}` is used by more than one variable (second: `{}`)",
                    var.name
                )));
            }
        }
        if let Some(default) = &var.default {
            if default.ty() != var.ty {
                return Err(VerifyError::module(format!(
                    "variable `{}` is {} but its default is {}",
                    var.name,
                    var.ty,
                    default.ty()
                )));
            }
        }
    }

    for ty in module.variables.iter().map(|v| &v.ty).chain(
        module
            .imports
            .iter()
            .flat_map(|i| i.sig.params.iter().map(|p| &p.ty).chain([&i.sig.ret])),
    ) {
        ty.validate().map_err(VerifyError::module)?;
    }

    for function in &module.functions {
        FunctionVerifier { module, function }.verify()?;
    }
    verify_events(module)
}

/// Declared events and subscriptions. A handler for an event the module
/// declares is checked here; one for any other event at link time.
fn verify_events(module: &Module) -> Result<(), VerifyError> {
    let mut names = HashSet::new();
    for event in &module.events {
        if event.name.trim().is_empty() {
            return Err(VerifyError::module("an event has an empty name"));
        }
        if !names.insert(event.name.as_str()) {
            return Err(VerifyError::module(format!(
                "event `{}` declared twice",
                event.name
            )));
        }
        let mut fields = HashSet::new();
        for field in &event.fields {
            if !fields.insert(field.name.as_str()) {
                return Err(VerifyError::module(format!(
                    "event `{}` has two fields named `{}`",
                    event.name, field.name
                )));
            }
            if !is_event_field_type(&field.ty) {
                return Err(VerifyError::module(format!(
                    "event `{}` field `{}` is {}; event fields are bool, int, float, string, entity or a registered value type",
                    event.name, field.name, field.ty
                )));
            }
        }
    }
    for (index, subscription) in module.subscriptions.iter().enumerate() {
        let what = || format!("subscription {index} (`{}`)", subscription.event);
        let handler = module
            .functions
            .get(subscription.handler as usize)
            .ok_or_else(|| {
                VerifyError::module(format!(
                    "{}: handler {} out of range",
                    what(),
                    subscription.handler
                ))
            })?;
        let err = |message: String| VerifyError {
            function: Some(handler.name.clone()),
            pc: None,
            message,
        };
        if handler.ret != crate::types::Type::Unit {
            return Err(err(format!(
                "{}: an event handler must return unit",
                what()
            )));
        }
        if let Some(bad) = handler.params.iter().find(|ty| !is_event_field_type(ty)) {
            return Err(err(format!(
                "{}: handler parameter type {bad} is not an event field type (expected a primitive or registered value type)",
                what()
            )));
        }
        if let EventRef::Name(name) = &subscription.event {
            if name.trim().is_empty() {
                return Err(err(format!("subscription {index}: empty event name")));
            }
            if let Some(event) = module.events.iter().find(|e| &e.name == name) {
                let fields: Vec<_> = event.fields.iter().map(|f| f.ty.clone()).collect();
                check_handler(&handler.params, &fields)
                    .map_err(|m| err(format!("{}: {m}", what())))?;
            }
        }
    }
    Ok(())
}

struct FunctionVerifier<'a> {
    module: &'a Module,
    function: &'a crate::module::Function,
}

impl FunctionVerifier<'_> {
    fn err(&self, pc: Option<usize>, message: impl Into<String>) -> VerifyError {
        VerifyError {
            function: Some(self.function.name.clone()),
            pc,
            message: message.into(),
        }
    }

    fn verify(&self) -> Result<(), VerifyError> {
        let f = self.function;
        if f.registers.len() > usize::from(Reg::MAX) + 1 {
            return Err(self.err(None, "too many registers"));
        }
        for ty in f.registers.iter().chain(&f.params).chain([&f.ret]) {
            ty.validate().map_err(|message| self.err(None, message))?;
        }
        if f.registers.len() < f.params.len() || f.registers[..f.params.len()] != f.params[..] {
            return Err(self.err(None, "the first registers must be the parameters"));
        }
        match f.code.last() {
            Some(Instr::Return { .. } | Instr::Jump { .. }) => {}
            _ => return Err(self.err(None, "code must end with `Return` or `Jump`")),
        }
        for (pc, instr) in f.code.iter().enumerate() {
            self.instr(pc, instr)?;
        }
        if let Some(debug) = &f.debug {
            let mut previous_end = 0;
            for range in &debug.ranges {
                if range.start >= range.end
                    || (range.end as usize) > f.code.len()
                    || range.start < previous_end
                {
                    return Err(self.err(
                        None,
                        format!(
                            "debug range {}..{} is empty, out of order or past the code ({} instructions)",
                            range.start,
                            range.end,
                            f.code.len()
                        ),
                    ));
                }
                previous_end = range.end;
            }
            if let Some(source) = debug
                .register_sources
                .iter()
                .find(|source| usize::from(source.register) >= f.registers.len())
            {
                return Err(self.err(
                    None,
                    format!(
                        "debug register {} is outside the register file",
                        source.register
                    ),
                ));
            }
        }
        Ok(())
    }

    fn reg(&self, pc: usize, reg: Reg) -> Result<&Type, VerifyError> {
        self.function
            .registers
            .get(usize::from(reg))
            .ok_or_else(|| self.err(Some(pc), format!("register r{reg} out of range")))
    }

    fn expect(&self, pc: usize, reg: Reg, ty: &Type) -> Result<(), VerifyError> {
        let actual = self.reg(pc, reg)?;
        if actual == ty {
            Ok(())
        } else {
            Err(self.err(Some(pc), format!("r{reg} is {actual}, expected {ty}")))
        }
    }

    fn target(&self, pc: usize, target: u32) -> Result<(), VerifyError> {
        if (target as usize) < self.function.code.len() {
            Ok(())
        } else {
            Err(self.err(Some(pc), format!("jump target {target} out of range")))
        }
    }

    /// Arguments against parameter types, and the result against `dst`.
    fn call(
        &self,
        pc: usize,
        what: &str,
        params: &[Type],
        ret: &Type,
        args: &[Reg],
        dst: Option<Reg>,
    ) -> Result<(), VerifyError> {
        if args.len() != params.len() {
            return Err(self.err(
                Some(pc),
                format!(
                    "{what} takes {} arguments, got {}",
                    params.len(),
                    args.len()
                ),
            ));
        }
        for (arg, ty) in args.iter().zip(params) {
            self.expect(pc, *arg, ty)?;
        }
        if let Some(dst) = dst {
            self.expect(pc, dst, ret)?;
        }
        Ok(())
    }

    fn instr(&self, pc: usize, instr: &Instr) -> Result<(), VerifyError> {
        let m = self.module;
        match instr {
            Instr::Const { dst, index } => {
                let constant = m
                    .constants
                    .get(*index as usize)
                    .ok_or_else(|| self.err(Some(pc), format!("constant {index} out of range")))?;
                self.expect(pc, *dst, &constant.ty())
            }
            Instr::Move { dst, src } => {
                let ty = self.reg(pc, *src)?.clone();
                self.expect(pc, *dst, &ty)
            }
            Instr::Unary { op, dst, src } => {
                let src_ty = self.reg(pc, *src)?.clone();
                let dst_ty = match (op, &src_ty) {
                    (UnOp::Neg, Type::Int | Type::Float) => src_ty.clone(),
                    (UnOp::Not, Type::Bool) => Type::Bool,
                    (UnOp::IntToFloat, Type::Int) => Type::Float,
                    (UnOp::FloatToInt, Type::Float) => Type::Int,
                    (UnOp::ToStr, _) => Type::Str,
                    _ => {
                        return Err(
                            self.err(Some(pc), format!("{op:?} does not apply to {src_ty}"))
                        );
                    }
                };
                self.expect(pc, *dst, &dst_ty)
            }
            Instr::Binary { op, dst, a, b } => {
                let a_ty = self.reg(pc, *a)?.clone();
                self.expect(pc, *b, &a_ty)?;
                let ok = match op {
                    BinOp::Add => matches!(a_ty, Type::Int | Type::Float | Type::Str),
                    BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => a_ty.is_numeric(),
                    BinOp::Eq | BinOp::Ne => true,
                    BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        matches!(a_ty, Type::Int | Type::Float | Type::Str)
                    }
                    BinOp::And | BinOp::Or => a_ty == Type::Bool,
                };
                if !ok {
                    return Err(self.err(Some(pc), format!("{op:?} does not apply to {a_ty}")));
                }
                let dst_ty = if op.is_comparison() { Type::Bool } else { a_ty };
                self.expect(pc, *dst, &dst_ty)
            }
            Instr::Jump { target } => self.target(pc, *target),
            Instr::Branch {
                cond,
                then,
                otherwise,
            } => {
                self.expect(pc, *cond, &Type::Bool)?;
                self.target(pc, *then)?;
                self.target(pc, *otherwise)
            }
            Instr::Call { func, args, dst } => {
                let callee = m
                    .functions
                    .get(*func as usize)
                    .ok_or_else(|| self.err(Some(pc), format!("function {func} out of range")))?;
                self.call(pc, &callee.name, &callee.params, &callee.ret, args, *dst)
            }
            Instr::CallNative { import, args, dst } => {
                let import = m
                    .imports
                    .get(*import as usize)
                    .ok_or_else(|| self.err(Some(pc), format!("import {import} out of range")))?;
                let params: Vec<Type> = import.sig.params.iter().map(|p| p.ty.clone()).collect();
                self.call(pc, &import.name, &params, &import.sig.ret, args, *dst)
            }
            Instr::LoadVar { dst, var } => {
                let var = self.var(pc, *var)?;
                self.expect(pc, *dst, &var.ty)
            }
            Instr::StoreVar { var, src } => {
                let var = self.var(pc, *var)?;
                self.expect(pc, *src, &var.ty)
            }
            Instr::SelfEntity { dst } => self.expect(pc, *dst, &Type::Entity),
            Instr::Now { dst } => self.expect(pc, *dst, &Type::Float),
            Instr::Wait { seconds } => self.expect(pc, *seconds, &Type::Float),
            Instr::Collection { op, dst, args } => self.collection(pc, *op, *dst, args),
            Instr::Return { value: Some(reg) } => self.expect(pc, *reg, &self.function.ret),
            Instr::Return { value: None } => {
                if self.function.ret == Type::Unit {
                    Ok(())
                } else {
                    Err(self.err(Some(pc), format!("must return a {}", self.function.ret)))
                }
            }
        }
    }

    /// The operand and result types of a collection operation.
    fn collection(&self, pc: usize, op: CollOp, dst: Reg, args: &[Reg]) -> Result<(), VerifyError> {
        let arity = |n: usize| {
            if args.len() == n {
                Ok(())
            } else {
                Err(self.err(
                    Some(pc),
                    format!("{op:?} takes {n} arguments, got {}", args.len()),
                ))
            }
        };
        let list_of = |reg: Reg| match self.reg(pc, reg)? {
            Type::List(element) => Ok((**element).clone()),
            other => Err(self.err(Some(pc), format!("{op:?} needs a list, r{reg} is {other}"))),
        };
        let map_of = |reg: Reg| match self.reg(pc, reg)? {
            Type::Map(key, value) => Ok(((**key).clone(), (**value).clone())),
            other => Err(self.err(Some(pc), format!("{op:?} needs a map, r{reg} is {other}"))),
        };
        match op {
            CollOp::MakeList => {
                let element = match self.reg(pc, dst)? {
                    Type::List(element) => (**element).clone(),
                    other => {
                        return Err(self.err(
                            Some(pc),
                            format!("MakeList makes a list, r{dst} is {other}"),
                        ));
                    }
                };
                args.iter()
                    .try_for_each(|arg| self.expect(pc, *arg, &element))
            }
            CollOp::ListLen => {
                arity(1)?;
                list_of(args[0])?;
                self.expect(pc, dst, &Type::Int)
            }
            CollOp::ListGet => {
                arity(2)?;
                let element = list_of(args[0])?;
                self.expect(pc, args[1], &Type::Int)?;
                self.expect(pc, dst, &element)
            }
            CollOp::ListSet | CollOp::ListInsert => {
                arity(3)?;
                let element = list_of(args[0])?;
                self.expect(pc, args[1], &Type::Int)?;
                self.expect(pc, args[2], &element)?;
                self.expect(pc, dst, &Type::list(element))
            }
            CollOp::ListPush => {
                arity(2)?;
                let element = list_of(args[0])?;
                self.expect(pc, args[1], &element)?;
                self.expect(pc, dst, &Type::list(element))
            }
            CollOp::ListRemove => {
                arity(2)?;
                let element = list_of(args[0])?;
                self.expect(pc, args[1], &Type::Int)?;
                self.expect(pc, dst, &Type::list(element))
            }
            CollOp::MakeMap => {
                let (key, value) = map_of(dst)?;
                if args.len() % 2 != 0 {
                    return Err(self.err(Some(pc), "MakeMap takes key and value pairs"));
                }
                for pair in args.chunks(2) {
                    self.expect(pc, pair[0], &key)?;
                    self.expect(pc, pair[1], &value)?;
                }
                Ok(())
            }
            CollOp::MapLen => {
                arity(1)?;
                map_of(args[0])?;
                self.expect(pc, dst, &Type::Int)
            }
            CollOp::MapGet => {
                arity(2)?;
                let (key, value) = map_of(args[0])?;
                self.expect(pc, args[1], &key)?;
                self.expect(pc, dst, &value)
            }
            CollOp::MapHas => {
                arity(2)?;
                let (key, _) = map_of(args[0])?;
                self.expect(pc, args[1], &key)?;
                self.expect(pc, dst, &Type::Bool)
            }
            CollOp::MapSet => {
                arity(3)?;
                let (key, value) = map_of(args[0])?;
                self.expect(pc, args[1], &key)?;
                self.expect(pc, args[2], &value)?;
                self.expect(pc, dst, &Type::map(key, value))
            }
            CollOp::MapRemove => {
                arity(2)?;
                let (key, value) = map_of(args[0])?;
                self.expect(pc, args[1], &key)?;
                self.expect(pc, dst, &Type::map(key, value))
            }
            CollOp::MapKeys => {
                arity(1)?;
                let (key, _) = map_of(args[0])?;
                self.expect(pc, dst, &Type::list(key))
            }
            CollOp::MakeTuple => {
                let Type::Tuple(types) = self.reg(pc, dst)? else {
                    return Err(self.err(Some(pc), "MakeTuple makes a tuple"));
                };
                self.call(
                    pc,
                    "MakeTuple",
                    types,
                    &self.reg(pc, dst)?.clone(),
                    args,
                    None,
                )
            }
            CollOp::TupleGet(index) => {
                arity(1)?;
                let Type::Tuple(types) = self.reg(pc, args[0])? else {
                    return Err(self.err(
                        Some(pc),
                        format!(
                            "TupleGet needs a tuple, r{} is {}",
                            args[0],
                            self.reg(pc, args[0])?
                        ),
                    ));
                };
                let item = types.get(index as usize).ok_or_else(|| {
                    self.err(
                        Some(pc),
                        format!("tuple of {} has no element {index}", types.len()),
                    )
                })?;
                self.expect(pc, dst, item)
            }
        }
    }

    fn var(&self, pc: usize, var: u32) -> Result<&crate::module::Variable, VerifyError> {
        self.module
            .variables
            .get(var as usize)
            .ok_or_else(|| self.err(Some(pc), format!("variable {var} out of range")))
    }
}
