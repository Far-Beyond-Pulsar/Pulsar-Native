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

#[derive(Clone, Debug)]
struct Frame {
    func: u32,
    pc: usize,
    base: usize,
    /// Where the caller wants the result.
    ret_dst: Option<Reg>,
}

/// How a started or resumed call ended.
#[derive(Debug)]
pub enum Completion {
    Returned(Value),
    /// The call executed `Wait`: resume `continuation` after `seconds` of
    /// game time.
    Waiting { seconds: f64, continuation: Continuation },
}

/// A suspended call: its frames and registers, for [`Vm::resume`]. Tied to
/// the module it was running (it survives relinking, not reloading).
#[derive(Debug)]
pub struct Continuation {
    module: Arc<crate::module::Module>,
    /// Frames with bases relative to `regs`.
    frames: Vec<Frame>,
    regs: Vec<Value>,
}

impl Continuation {
    pub fn module(&self) -> &Arc<crate::module::Module> {
        &self.module
    }

    /// Names of the suspended functions, outermost first.
    pub fn functions(&self) -> Vec<&str> {
        self.frames.iter().map(|f| self.module.functions[f.func as usize].name.as_str()).collect()
    }

    /// Move this suspended call onto `module`, a new version of the module
    /// it was running (a class reload, #862), if its code layout is
    /// compatible. For every suspended frame, the new module must have a
    /// function with the same name and the same parameter, return and
    /// register types and the same number of instructions; the innermost
    /// frame must still resume right after a `Wait`, and every outer
    /// frame must still be at a `Call` of the next frame's function. Then
    /// the call continues in the new code at the same pcs with its
    /// registers as they are. Otherwise the reason is returned and the
    /// call cannot continue.
    pub fn rebase(&self, module: &Arc<crate::module::Module>) -> Result<Continuation, String> {
        let mut frames = Vec::with_capacity(self.frames.len());
        let mut new_indices = Vec::with_capacity(self.frames.len());
        for frame in &self.frames {
            let old = &self.module.functions[frame.func as usize];
            let (index, new) = module
                .function(&old.name)
                .ok_or_else(|| format!("function `{}` no longer exists", old.name))?;
            if new.params != old.params || new.ret != old.ret || new.registers != old.registers {
                return Err(format!("function `{}` changed its parameters or registers", old.name));
            }
            if new.code.len() != old.code.len() {
                return Err(format!(
                    "function `{}` changed its instruction count ({} -> {})",
                    old.name,
                    old.code.len(),
                    new.code.len()
                ));
            }
            new_indices.push(index);
            frames.push(Frame { func: index, ..frame.clone() });
        }
        for (depth, frame) in frames.iter().enumerate() {
            let code = &module.functions[frame.func as usize].code;
            let name = &module.functions[frame.func as usize].name;
            match new_indices.get(depth + 1) {
                // Outer frame: parked on the call of the next frame.
                Some(&callee) => {
                    if !matches!(code.get(frame.pc), Some(Instr::Call { func, .. }) if *func == callee) {
                        return Err(format!("function `{name}` no longer calls the waiting function at {}", frame.pc));
                    }
                }
                // Innermost: resumes right after its `Wait`.
                None => {
                    let waited = frame.pc.checked_sub(1).and_then(|pc| code.get(pc));
                    if !matches!(waited, Some(Instr::Wait { .. })) {
                        return Err(format!("function `{name}` no longer waits at {}", frame.pc.saturating_sub(1)));
                    }
                }
            }
        }
        Ok(Continuation { module: Arc::clone(module), frames, regs: self.regs.clone() })
    }
}

/// Default [`Vm::max_depth`].
pub const DEFAULT_MAX_DEPTH: usize = 256;

/// Execution state reused across calls (register stack and scratch).
pub struct Vm {
    regs: Vec<Value>,
    frames: Vec<Frame>,
    args: Vec<Value>,
    /// Call depth limit ([`ScriptErrorKind::StackOverflow`] past it).
    pub max_depth: usize,
    /// Integer `Add`, `Sub`, `Mul`, `Div`, `Rem` and `Neg` that overflow,
    /// and `FloatToInt` of a value outside `int` (or NaN), raise
    /// [`ScriptErrorKind::Overflow`] instead of wrapping or saturating
    /// (#858). Off by default, like a Rust release build; the engine turns
    /// it on in the editor and Play-in-Editor and off in shipping builds.
    pub checked_arithmetic: bool,
}

impl Default for Vm {
    fn default() -> Self {
        Self {
            regs: Vec::new(),
            frames: Vec::new(),
            args: Vec::new(),
            max_depth: DEFAULT_MAX_DEPTH,
            checked_arithmetic: false,
        }
    }
}

impl Vm {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `func` of `program` with `args` on `instance` to completion. A
    /// function that waits fails with [`ScriptErrorKind::Suspended`]; use
    /// [`start`](Self::start) where waiting is allowed.
    pub fn call(
        &mut self,
        program: &Program,
        instance: &mut Instance,
        func: FuncId,
        args: &[Value],
        host: &mut Host<'_>,
        budget: &mut Budget,
    ) -> Result<Value, ScriptError> {
        match self.start(program, instance, func, args, host, budget)? {
            Completion::Returned(value) => Ok(value),
            Completion::Waiting { .. } => Err(ScriptError::new(ScriptErrorKind::Suspended)),
        }
    }

    /// Run `func` of `program` with `args` on `instance` until it returns
    /// or waits.
    pub fn start(
        &mut self,
        program: &Program,
        instance: &mut Instance,
        func: FuncId,
        args: &[Value],
        host: &mut Host<'_>,
        budget: &mut Budget,
    ) -> Result<Completion, ScriptError> {
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

    /// Continue a suspended call until it returns or waits again.
    pub fn resume(
        &mut self,
        program: &Program,
        instance: &mut Instance,
        continuation: Continuation,
        host: &mut Host<'_>,
        budget: &mut Budget,
    ) -> Result<Completion, ScriptError> {
        if !Arc::ptr_eq(&continuation.module, program.module())
            || !Arc::ptr_eq(&instance.module, program.module())
        {
            return Err(ScriptError::new(ScriptErrorKind::BadEntryCall(
                "continuation does not belong to this program".into(),
            )));
        }
        let stack_base = self.regs.len();
        let frame_base = self.frames.len();
        self.regs.extend(continuation.regs);
        self.frames.extend(continuation.frames.into_iter().map(|mut f| {
            f.base += stack_base;
            f
        }));
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
        let frames = self.frames[frame_base..].iter().rev();
        let trace = frames.clone().map(|f| (module.functions[f.func as usize].name.clone(), f.pc)).collect();
        let locations = frames.map(|f| module.functions[f.func as usize].location(f.pc).cloned()).collect();
        ScriptError { kind, trace, locations }
    }

    fn run(
        &mut self,
        program: &Program,
        instance: &mut Instance,
        host: &mut Host<'_>,
        budget: &mut Budget,
        frame_base: usize,
    ) -> Result<Completion, ScriptError> {
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
                    let value = unary(*op, &self.regs[r(*src)], self.checked_arithmetic)
                        .map_err(|kind| self.fail(program, frame_base, kind))?;
                    self.regs[r(*dst)] = value;
                }
                Instr::Binary { op, dst, a, b } => {
                    let value = binary(*op, &self.regs[r(*a)], &self.regs[r(*b)], self.checked_arithmetic)
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
                Instr::Now { dst } => {
                    self.regs[r(*dst)] = Value::Float(host.time);
                }
                Instr::Wait { seconds } => {
                    let seconds = self.regs[r(*seconds)].as_float().unwrap_or(0.0);
                    // NaN and negative waits resume on the next opportunity.
                    let seconds = if seconds > 0.0 { seconds } else { 0.0 };
                    self.frames.last_mut().expect("active").pc = next;
                    let stack_base = self.frames[frame_base].base;
                    let frames = self
                        .frames
                        .drain(frame_base..)
                        .map(|mut f| {
                            f.base -= stack_base;
                            f
                        })
                        .collect();
                    let regs = self.regs.split_off(stack_base);
                    let continuation = Continuation { module: Arc::clone(&module), frames, regs };
                    return Ok(Completion::Waiting { seconds, continuation });
                }
                Instr::Return { value } => {
                    let value = value.map_or(Value::Unit, |v| self.regs[r(v)].clone());
                    let frame = self.frames.pop().expect("active");
                    self.regs.truncate(frame.base);
                    if self.frames.len() == frame_base {
                        return Ok(Completion::Returned(value));
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

fn overflow(op: &str) -> ScriptErrorKind {
    ScriptErrorKind::Overflow { op: op.to_owned() }
}

fn unary(op: UnOp, value: &Value, checked: bool) -> Result<Value, ScriptErrorKind> {
    Ok(match (op, value) {
        (UnOp::Neg, Value::Int(i)) if checked => Value::Int(i.checked_neg().ok_or_else(|| overflow("Neg"))?),
        (UnOp::Neg, Value::Int(i)) => Value::Int(i.wrapping_neg()),
        (UnOp::Neg, Value::Float(f)) => Value::Float(-f),
        (UnOp::Not, Value::Bool(b)) => Value::Bool(!b),
        (UnOp::IntToFloat, Value::Int(i)) => Value::Float(*i as f64),
        // `i64::MAX as f64` rounds up to 2^63, which is already out of range.
        (UnOp::FloatToInt, Value::Float(f)) if checked && !(f.is_finite() && *f >= -(2f64.powi(63)) && *f < 2f64.powi(63)) => {
            return Err(overflow("FloatToInt"));
        }
        // `as` saturates and maps NaN to 0.
        (UnOp::FloatToInt, Value::Float(f)) => Value::Int(*f as i64),
        (UnOp::ToStr, v) => Value::Str(display(v).into()),
        // The verifier rules out every other combination.
        _ => unreachable!("unverified unary operand"),
    })
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

fn binary(op: BinOp, a: &Value, b: &Value, checked: bool) -> Result<Value, ScriptErrorKind> {
    use Value::{Bool, Float, Int, Str};
    if checked {
        if let (Int(x), Int(y)) = (a, b) {
            let result = match op {
                BinOp::Add => Some(x.checked_add(*y)),
                BinOp::Sub => Some(x.checked_sub(*y)),
                BinOp::Mul => Some(x.checked_mul(*y)),
                BinOp::Div | BinOp::Rem if *y == 0 => return Err(ScriptErrorKind::DivideByZero),
                BinOp::Div => Some(x.checked_div(*y)),
                BinOp::Rem => Some(x.checked_rem(*y)),
                _ => None,
            };
            if let Some(result) = result {
                return result.map(Int).ok_or_else(|| overflow(&format!("{op:?}")));
            }
        }
    }
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
