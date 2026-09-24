//! Structural and type verification of a [`Module`], independent of any
//! native registry: every index is in range, every instruction's operand
//! types agree, every call matches its callee's declared signature, and
//! control cannot fall off the end of a function. A verified module cannot
//! make the interpreter index out of bounds or see a value of the wrong
//! type. (Imports are checked against the actual natives at link time.)

use std::collections::HashSet;

use crate::error::VerifyError;
use crate::module::{BinOp, Instr, Module, Reg, UnOp, FORMAT_VERSION};
use crate::types::Type;

pub fn verify(module: &Module) -> Result<(), VerifyError> {
    if module.format_version != FORMAT_VERSION {
        return Err(VerifyError::module(format!(
            "format version {} (this VM reads {FORMAT_VERSION})",
            module.format_version
        )));
    }

    let mut names = HashSet::new();
    for function in &module.functions {
        if !names.insert(function.name.as_str()) {
            return Err(VerifyError::module(format!("function `{}` defined twice", function.name)));
        }
    }
    let mut imports = HashSet::new();
    for import in &module.imports {
        if !imports.insert(import.name.as_str()) {
            return Err(VerifyError::module(format!("`{}` imported twice", import.name)));
        }
    }
    let mut vars = HashSet::new();
    for var in &module.variables {
        if !vars.insert(var.name.as_str()) {
            return Err(VerifyError::module(format!("variable `{}` declared twice", var.name)));
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

    for function in &module.functions {
        FunctionVerifier { module, function }.verify()?;
    }
    Ok(())
}

struct FunctionVerifier<'a> {
    module: &'a Module,
    function: &'a crate::module::Function,
}

impl FunctionVerifier<'_> {
    fn err(&self, pc: Option<usize>, message: impl Into<String>) -> VerifyError {
        VerifyError { function: Some(self.function.name.clone()), pc, message: message.into() }
    }

    fn verify(&self) -> Result<(), VerifyError> {
        let f = self.function;
        if f.registers.len() > usize::from(Reg::MAX) + 1 {
            return Err(self.err(None, "too many registers"));
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
                format!("{what} takes {} arguments, got {}", params.len(), args.len()),
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
                    (UnOp::ToStr, Type::Object(_)) => {
                        return Err(self.err(Some(pc), "cannot convert an object to a string"))
                    }
                    (UnOp::ToStr, _) => Type::Str,
                    _ => return Err(self.err(Some(pc), format!("{op:?} does not apply to {src_ty}"))),
                };
                self.expect(pc, *dst, &dst_ty)
            }
            Instr::Binary { op, dst, a, b } => {
                let a_ty = self.reg(pc, *a)?.clone();
                self.expect(pc, *b, &a_ty)?;
                let ok = match op {
                    BinOp::Add => matches!(a_ty, Type::Int | Type::Float | Type::Str),
                    BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => a_ty.is_numeric(),
                    BinOp::Eq | BinOp::Ne => !matches!(a_ty, Type::Object(_)),
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
            Instr::Branch { cond, then, otherwise } => {
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

    fn var(&self, pc: usize, var: u32) -> Result<&crate::module::Variable, VerifyError> {
        self.module
            .variables
            .get(var as usize)
            .ok_or_else(|| self.err(Some(pc), format!("variable {var} out of range")))
    }
}
