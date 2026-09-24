//! The interpreter.
//!
//! Registers live in one growable stack shared by all frames; calls do not
//! recurse on the Rust stack, so script recursion depth is bounded only by
//! [`Vm::max_depth`]. Every instruction costs one step of the caller's
//! [`Budget`], so a runaway loop ends in an error, not a hang.

use std::sync::Arc;

use crate::error::{ScriptError, ScriptErrorKind};
use crate::link::{FuncId, Instance, Program};
use crate::module::{BinOp, Instr, Reg, UnOp};
use crate::native::Host;
use crate::value::Value;

/// Instructions a call may execute.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub remaining: u64,
}

impl Budget {
    pub fn new(steps: u64) -> Self {
        Self { remaining: steps }
    }
}

struct Frame {
    func: u32,
    pc: usize,
    base: usize,
    /// Where the caller wants the result.
    ret_dst: Option<Reg>,
}

/// Execution state reused across calls (register stack and scratch).
pub struct Vm {
    regs: Vec<Value>,
    frames: Vec<Frame>,
    args: Vec<Value>,
    pub max_depth: usize,
}

impl Default for Vm {
    fn default() -> Self {
        Self { regs: Vec::new(), frames: Vec::new(), args: Vec::new(), max_depth: 256 }
    }
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `func` of `program` with `args` on `instance`.
    pub fn call(
        &mut self,
        program: &Program,
        instance: &mut Instance,
        func: FuncId,
        args: &[Value],
        host: &mut Host<'_>,
        budget: &mut Budget,
    ) -> Result<Value, ScriptError> {
        let module = Arc::clone(program.module());
        let function = module
            .functions
            .get(func.0 as usize)
            .ok_or_else(|| ScriptError::new(ScriptErrorKind::BadEntryCall(format!("no function {}", func.0))))?;
        if args.len() != function.params.len()
            || !args.iter().zip(&function.params).all(|(v, t)| v.fits(t))
        {
            return Err(ScriptError::new(ScriptErrorKind::BadEntryCall(format!(
                "`{}` takes ({}), got {:?}",
                function.name,
                function.params.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "),
                args
            ))));
        }
        if !Arc::ptr_eq(&instance.module, &module) {
            return Err(ScriptError::new(ScriptErrorKind::BadEntryCall(
                "instance does not belong to this program".into(),
            )));
        }

        let stack_base = self.regs.len();
        let frame_base = self.frames.len();
        self.push_frame(program, func.0, args.iter().cloned(), None);
        let result = self.run(program, instance, host, budget, frame_base);
        self.regs.truncate(stack_base);
        self.frames.truncate(frame_base);
        result
    }

    fn push_frame(
        &mut self,
        program: &Program,
        func: u32,
        args: impl Iterator<Item = Value>,
        ret_dst: Option<Reg>,
    ) {
        let base = self.regs.len();
        self.regs.extend(program.registers[func as usize].iter().cloned());
        for (slot, arg) in self.regs[base..].iter_mut().zip(args) {
            *slot = arg;
        }
        self.frames.push(Frame { func, pc: 0, base, ret_dst });
    }

    fn fail(&self, program: &Program, frame_base: usize, kind: ScriptErrorKind) -> ScriptError {
        let module = program.module();
        let trace = self.frames[frame_base..]
            .iter()
            .rev()
            .map(|f| (module.functions[f.func as usize].name.clone(), f.pc))
            .collect();
        ScriptError { kind, trace }
    }

    fn run(
        &mut self,
        program: &Program,
        instance: &mut Instance,
        host: &mut Host<'_>,
        budget: &mut Budget,
        frame_base: usize,
    ) -> Result<Value, ScriptError> {
        let module = Arc::clone(program.module());
        loop {
            if budget.remaining == 0 {
                return Err(self.fail(program, frame_base, ScriptErrorKind::BudgetExceeded));
            }
            budget.remaining -= 1;

            let frame = self.frames.last().expect("a frame is active");
            let (func, pc, base) = (frame.func, frame.pc, frame.base);
            let code = &module.functions[func as usize].code;
            let r = |reg: Reg| base + usize::from(reg);
            let mut next = pc + 1;

            match &code[pc] {
                Instr::Const { dst, index } => {
                    self.regs[r(*dst)] = program.constants[*index as usize].clone();
                }
                Instr::Move { dst, src } => {
                    self.regs[r(*dst)] = self.regs[r(*src)].clone();
                }
                Instr::Unary { op, dst, src } => {
                    self.regs[r(*dst)] = unary(*op, &self.regs[r(*src)]);
                }
                Instr::Binary { op, dst, a, b } => {
                    let value = binary(*op, &self.regs[r(*a)], &self.regs[r(*b)])
                        .map_err(|kind| self.fail(program, frame_base, kind))?;
                    self.regs[r(*dst)] = value;
                }
                Instr::Jump { target } => next = *target as usize,
                Instr::Branch { cond, then, otherwise } => {
                    let taken = matches!(self.regs[r(*cond)], Value::Bool(true));
                    next = if taken { *then } else { *otherwise } as usize;
                }
                Instr::Call { func: callee, args, dst } => {
                    if self.frames.len() - frame_base >= self.max_depth {
                        return Err(self.fail(program, frame_base, ScriptErrorKind::StackOverflow));
                    }
                    // The caller stays at the call site (for traces) and
                    // advances when the callee returns.
                    let values: Vec<Value> = args.iter().map(|a| self.regs[r(*a)].clone()).collect();
                    self.push_frame(program, *callee, values.into_iter(), *dst);
                    continue;
                }
                Instr::CallNative { import, args, dst } => {
                    let native = &program.natives[*import as usize];
                    let mut values = std::mem::take(&mut self.args);
                    values.clear();
                    values.extend(args.iter().map(|a| self.regs[r(*a)].clone()));
                    // A panicking native fails the call instead of unwinding
                    // through the VM into the game loop.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        native.call(host, &mut values)
                    }))
                    .unwrap_or_else(|panic| Err(ScriptError::native(panic_message(&*panic))));
                    let result = match result {
                        Ok(value) if value.fits(&native.sig.ret) => value,
                        Ok(value) => {
                            self.args = values;
                            let message = format!("returned {}, declared {}", value.kind(), native.sig.ret);
                            let kind = ScriptErrorKind::Native { name: native.name.clone(), message };
                            return Err(self.fail(program, frame_base, kind));
                        }
                        Err(err) => {
                            self.args = values;
                            let kind = match err.kind {
                                ScriptErrorKind::Native { message, .. } => {
                                    ScriptErrorKind::Native { name: native.name.clone(), message }
                                }
                                other => other,
                            };
                            return Err(self.fail(program, frame_base, kind));
                        }
                    };
                    for ((param, arg), value) in native.sig.params.iter().zip(args).zip(values.drain(..)) {
                        if param.inout && value.fits(&param.ty) {
                            self.regs[r(*arg)] = value;
                        }
                    }
                    self.args = values;
                    if let Some(dst) = dst {
                        self.regs[r(*dst)] = result;
                    }
                }
                Instr::LoadVar { dst, var } => {
                    self.regs[r(*dst)] = instance.vars[*var as usize].clone();
                }
                Instr::StoreVar { var, src } => {
                    instance.vars[*var as usize] = self.regs[r(*src)].clone();
                }
                Instr::SelfEntity { dst } => {
                    self.regs[r(*dst)] = Value::Entity(host.entity);
                }
                Instr::Return { value } => {
                    let value = value.map_or(Value::Unit, |v| self.regs[r(v)].clone());
                    let frame = self.frames.pop().expect("active");
                    self.regs.truncate(frame.base);
                    if self.frames.len() == frame_base {
                        return Ok(value);
                    }
                    let caller = self.frames.last_mut().expect("caller");
                    caller.pc += 1;
                    if let Some(dst) = frame.ret_dst {
                        self.regs[caller.base + usize::from(dst)] = value;
                    }
                    continue;
                }
            }
            self.frames.last_mut().expect("active").pc = next;
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    let message = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into());
    format!("panicked: {message}")
}

fn unary(op: UnOp, value: &Value) -> Value {
    match (op, value) {
        (UnOp::Neg, Value::Int(i)) => Value::Int(i.wrapping_neg()),
        (UnOp::Neg, Value::Float(f)) => Value::Float(-f),
        (UnOp::Not, Value::Bool(b)) => Value::Bool(!b),
        (UnOp::IntToFloat, Value::Int(i)) => Value::Float(*i as f64),
        // `as` saturates and maps NaN to 0.
        (UnOp::FloatToInt, Value::Float(f)) => Value::Int(*f as i64),
        (UnOp::ToStr, v) => Value::Str(display(v).into()),
        // The verifier rules out every other combination.
        _ => unreachable!("unverified unary operand"),
    }
}

fn display(value: &Value) -> String {
    match value {
        Value::Unit => "()".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Str(s) => s.to_string(),
        Value::Entity(e) => e.to_string(),
        Value::Component(c) => format!("{}({})", pulsar_scenedb::component::type_name(c.component), c.entity),
        Value::Object(o) => o.type_name().to_owned(),
    }
}

fn binary(op: BinOp, a: &Value, b: &Value) -> Result<Value, ScriptErrorKind> {
    use Value::{Bool, Float, Int, Str};
    Ok(match (op, a, b) {
        (BinOp::Add, Int(a), Int(b)) => Int(a.wrapping_add(*b)),
        (BinOp::Sub, Int(a), Int(b)) => Int(a.wrapping_sub(*b)),
        (BinOp::Mul, Int(a), Int(b)) => Int(a.wrapping_mul(*b)),
        (BinOp::Div | BinOp::Rem, Int(_), Int(0)) => return Err(ScriptErrorKind::DivideByZero),
        (BinOp::Div, Int(a), Int(b)) => Int(a.wrapping_div(*b)),
        (BinOp::Rem, Int(a), Int(b)) => Int(a.wrapping_rem(*b)),
        (BinOp::Add, Float(a), Float(b)) => Float(a + b),
        (BinOp::Sub, Float(a), Float(b)) => Float(a - b),
        (BinOp::Mul, Float(a), Float(b)) => Float(a * b),
        (BinOp::Div, Float(a), Float(b)) => Float(a / b),
        (BinOp::Rem, Float(a), Float(b)) => Float(a % b),
        (BinOp::Add, Str(a), Str(b)) => Str(format!("{a}{b}").into()),
        (BinOp::Eq, a, b) => Bool(a == b),
        (BinOp::Ne, a, b) => Bool(a != b),
        (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, a, b) => {
            let ordering = match (a, b) {
                (Int(a), Int(b)) => a.partial_cmp(b),
                (Float(a), Float(b)) => a.partial_cmp(b),
                (Str(a), Str(b)) => a.partial_cmp(b),
                _ => unreachable!("unverified comparison operands"),
            };
            // NaN compares false, like IEEE.
            Bool(ordering.is_some_and(|o| match op {
                BinOp::Lt => o.is_lt(),
                BinOp::Le => o.is_le(),
                BinOp::Gt => o.is_gt(),
                _ => o.is_ge(),
            }))
        }
        (BinOp::And, Bool(a), Bool(b)) => Bool(*a && *b),
        (BinOp::Or, Bool(a), Bool(b)) => Bool(*a || *b),
        _ => unreachable!("unverified binary operands"),
    })
}
