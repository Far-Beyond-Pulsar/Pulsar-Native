//! Errors from verification, linking and execution.

use std::fmt;

use crate::module::{Signature, SourceLoc};

/// A module failed verification. Reported before anything runs.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.render())]
pub struct VerifyError {
    pub function: Option<String>,
    pub pc: Option<usize>,
    pub message: String,
}

impl VerifyError {
    pub(crate) fn module(message: impl Into<String>) -> Self {
        Self { function: None, pc: None, message: message.into() }
    }

    fn render(&self) -> String {
        match (&self.function, self.pc) {
            (Some(f), Some(pc)) => format!("{f}@{pc}: {}", self.message),
            (Some(f), None) => format!("{f}: {}", self.message),
            _ => self.message.clone(),
        }
    }
}

/// A verified module could not be linked against the native registry.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LinkError {
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error("no native function `{name}` is registered")]
    MissingNative { name: String },
    #[error("native `{name}` has signature {found}, the module expects {expected}")]
    SignatureMismatch { name: String, expected: Box<Signature>, found: Box<Signature> },
    #[error("unknown type `{name}`")]
    UnknownType { name: String },
    #[error("no event `{event}` is declared by the module or registered with the engine")]
    UnknownEvent { event: String },
    #[error("`{handler}` cannot handle event `{event}`: {message}")]
    HandlerMismatch { event: String, handler: String, message: String },
    #[error("`{name}`: {message}")]
    PolyNative { name: String, message: String },
    /// The module imports a native gated behind a capability the link
    /// policy does not allow (#869).
    #[error("native `{name}` needs capability `{capability}`, which this project does not allow")]
    CapabilityDenied { name: String, capability: String },
}

/// What went wrong while running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptErrorKind {
    /// A native returned an error (bad argument, missing component, ..).
    Native { name: String, message: String },
    DivideByZero,
    /// The step budget ran out (e.g. an infinite loop).
    BudgetExceeded,
    /// Call depth limit reached.
    StackOverflow,
    /// Integer overflow in `op` with checked arithmetic on
    /// ([`Vm::checked_arithmetic`](crate::Vm::checked_arithmetic)).
    Overflow { op: String },
    /// A call from the host passed the wrong arguments.
    BadEntryCall(String),
    /// The function waited (`Wait`) under [`Vm::call`](crate::Vm::call),
    /// which cannot suspend; use `Vm::start`.
    Suspended,
}

impl fmt::Display for ScriptErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Native { name, message } => write!(f, "{name}: {message}"),
            Self::DivideByZero => f.write_str("integer division by zero"),
            Self::BudgetExceeded => f.write_str("step budget exceeded"),
            Self::StackOverflow => f.write_str("call depth limit exceeded"),
            Self::Overflow { op } => write!(f, "integer overflow in {op} (checked arithmetic)"),
            Self::BadEntryCall(message) => write!(f, "bad call: {message}"),
            Self::Suspended => f.write_str("the function waited; run it with Vm::start"),
        }
    }
}

/// A runtime error with the script call stack where it happened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptError {
    pub kind: ScriptErrorKind,
    /// `(function, pc)`, innermost first.
    pub trace: Vec<(String, usize)>,
    /// The source location of each [`trace`](Self::trace) entry, from the
    /// functions' debug info (#854); `None` where a function has none.
    /// Same length as `trace` when filled by the VM.
    pub locations: Vec<Option<SourceLoc>>,
}

impl ScriptError {
    pub fn new(kind: ScriptErrorKind) -> Self {
        Self { kind, trace: Vec::new(), locations: Vec::new() }
    }

    /// For natives: a failure with a message (the VM fills in the name).
    pub fn native(message: impl Into<String>) -> Self {
        Self::new(ScriptErrorKind::Native { name: String::new(), message: message.into() })
    }

    /// The innermost frame, as `(function, pc)`.
    pub fn function(&self) -> Option<(&str, usize)> {
        self.trace.first().map(|(f, pc)| (f.as_str(), *pc))
    }

    /// The innermost source location the debug info knows: the node that
    /// failed, or the call site in the nearest caller that has one.
    pub fn location(&self) -> Option<&SourceLoc> {
        self.locations.iter().flatten().next()
    }
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        for (index, (function, pc)) in self.trace.iter().enumerate() {
            write!(f, "\n  at {function}@{pc}")?;
            if let Some(Some(location)) = self.locations.get(index) {
                write!(f, " ({location})")?;
            }
        }
        Ok(())
    }
}

impl std::error::Error for ScriptError {}
