//! The bytecode module format every scripting frontend compiles to.
//!
//! A [`Module`] is self-describing and serializable (serde; JSON today, a
//! compact binary encoding can be added without changing the model). It
//! contains:
//!
//! - **functions**, each with typed parameters, a typed register file and
//!   straight-line [`Instr`]uctions with explicit jumps;
//! - **imports**: the natives it calls, by stable qualified name and full
//!   signature, resolved against the native registry at link time;
//! - **variables**: per-instance state (a script bound to an entity keeps
//!   one set), with defaults;
//! - a constant pool;
//! - **events** the module declares (name and typed fields), registered as
//!   dynamic event descriptors on the engine event hub when the module is
//!   loaded;
//! - **subscriptions**: which function handles which event, and on which
//!   channel ([`SubscriptionScope`]). The engine subscribes every instance
//!   when it spawns and unsubscribes it when it despawns.
//!
//! Nothing here refers to a particular source language.
//!
//! # Events and handlers
//!
//! An event's fields map to script types one to one: `bool`, `int`
//! (`i64`), `float` (`f64`), `string`, and `entity` (event fields of type
//! `u64` are entity handles). A handler is a module function returning
//! `unit` whose parameters are a **prefix** of the event's fields, in
//! order: `fn on_hit(other: entity)` cannot handle `Hit(entity, other,
//! impulse)`, but `fn on_hit(entity: entity, other: entity)` and
//! `fn on_hit()` can. The verifier checks handlers against events the
//! module declares; the linker checks the rest against the engine's event
//! catalog.

use serde::{Deserialize, Serialize};

use crate::types::Type;

/// Bumped on any incompatible change to the format. Version 2 added
/// [`Module::events`] and [`Module::subscriptions`].
pub const FORMAT_VERSION: u32 = 2;

/// The oldest format version this VM still reads. Version 1 modules have
/// no events or subscriptions (both default to empty).
pub const MIN_FORMAT_VERSION: u32 = 1;

/// Register index within a function.
pub type Reg = u16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub format_version: u32,
    pub name: String,
    #[serde(default)]
    pub constants: Vec<Constant>,
    #[serde(default)]
    pub imports: Vec<Import>,
    #[serde(default)]
    pub variables: Vec<Variable>,
    #[serde(default)]
    pub functions: Vec<Function>,
    /// Events this module declares.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventDecl>,
    /// Event handlers, subscribed per instance.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<Subscription>,
}

impl Module {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            name: name.into(),
            constants: Vec::new(),
            imports: Vec::new(),
            variables: Vec::new(),
            functions: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        serde_json::from_str(json)
    }

    pub fn function(&self, name: &str) -> Option<(u32, &Function)> {
        self.functions
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == name)
            .map(|(i, f)| (i as u32, f))
    }
}

/// An event a module declares: registered on the engine event hub as a
/// dynamic descriptor with these fields when the module loads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventDecl {
    /// Unique engine-wide. Frontends qualify it (the Blueprint compiler
    /// uses `<Class>.<Event>`).
    pub name: String,
    #[serde(default)]
    pub fields: Vec<EventField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventField {
    pub name: String,
    /// `bool`, `int`, `float`, `string` or `entity`.
    pub ty: Type,
}

impl EventField {
    pub fn new(name: impl Into<String>, ty: Type) -> Self {
        Self { name: name.into(), ty }
    }
}

/// Which event a subscription is for.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventRef {
    /// By descriptor name (the usual form).
    Name(String),
    /// By stable descriptor id (e.g. a plugin event known only by id).
    Id(u64),
}

impl std::fmt::Display for EventRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Name(name) => f.write_str(name),
            Self::Id(id) => write!(f, "#{id:016x}"),
        }
    }
}

/// The channel a subscription listens on, relative to the instance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubscriptionScope {
    /// The entity channel of the entity the instance is bound to: events
    /// about or sent to this object only. Not subscribed for an unbound
    /// instance (a global script).
    #[serde(rename = "Self")]
    Self_,
    /// The global channel.
    #[default]
    Global,
    /// The class channel of the instance's own class: events sent to every
    /// instance of the class.
    Class,
}

/// "Run `handler` when `event` arrives on `scope`".
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Subscription {
    pub event: EventRef,
    /// Index into [`Module::functions`].
    pub handler: u32,
    #[serde(default)]
    pub scope: SubscriptionScope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Constant {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl Constant {
    pub fn ty(&self) -> Type {
        match self {
            Self::Bool(_) => Type::Bool,
            Self::Int(_) => Type::Int,
            Self::Float(_) => Type::Float,
            Self::Str(_) => Type::Str,
        }
    }
}

/// A native function the module calls.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Import {
    /// Stable qualified name, e.g. `math::sin` or `Health::damage`.
    pub name: String,
    pub sig: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Signature {
    pub params: Vec<Param>,
    pub ret: Type,
}

impl Signature {
    pub fn new(params: impl IntoIterator<Item = Param>, ret: Type) -> Self {
        Self { params: params.into_iter().collect(), ret }
    }
}

impl std::fmt::Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("(")?;
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            if p.inout {
                f.write_str("inout ")?;
            }
            write!(f, "{}", p.ty)?;
        }
        write!(f, ") -> {}", self.ret)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Param {
    pub ty: Type,
    /// The callee may modify the argument; the new value is written back to
    /// the caller's register after the call.
    #[serde(default)]
    pub inout: bool,
}

impl Param {
    pub fn new(ty: Type) -> Self {
        Self { ty, inout: false }
    }

    pub fn inout(ty: Type) -> Self {
        Self { ty, inout: true }
    }
}

/// Per-instance state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub ty: Type,
    /// Initial value; `None` means the type's default.
    #[serde(default)]
    pub default: Option<Constant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Function {
    pub name: String,
    /// Callable from outside the module (entry points and events).
    #[serde(default)]
    pub exported: bool,
    /// Parameters occupy registers `0..params.len()`.
    pub params: Vec<Type>,
    pub ret: Type,
    /// Type of every register, parameters first. Non-parameter registers
    /// start at their type's default on each call.
    pub registers: Vec<Type>,
    pub code: Vec<Instr>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    /// `int -> int`, `float -> float`.
    Neg,
    /// `bool -> bool`.
    Not,
    /// `int -> float`.
    IntToFloat,
    /// `float -> int`, truncating and saturating.
    FloatToInt,
    /// Any builtin value to its display string.
    ToStr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    /// `int`/`float` (wrapping for ints); `string` concatenation.
    Add,
    Sub,
    Mul,
    /// Division by zero is a runtime error for ints.
    Div,
    Rem,
    /// Any matching builtin types except objects.
    Eq,
    Ne,
    /// `int`, `float`, `string`.
    Lt,
    Le,
    Gt,
    Ge,
    /// `bool`.
    And,
    Or,
}

impl BinOp {
    pub fn is_comparison(self) -> bool {
        matches!(self, Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Instr {
    /// `dst = constants[index]`.
    Const { dst: Reg, index: u32 },
    Move { dst: Reg, src: Reg },
    Unary { op: UnOp, dst: Reg, src: Reg },
    Binary { op: BinOp, dst: Reg, a: Reg, b: Reg },
    Jump { target: u32 },
    Branch { cond: Reg, then: u32, otherwise: u32 },
    /// Call module function `func`; `dst` receives the result.
    Call { func: u32, args: Vec<Reg>, dst: Option<Reg> },
    /// Call `imports[import]`. `inout` arguments are written back.
    CallNative { import: u32, args: Vec<Reg>, dst: Option<Reg> },
    LoadVar { dst: Reg, var: u32 },
    StoreVar { var: u32, src: Reg },
    /// The entity this instance is bound to.
    SelfEntity { dst: Reg },
    /// Game time in seconds (`float`), as the host reports it.
    Now { dst: Reg },
    /// Suspend this call for `seconds` (`float`) of game time. The host
    /// resumes it later with [`Vm::resume`](crate::Vm::resume); execution
    /// continues at the next instruction with every frame and register as
    /// it was.
    Wait { seconds: Reg },
    Return { value: Option<Reg> },
}
