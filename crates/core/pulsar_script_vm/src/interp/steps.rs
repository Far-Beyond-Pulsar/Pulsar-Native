//! The interpreter's instruction and frame helpers: what the arms of
//! [`Vm::run`] call, kept apart so the loop reads as a table.

use std::sync::Arc;

use crate::compiled::{CompiledCode, Cx, Exit};
use crate::debugger::StopReason;
use crate::error::{ScriptError, ScriptErrorKind};
use crate::exec::{self, binary, binary_scalar, unary};
use crate::link::{Instance, Program};
use crate::module::{BinOp, CollOp, Module, Reg, UnOp};
use crate::native::{Host, NativeFn};
use crate::value::Value;

use super::{Budget, Completion, Vm};

/// A call from the running frame: the callee, the caller's register base, the
/// argument registers and where the result goes.
pub(super) struct CallSite<'a> {
    pub(super) callee: u32,
    pub(super) caller_base: usize,
    pub(super) args: &'a [Reg],
    pub(super) dst: Option<Reg>,
}

/// The seconds a `Wait` register asks for. NaN and negative waits resume on
/// the next opportunity.
#[inline]
pub(super) fn wait_seconds(value: &Value) -> f64 {
    let seconds = value.as_float().unwrap_or(0.0);
    if seconds > 0.0 { seconds } else { 0.0 }
}

/// A latent native (`wait::frames`, ..) asked for the call to suspend: park it
/// right after the call; the runtime reads what it is waiting for from the
/// host's latent state.
#[inline]
pub(super) fn latent_suspend_requested(host: &Host<'_>) -> bool {
    host.latent
        .as_deref()
        .is_some_and(crate::latent::Latent::suspend_requested)
}

impl Vm {
    /// Charges one instruction to the budget.
    #[inline]
    pub(super) fn charge(
        &self,
        program: &Program,
        frame_base: usize,
        budget: &mut Budget,
    ) -> Result<(), ScriptError> {
        if budget.remaining == 0 {
            return Err(self.fail(program, frame_base, ScriptErrorKind::BudgetExceeded));
        }
        budget.remaining -= 1;
        Ok(())
    }

    /// `Call`: pushes the callee's frame. The caller stays at the call site
    /// (for traces) and advances when the callee returns.
    #[inline]
    pub(super) fn enter_call(
        &mut self,
        program: &Program,
        frame_base: usize,
        site: CallSite<'_>,
    ) -> Result<(), ScriptError> {
        if self.frames.len() - frame_base >= self.max_depth {
            return Err(self.fail(program, frame_base, ScriptErrorKind::StackOverflow));
        }
        self.push_frame_from(program, site.callee, site.caller_base, site.args, site.dst);
        Ok(())
    }

    /// Parks the call to resume at `resume_pc` after `seconds`.
    #[inline]
    pub(super) fn park(
        &mut self,
        program: &Program,
        frame_base: usize,
        resume_pc: usize,
        seconds: f64,
    ) -> Completion {
        self.frames.last_mut().expect("active").pc = resume_pc;
        self.suspend(program, frame_base, seconds)
    }

    /// The generated code that runs the active frame, if any. Generated code
    /// runs the frame unless a debugger is attached and the function kept its
    /// instructions: the interpreter runs those, where the debugger can stop.
    /// A function with no instructions (a module linked with generated code
    /// carries none) always runs its generated code: there is nothing to
    /// interpret, and indexing the empty stream would panic.
    #[inline]
    pub(super) fn generated_code<'p>(
        &self,
        program: &'p Program,
        module: &Module,
    ) -> Option<&'p Arc<dyn CompiledCode>> {
        program.compiled.as_ref().filter(|_| {
            let func = self.frames.last().expect("a frame is active").func;
            self.debugger.is_none() || module.functions[func as usize].code.is_empty()
        })
    }

    /// Pauses the call when the debugger wants to stop before the instruction
    /// at `pc` of `func`. Source locations are only looked up for the
    /// debugger; without one the interpreter pays nothing for them.
    #[inline]
    pub(super) fn debugger_pause(
        &mut self,
        program: &Program,
        instance: &Instance,
        frame_base: usize,
        func: u32,
        pc: usize,
    ) -> Option<Completion> {
        let function = &program.module().functions[func as usize];
        let reason = self.debugger.as_mut().and_then(|debugger| {
            debugger.should_stop(
                &function.name,
                pc,
                self.frames.len() - frame_base,
                function.location(pc),
            )
        })?;
        Some(self.pause(program, instance, frame_base, reason))
    }

    /// `Unary`: writes `op` of the register `src` to the register `dst` (both
    /// absolute stack indices).
    #[inline]
    pub(super) fn unary_instr(&mut self, op: UnOp, dst: usize, src: usize) -> Result<(), ScriptErrorKind> {
        self.regs[dst] = unary(op, &self.regs[src], self.checked_arithmetic)?;
        Ok(())
    }

    /// `Binary`: writes `op` of the registers `(a, b)` to the register `dst`
    /// (absolute stack indices): the scalar fast path, else the general
    /// operation.
    #[inline]
    pub(super) fn binary_instr(
        &mut self,
        op: BinOp,
        dst: usize,
        (a, b): (usize, usize),
    ) -> Result<(), ScriptErrorKind> {
        let (x, y) = (&self.regs[a], &self.regs[b]);
        let value = match binary_scalar(op, x, y, self.checked_arithmetic) {
            Some(value) => value,
            None => binary(op, x, y, self.checked_arithmetic)?,
        };
        self.regs[dst] = value;
        Ok(())
    }

    /// Runs the active frame's generated code until it must call, return, wait
    /// or fail. It charges the budget itself, per instruction.
    #[inline]
    pub(super) fn step_generated(
        &mut self,
        code: &dyn CompiledCode,
        program: &Program,
        vars: &mut [Value],
        host: &mut Host<'_>,
        budget: &mut Budget,
    ) -> Result<Exit, ScriptErrorKind> {
        let frame = self.frames.last_mut().expect("a frame is active");
        let func = frame.func;
        let registers = program.registers[func as usize].len();
        let mut cx = Cx {
            host: &mut *host,
            vars,
            budget: &mut *budget,
            natives: &program.natives,
            constants: &program.constants,
            checked: self.checked_arithmetic,
            scratch: &mut self.args,
        };
        let regs = &mut self.regs[frame.base..frame.base + registers];
        code.step(func, &mut frame.pc, regs, &mut cx)
    }

    /// Handles how generated code stopped. `Some` when the call is over (it
    /// returned from its outermost frame or waits); `None` to keep running.
    #[inline]
    pub(super) fn after_generated_exit(
        &mut self,
        program: &Program,
        frame_base: usize,
        exit: Result<Exit, ScriptErrorKind>,
    ) -> Result<Option<Completion>, ScriptError> {
        match exit {
            Err(kind) => Err(self.fail(program, frame_base, kind)),
            Ok(Exit::Call {
                func: callee,
                args,
                dst,
            }) => {
                if self.frames.len() - frame_base >= self.max_depth {
                    return Err(self.fail(program, frame_base, ScriptErrorKind::StackOverflow));
                }
                self.push_frame(program, callee, args.into_iter(), dst);
                Ok(None)
            }
            Ok(Exit::Return(value)) => Ok(self.finish_frame(value, frame_base)),
            Ok(Exit::Wait(seconds)) => Ok(Some(self.suspend(program, frame_base, seconds))),
        }
    }

    /// Stops the call at the debugger's request: a snapshot of it, and the
    /// continuation that resumes it.
    pub(super) fn pause(
        &mut self,
        program: &Program,
        instance: &Instance,
        frame_base: usize,
        reason: StopReason,
    ) -> Completion {
        let snapshot = self.snapshot(program, instance, frame_base, reason);
        let continuation = match self.suspend(program, frame_base, 0.0) {
            Completion::Waiting { continuation, .. } => continuation,
            Completion::Returned(_) => unreachable!("suspend always yields a continuation"),
            Completion::Paused { .. } => unreachable!("suspend does not pause"),
        };
        Completion::Paused {
            snapshot,
            continuation,
        }
    }

    /// `CallNative`: calls `native` with the registers `args` of the frame at
    /// `base`, writes `inout` parameters back and the result to `dst`.
    #[inline]
    pub(super) fn call_native_instr(
        &mut self,
        native: &NativeFn,
        host: &mut Host<'_>,
        base: usize,
        args: &[Reg],
        dst: Option<Reg>,
    ) -> Result<(), ScriptErrorKind> {
        let r = |reg: Reg| base + usize::from(reg);
        let mut values = std::mem::take(&mut self.args);
        values.clear();
        values.extend(args.iter().map(|a| self.regs[r(*a)].clone()));
        let result = exec::call_native(native, host, &mut values);
        let result = match result {
            Ok(value) => value,
            Err(kind) => {
                self.args = values;
                return Err(kind);
            }
        };
        for ((param, arg), value) in native.sig.params.iter().zip(args).zip(values.drain(..)) {
            if param.inout && value.fits(&param.ty) {
                self.regs[r(*arg)] = value;
            }
        }
        self.args = values;
        if let Some(dst) = dst {
            self.regs[r(dst)] = result;
        }
        Ok(())
    }

    /// `Collection`: applies `op` to the registers `args` of the frame at
    /// `base`, writing the result to `dst`.
    #[inline]
    pub(super) fn collection_instr(
        &mut self,
        op: CollOp,
        base: usize,
        dst: Reg,
        args: &[Reg],
    ) -> Result<(), ScriptErrorKind> {
        let r = |reg: Reg| base + usize::from(reg);
        let mut values = std::mem::take(&mut self.args);
        values.clear();
        for (position, arg) in args.iter().enumerate() {
            // A result written back over its own first argument
            // is moved, not cloned, so the edit is in place.
            let moved = position == 0 && *arg == dst && !args[1..].contains(arg);
            values.push(if moved {
                std::mem::replace(&mut self.regs[r(*arg)], Value::Unit)
            } else {
                self.regs[r(*arg)].clone()
            });
        }
        let result = exec::collection(op, &mut values);
        values.clear();
        self.args = values;
        self.regs[r(dst)] = result?;
        Ok(())
    }
}
