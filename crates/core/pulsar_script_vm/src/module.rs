//! The bytecode module format every scripting frontend compiles to.
//!
//! A [`Module`] is self-describing and serializable in two encodings
//! (#852): JSON ([`Module::to_json`], what the editor writes, easy to
//! read and diff) and a compact binary one ([`Module::to_binary`], what
//! packaged games ship). [`Module::decode`] reads either, telling them
//! apart by the binary header ([`BINARY_MAGIC`] plus the format version).
//! It contains:
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

use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};

use crate::types::Type;

/// Bumped on any incompatible change to the format. Version 2 added
/// [`Module::events`] and [`Module::subscriptions`].
pub const FORMAT_VERSION: u32 = 2;

/// The oldest format version this VM still reads. Version 1 modules have
/// no events or subscriptions (both default to empty).
pub const MIN_FORMAT_VERSION: u32 = 1;

/// First bytes of a binary module ([`Module::to_binary`]). JSON modules
/// start with `{` or whitespace, so the two never collide.
pub const BINARY_MAGIC: [u8; 4] = *b"PSVM";

/// Length of the binary header: [`BINARY_MAGIC`], then the format version
/// as a little-endian `u32`.
pub const BINARY_HEADER_LEN: usize = 8;

/// Why bytes could not be read as a module.
#[derive(Debug, thiserror::Error)]
pub enum ModuleDecodeError {
    #[error("invalid JSON module: {0}")]
    Json(#[from] serde_json::Error),
    #[error("binary module header is truncated")]
    Truncated,
    #[error("binary module has format version {found}; this engine reads version {expected}")]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("corrupt binary module: {0}")]
    Corrupt(String),
}

fn bincode_config() -> impl bincode::config::Config {
    bincode::config::standard()
}

/// Register index within a function.
pub type Reg = u16;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
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

    /// The compact binary encoding: [`BINARY_MAGIC`], [`FORMAT_VERSION`]
    /// (little-endian `u32`), then the module (bincode, standard config).
    /// Debug info is kept; strip [`Function::debug`] first to drop it.
    pub fn to_binary(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&BINARY_MAGIC);
        out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        let body = bincode::encode_to_vec(self, bincode_config())
            .expect("encoding a module into memory cannot fail");
        out.extend_from_slice(&body);
        out
    }

    /// Read a binary module ([`to_binary`](Self::to_binary)). Only the
    /// current [`FORMAT_VERSION`] is accepted: binary modules are build
    /// outputs, rebuilt with the engine, never hand-kept.
    pub fn from_binary(bytes: &[u8]) -> Result<Self, ModuleDecodeError> {
        if bytes.len() < BINARY_HEADER_LEN || bytes[..4] != BINARY_MAGIC {
            return Err(ModuleDecodeError::Truncated);
        }
        let found = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        if found != FORMAT_VERSION {
            return Err(ModuleDecodeError::UnsupportedVersion { found, expected: FORMAT_VERSION });
        }
        let (module, read): (Self, usize) =
            bincode::decode_from_slice(&bytes[BINARY_HEADER_LEN..], bincode_config())
                .map_err(|error| ModuleDecodeError::Corrupt(error.to_string()))?;
        if BINARY_HEADER_LEN + read != bytes.len() {
            return Err(ModuleDecodeError::Corrupt(format!(
                "{} trailing bytes",
                bytes.len() - BINARY_HEADER_LEN - read
            )));
        }
        Ok(module)
    }

    /// Whether `bytes` start like a binary module.
    pub fn is_binary(bytes: &[u8]) -> bool {
        bytes.starts_with(&BINARY_MAGIC)
    }

    /// Read a module in either encoding, detected from the first bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self, ModuleDecodeError> {
        if Self::is_binary(bytes) {
            Self::from_binary(bytes)
        } else {
            Ok(serde_json::from_slice(bytes)?)
        }
    }

    pub fn function(&self, name: &str) -> Option<(u32, &Function)> {
        self.functions
            .iter()
            .enumerate()
            .find(|(_, f)| f.name == name)
            .map(|(i, f)| (i as u32, f))
    }

    /// Where in this module a link error comes from (#868): the function
    /// and, with debug info, the source location of the first instruction
    /// or subscription involved. `None` for errors about the module as a
    /// whole.
    pub fn locate_link_error(&self, error: &crate::error::LinkError) -> Option<ErrorSite> {
        use crate::error::LinkError;
        let at_function = |name: &str, pc: Option<usize>| {
            let (_, function) = self.function(name)?;
            let location = match pc {
                Some(pc) => function.location(pc),
                None => function.first_location(),
            };
            Some(ErrorSite { function: function.name.clone(), pc, location: location.cloned() })
        };
        match error {
            LinkError::Verify(verify) => at_function(verify.function.as_deref()?, verify.pc),
            LinkError::MissingNative { name }
            | LinkError::SignatureMismatch { name, .. }
            | LinkError::PolyNative { name, .. }
            | LinkError::CapabilityDenied { name, .. } => {
                let import = self.imports.iter().position(|i| &i.name == name)? as u32;
                self.functions.iter().find_map(|function| {
                    let pc = function.code.iter().position(
                        |instr| matches!(instr, Instr::CallNative { import: i, .. } if *i == import),
                    )?;
                    Some(ErrorSite {
                        function: function.name.clone(),
                        pc: Some(pc),
                        location: function.location(pc).cloned(),
                    })
                })
            }
            LinkError::HandlerMismatch { handler, .. } => at_function(handler, None),
            LinkError::UnknownEvent { event } => {
                let subscription = self.subscriptions.iter().find(|s| s.event.to_string() == *event)?;
                let function = self.functions.get(subscription.handler as usize)?;
                at_function(&function.name, None)
            }
            LinkError::UnknownType { .. } => None,
        }
    }
}

/// Where in a module an error is: a function, maybe an instruction, and
/// the source location debug info gives for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorSite {
    pub function: String,
    pub pc: Option<usize>,
    pub location: Option<SourceLoc>,
}

impl std::fmt::Display for ErrorSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.function)?;
        if let Some(pc) = self.pc {
            write!(f, "@{pc}")?;
        }
        if let Some(location) = &self.location {
            write!(f, " ({location})")?;
        }
        Ok(())
    }
}

/// An event a module declares: registered on the engine event hub as a
/// dynamic descriptor with these fields when the module loads.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct EventDecl {
    /// Unique engine-wide. Frontends qualify it (the Blueprint compiler
    /// uses `<Class>.<Event>`).
    pub name: String,
    #[serde(default)]
    pub fields: Vec<EventField>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct Subscription {
    pub event: EventRef,
    /// Index into [`Module::functions`].
    pub handler: u32,
    #[serde(default)]
    pub scope: SubscriptionScope,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct Import {
    /// Stable qualified name, e.g. `math::sin` or `Health::damage`.
    pub name: String,
    pub sig: Signature,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub struct Variable {
    pub name: String,
    pub ty: Type,
    /// Initial value; `None` means the type's default.
    #[serde(default)]
    pub default: Option<Constant>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
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
    /// Optional debug info: which source location produced each range of
    /// instructions (#854). Runtime errors and link errors resolve their
    /// pc through it. Absent in modules built without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<DebugInfo>,
}

impl Function {
    /// The source location of instruction `pc`, if the function has debug
    /// info covering it.
    pub fn location(&self, pc: usize) -> Option<&SourceLoc> {
        self.debug.as_ref()?.location(pc)
    }

    /// The first source location recorded for the function (its entry
    /// node, for a Blueprint event).
    pub fn first_location(&self) -> Option<&SourceLoc> {
        self.debug.as_ref()?.ranges.first().map(|r| &r.loc)
    }
}

/// Where some instructions came from. Opaque to the VM: each frontend
/// fills what it has. The Blueprint compiler sets `node` to the graph node
/// id and `file` to the graph file relative to the class directory; a
/// text language would use `file` plus `line` / `column`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
pub struct SourceLoc {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub file: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

impl SourceLoc {
    /// A location naming graph node `node` in `file`.
    pub fn node(file: impl Into<String>, node: impl Into<String>) -> Self {
        Self { file: file.into(), node: node.into(), line: None, column: None }
    }
}

impl std::fmt::Display for SourceLoc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if !self.node.is_empty() {
            parts.push(format!("node {}", self.node));
        }
        if !self.file.is_empty() {
            let mut at = self.file.clone();
            if let Some(line) = self.line {
                at.push_str(&format!(":{line}"));
                if let Some(column) = self.column {
                    at.push_str(&format!(":{column}"));
                }
            }
            parts.push(format!("in {at}"));
        }
        f.write_str(&parts.join(" "))
    }
}

/// A function's pc → source table: sorted, non-overlapping ranges. Pcs no
/// range covers have no location.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct DebugInfo {
    #[serde(default)]
    pub ranges: Vec<DebugRange>,
}

/// Instructions `start..end` came from `loc`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub struct DebugRange {
    pub start: u32,
    /// Exclusive.
    pub end: u32,
    pub loc: SourceLoc,
}

impl DebugInfo {
    /// The location of `pc`.
    pub fn location(&self, pc: usize) -> Option<&SourceLoc> {
        let pc = u32::try_from(pc).ok()?;
        let index = self.ranges.partition_point(|r| r.end <= pc);
        self.ranges.get(index).filter(|r| r.start <= pc).map(|r| &r.loc)
    }

    /// Record that instruction `pc` came from `loc`. Pcs must be recorded
    /// in increasing order; a pc continuing the last range with the same
    /// location extends it.
    pub fn record(&mut self, pc: u32, loc: &SourceLoc) {
        if let Some(last) = self.ranges.last_mut() {
            if last.end == pc && last.loc == *loc {
                last.end = pc + 1;
                return;
            }
            if pc < last.end {
                return;
            }
        }
        self.ranges.push(DebugRange { start: pc, end: pc + 1, loc: loc.clone() });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum BinOp {
    /// `int`/`float`; `string` concatenation. Integer `Add`, `Sub`, `Mul`,
    /// `Div` and `Rem` wrap, unless the VM runs with checked arithmetic
    /// ([`Vm::checked_arithmetic`](crate::Vm::checked_arithmetic)), where
    /// overflow is a runtime error.
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Encode, Decode)]
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
