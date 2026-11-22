//! [`FunctionBuilder`]: one function's registers, code, labels, loops and scopes,
//! and the operator result-type tables.

use std::collections::HashMap;

use crate::module::{BinOp, DebugInfo, Instr, Reg, SourceLoc, UnOp};
use crate::types::{Type, TypeRegistry};

use super::*;

/// A position in a function's code, referred to before it is known.
/// Bind it ([`FunctionBuilder::bind`]) where the code it names begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Label(u32);

/// A named local of a function: its register, type and whether it may be
/// assigned (a `const` may not).
#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub reg: Reg,
    pub ty: Type,
    pub mutable: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Slot {
    Jump,
    Then,
    Otherwise,
}

#[derive(Default)]
pub(super) struct LabelState {
    pub(super) target: Option<u32>,
    pub(super) fixups: Vec<(usize, Slot)>,
}

pub(super) struct LoopTargets {
    break_to: Label,
    continue_to: Label,
}

/// Builds one function: registers, instructions, labels, loops, scopes and
/// debug info. Made by [`ModuleBuilder::begin_function`], handed back to
/// [`ModuleBuilder::finish_function`].
pub struct FunctionBuilder {
    pub(super) name: String,
    params: Vec<Type>,
    ret: Type,
    pub(super) registers: Vec<Type>,
    pub(super) code: Vec<Instr>,
    pub(super) debug: DebugInfo,
    pub(super) labels: Vec<LabelState>,
    pub(super) loops: Vec<LoopTargets>,
    scopes: Vec<HashMap<String, Local>>,
    loc: Option<SourceLoc>,
    /// The first misuse of a label, reported when the function is finished.
    pub(super) error: Option<BuildError>,
}

impl FunctionBuilder {
    pub(super) fn new(name: String, params: Vec<Type>, ret: Type) -> Self {
        Self {
            registers: params.clone(),
            name,
            params,
            ret,
            code: Vec::new(),
            debug: DebugInfo::default(),
            labels: Vec::new(),
            loops: Vec::new(),
            scopes: vec![HashMap::new()],
            loc: None,
            error: None,
        }
    }

    /// The function's return type.
    pub fn return_type(&self) -> &Type {
        &self.ret
    }

    // ---- registers ---------------------------------------------------------

    /// The register of parameter `index`.
    ///
    /// # Panics
    ///
    /// If the function has no such parameter.
    pub fn param_reg(&self, index: usize) -> Reg {
        assert!(index < self.params.len(), "no parameter {index}");
        index as Reg
    }

    /// A new register of type `ty`. Registers start at their type's default
    /// on every call.
    pub fn new_reg(&mut self, ty: Type) -> Result<Reg, BuildError> {
        if self.registers.len() >= usize::from(Reg::MAX) {
            return Err(BuildError::TooManyRegisters);
        }
        self.registers.push(ty);
        Ok((self.registers.len() - 1) as Reg)
    }

    /// The type of register `reg`.
    pub fn reg_type(&self, reg: Reg) -> Result<&Type, BuildError> {
        self.registers
            .get(usize::from(reg))
            .ok_or(BuildError::NoSuchRegister(reg))
    }

    // ---- instructions and debug info --------------------------------------------

    /// The pc the next instruction gets.
    pub fn here(&self) -> u32 {
        self.code.len() as u32
    }

    /// The source location instructions emitted from now on are recorded
    /// against ([`DebugInfo`]), until it is set again.
    pub fn set_loc(&mut self, loc: SourceLoc) {
        self.loc = Some(loc);
    }

    /// Append `instr`; returns its pc. It is recorded in the debug info under
    /// the current location.
    pub fn emit(&mut self, instr: Instr) -> usize {
        let pc = self.code.len();
        if let Some(loc) = &self.loc {
            self.debug.record(pc as u32, loc);
        }
        self.code.push(instr);
        pc
    }

    /// `dst = src`.
    pub fn mov(&mut self, dst: Reg, src: Reg) -> usize {
        self.emit(Instr::Move { dst, src })
    }

    /// `return` (a `unit` function) or `return value`.
    pub fn ret(&mut self, value: Option<Reg>) -> usize {
        self.emit(Instr::Return { value })
    }

    /// Suspend for `seconds` (a `float` register) of game time.
    pub fn wait(&mut self, seconds: Reg) -> usize {
        self.emit(Instr::Wait { seconds })
    }

    /// `dst = op(src)` into a new register of the result type
    /// ([`unary_result`]), or `None` when `op` does not apply to `src`'s type.
    pub fn unary(&mut self, op: UnOp, src: Reg) -> Result<Option<Reg>, BuildError> {
        let Some(result) = unary_result(op, self.reg_type(src)?) else {
            return Ok(None);
        };
        let dst = self.new_reg(result)?;
        self.emit(Instr::Unary { op, dst, src });
        Ok(Some(dst))
    }

    /// `dst = a op b` into a new register of the result type
    /// ([`binary_result`]), or `None` when `op` does not apply to the
    /// operands' type (which must be one type).
    pub fn binary(&mut self, op: BinOp, a: Reg, b: Reg) -> Result<Option<Reg>, BuildError> {
        let ty = self.reg_type(a)?.clone();
        if *self.reg_type(b)? != ty {
            return Ok(None);
        }
        let Some(result) = binary_result(op, &ty) else {
            return Ok(None);
        };
        let dst = self.new_reg(result)?;
        self.emit(Instr::Binary { op, dst, a, b });
        Ok(Some(dst))
    }

    /// An instruction that jumps to itself: where control cannot arrive (a
    /// function whose every path returned), so the end of the code still has
    /// an instruction for jumps to land on.
    pub fn unreachable(&mut self) -> usize {
        let here = self.here();
        self.emit(Instr::Jump { target: here })
    }

    // ---- labels ------------------------------------------------------------------

    /// A new, unbound label.
    pub fn new_label(&mut self) -> Label {
        self.labels.push(LabelState::default());
        Label((self.labels.len() - 1) as u32)
    }

    /// Bind `label` to the next instruction emitted. Every jump and branch
    /// that named it (before or after) goes there. A label is bound once.
    pub fn bind(&mut self, label: Label) {
        let here = self.here();
        let state = &mut self.labels[label.0 as usize];
        if state.target.is_some() {
            self.error.get_or_insert(BuildError::LabelBoundTwice {
                function: self.name.clone(),
            });
            return;
        }
        state.target = Some(here);
        for (pc, slot) in std::mem::take(&mut state.fixups) {
            set_target(&mut self.code[pc], slot, here);
        }
    }

    /// A label bound right now.
    pub fn label_here(&mut self) -> Label {
        let label = self.new_label();
        self.bind(label);
        label
    }

    fn refer(&mut self, label: Label, pc: usize, slot: Slot) {
        let state = &mut self.labels[label.0 as usize];
        match state.target {
            Some(target) => set_target(&mut self.code[pc], slot, target),
            None => state.fixups.push((pc, slot)),
        }
    }

    /// Jump to `label`; returns the pc of the jump.
    pub fn jump(&mut self, label: Label) -> usize {
        let pc = self.emit(Instr::Jump { target: 0 });
        self.refer(label, pc, Slot::Jump);
        pc
    }

    /// Branch on the `bool` register `cond`; returns the pc of the branch.
    pub fn branch(&mut self, cond: Reg, then: Label, otherwise: Label) -> usize {
        let pc = self.emit(Instr::Branch {
            cond,
            then: 0,
            otherwise: 0,
        });
        self.refer(then, pc, Slot::Then);
        self.refer(otherwise, pc, Slot::Otherwise);
        pc
    }

    // ---- loops -------------------------------------------------------------------

    /// Enter a loop: `break` jumps to `break_to` and `continue` to
    /// `continue_to`, labels the front end binds where its loop puts them.
    pub fn enter_loop(&mut self, break_to: Label, continue_to: Label) {
        self.loops.push(LoopTargets {
            break_to,
            continue_to,
        });
    }

    /// Leave the innermost loop.
    pub fn exit_loop(&mut self) {
        self.loops.pop();
    }

    /// Whether a loop is being lowered.
    pub fn in_loop(&self) -> bool {
        !self.loops.is_empty()
    }

    /// `break`: jump to the innermost loop's end.
    pub fn break_jump(&mut self) -> Result<usize, BuildError> {
        let target = self
            .loops
            .last()
            .ok_or(BuildError::NotInLoop("break"))?
            .break_to;
        Ok(self.jump(target))
    }

    /// `continue`: jump to the innermost loop's continue point.
    pub fn continue_jump(&mut self) -> Result<usize, BuildError> {
        let target = self
            .loops
            .last()
            .ok_or(BuildError::NotInLoop("continue"))?
            .continue_to;
        Ok(self.jump(target))
    }

    // ---- scopes and locals --------------------------------------------------------

    /// Open a scope for named locals.
    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    /// Close the innermost scope. The outermost (parameters) is never closed.
    pub fn pop_scope(&mut self) {
        if self.scopes.len() > 1 {
            self.scopes.pop();
        }
    }

    /// Whether `name` is already declared in the innermost scope.
    pub fn declared_in_scope(&self, name: &str) -> bool {
        self.scopes.last().is_some_and(|s| s.contains_key(name))
    }

    /// Name a register in the innermost scope (shadowing an outer local of
    /// that name).
    pub fn bind_local(&mut self, name: impl Into<String>, reg: Reg, ty: Type, mutable: bool) {
        self.scopes
            .last_mut()
            .expect("a function always has its outermost scope")
            .insert(name.into(), Local { reg, ty, mutable });
    }

    /// A new register of type `ty`, named `name` in the innermost scope.
    pub fn declare_local(
        &mut self,
        name: impl Into<String>,
        ty: Type,
        mutable: bool,
    ) -> Result<Reg, BuildError> {
        let reg = self.new_reg(ty.clone())?;
        self.bind_local(name, reg, ty, mutable);
        Ok(reg)
    }

    /// The innermost local called `name`.
    pub fn lookup(&self, name: &str) -> Option<&Local> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }
}

fn set_target(instr: &mut Instr, slot: Slot, target: u32) {
    match (instr, slot) {
        (Instr::Jump { target: t }, Slot::Jump) => *t = target,
        (Instr::Branch { then, .. }, Slot::Then) => *then = target,
        (Instr::Branch { otherwise, .. }, Slot::Otherwise) => *otherwise = target,
        (other, _) => unreachable!("a label fixup recorded for {other:?}"),
    }
}

/// The result type of `op` on an operand of type `src`, or `None` when the
/// verifier would refuse it.
pub fn unary_result(op: UnOp, src: &Type) -> Option<Type> {
    match (op, src) {
        (UnOp::Neg, Type::Int | Type::Float) => Some(src.clone()),
        (UnOp::Not, Type::Bool) => Some(Type::Bool),
        (UnOp::IntToFloat, Type::Int) => Some(Type::Float),
        (UnOp::IntToI32Checked, Type::Int) => Some(Type::Int),
        (UnOp::FloatToInt, Type::Float) => Some(Type::Int),
        (UnOp::ToStr, _) => Some(Type::Str),
        _ => None,
    }
}

/// The result type of `op` on two operands of type `operand`, or `None` when
/// the verifier or the linker would refuse it: arithmetic on numbers (`+` also
/// joins strings), ordering on numbers and strings, `and`/`or` on booleans,
/// and equality on every builtin type and on a registered value type that
/// registers equality.
pub fn binary_result(op: BinOp, operand: &Type) -> Option<Type> {
    match op {
        BinOp::Add if matches!(operand, Type::Int | Type::Float | Type::Str) => {
            Some(operand.clone())
        }
        BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem if operand.is_numeric() => {
            Some(operand.clone())
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
            if matches!(operand, Type::Int | Type::Float | Type::Str) =>
        {
            Some(Type::Bool)
        }
        BinOp::Eq | BinOp::Ne
            if matches!(
                operand,
                Type::Int
                    | Type::Float
                    | Type::Str
                    | Type::Bool
                    | Type::Entity
                    | Type::Component(_)
            ) || TypeRegistry::global().supports_eq(operand) =>
        {
            Some(Type::Bool)
        }
        BinOp::And | BinOp::Or if *operand == Type::Bool => Some(Type::Bool),
        _ => None,
    }
}
