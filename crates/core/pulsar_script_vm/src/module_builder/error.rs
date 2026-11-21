//! [`BuildError`]: why a module or one of its functions could not be built.

use crate::error::VerifyError;
use crate::module::Reg;
use crate::types::Type;

/// Why a module (or one function of it) could not be built.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum BuildError {
    /// A function needs more registers than [`Reg`] can index.
    #[error("a function needs more than {} registers", Reg::MAX)]
    TooManyRegisters,
    /// An instruction named a register the function does not have.
    #[error("register {0} does not exist in this function")]
    NoSuchRegister(Reg),
    /// No native of this name is registered.
    #[error("no native function `{0}` is registered")]
    UnknownNative(String),
    /// A call passes the wrong number of arguments.
    #[error("`{what}` takes {expected} argument(s), got {found}")]
    ArgumentCount {
        what: String,
        expected: usize,
        found: usize,
    },
    /// A call passes an argument of the wrong type.
    #[error("argument {index} of `{what}` is `{expected}`, found `{found}`")]
    ArgumentType {
        what: String,
        index: usize,
        expected: Type,
        found: Type,
    },
    /// A store or load names a variable the module does not have.
    #[error("variable {0} does not exist")]
    NoSuchVariable(u32),
    /// A value of one type stored to a variable of another.
    #[error("variable `{name}` is `{expected}`, a `{found}` cannot be stored in it")]
    VariableType {
        name: String,
        expected: Type,
        found: Type,
    },
    /// Two variables, functions or events share a name.
    #[error("{kind} `{name}` is declared twice")]
    Duplicate { kind: &'static str, name: String },
    /// `break` or `continue` with no loop around it.
    #[error("`{0}` outside a loop")]
    NotInLoop(&'static str),
    /// A label was bound twice.
    #[error("label bound twice in `{function}`")]
    LabelBoundTwice { function: String },
    /// A jump refers to a label nothing was ever bound to.
    #[error("a jump in `{function}` refers to a label that was never bound")]
    UnboundLabel { function: String },
    /// A loop was entered and never left.
    #[error("a loop in `{function}` was never closed")]
    UnclosedLoop { function: String },
    /// A jump target is past the last instruction: end a function with an
    /// instruction (a `Return`, or [`FunctionBuilder::unreachable`]).
    #[error("`{function}`@{pc} jumps past the end of the function")]
    JumpPastEnd { function: String, pc: usize },
    /// A function was declared and never finished.
    #[error("function `{0}` was declared and never given a body")]
    FunctionNotDefined(String),
    /// The finished function belongs to another declaration.
    #[error("`{0}` was finished against a different declaration")]
    WrongFunction(String),
    /// The finished module failed verification: a bug in the front end.
    #[error("the compiled module does not verify: {0}")]
    Verify(VerifyError),
}
