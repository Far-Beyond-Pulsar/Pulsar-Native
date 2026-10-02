//! Code generated from a module, run by the same VM.
//!
//! `pulsar_script_codegen` turns a verified [`Module`] into Rust. What it
//! generates is only the part of the interpreter that decodes and
//! dispatches instructions: one `step` per function, whose control flow
//! (jumps, branches, loops) is Rust control flow and whose operations call
//! the same [`exec`](crate::exec) functions the interpreter uses.
//! Everything else is shared with the interpreter, not re-implemented:
//!
//! - linking ([`Program::link_compiled`]): import resolution, capability
//!   checks, constants, variable defaults, event subscriptions;
//! - instances ([`Instance`](crate::Instance)): per-instance variables;
//! - calls, returns, call-depth limit and error traces
//!   ([`Vm`](crate::Vm)), and waiting: a compiled call suspends into the
//!   same [`Continuation`](crate::Continuation);
//! - the instruction budget and arithmetic policy (through [`Cx`]).
//!
//! # The step contract
//!
//! [`CompiledCode::step`] runs one function from `*pc` until it must call
//! another function, return, wait, or fail. Every executed instruction
//! costs one [`Cx::spend`]. `*pc` is the instruction being executed when an
//! error is returned (the VM reports it in the trace), stays at the `Call`
//! instruction for [`Exit::Call`] (the VM advances it when the callee
//! returns), and is already past the `Wait` for [`Exit::Wait`].

use std::sync::Arc;

use crate::error::ScriptErrorKind;
use crate::interp::Budget;
use crate::module::Reg;
use crate::native::{Host, NativeFn};
use crate::value::Value;

/// Why a compiled function stopped running.
#[derive(Debug)]
pub enum Exit {
    /// Call module function `func`; its result goes to `dst`.
    Call { func: u32, args: Vec<Value>, dst: Option<Reg> },
    Return(Value),
    /// Suspend for this many seconds of game time.
    Wait(f64),
}

/// What a compiled function can reach: the same things an instruction in the
/// interpreter can.
pub struct Cx<'a, 'w> {
    pub host: &'a mut Host<'w>,
    /// The instance's variables.
    pub vars: &'a mut [Value],
    pub budget: &'a mut Budget,
    /// Resolved imports, by import index.
    pub natives: &'a [Arc<NativeFn>],
    /// The module's constants, decoded.
    pub constants: &'a [Value],
    /// Integer overflow is an error rather than wrapping (see
    /// [`Vm::checked_arithmetic`](crate::Vm::checked_arithmetic)).
    pub checked: bool,
    /// Reusable argument buffer for native calls.
    pub scratch: &'a mut Vec<Value>,
}

impl Cx<'_, '_> {
    /// Charge one instruction.
    #[inline]
    pub fn spend(&mut self) -> Result<(), ScriptErrorKind> {
        if self.budget.remaining == 0 {
            return Err(ScriptErrorKind::BudgetExceeded);
        }
        self.budget.remaining -= 1;
        Ok(())
    }
}

/// The compiled functions of one module.
pub trait CompiledCode: Send + Sync + 'static {
    /// Run function `func` (an index into the module's functions) of the
    /// frame whose registers are `regs`. See the module docs.
    fn step(
        &self,
        func: u32,
        pc: &mut usize,
        regs: &mut [Value],
        cx: &mut Cx<'_, '_>,
    ) -> Result<Exit, ScriptErrorKind>;
}
