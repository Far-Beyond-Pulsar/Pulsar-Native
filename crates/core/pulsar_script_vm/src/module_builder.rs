//! Building a [`Module`]: what every text language's lowering needs.
//!
//! A language front end parses and type-checks its own syntax, and then has to
//! emit a bytecode module that the verifier accepts. The same chores come with
//! every language: typed registers, labels and jump patching, loop targets,
//! constants and imports de-duplicated, native calls checked against the
//! registry's signatures, per-instance variables with a stable identity, events
//! and subscriptions, and debug info. This module is those chores, with no
//! syntax in it, so a language is its front end plus calls into a
//! [`ModuleBuilder`].
//!
//! ```
//! use pulsar_script_vm::module_builder::ModuleBuilder;
//! use pulsar_script_vm::{BinOp, Constant, NativeRegistry, Type};
//!
//! let natives = NativeRegistry::new();
//! let mut module = ModuleBuilder::new("Sum", &natives);
//! let add = module.declare_function("add_one", vec![Type::Int], Type::Int, true)?;
//!
//! let mut f = module.begin_function(add);
//! let x = f.param_reg(0);
//! let (one, _) = module.load_constant(&mut f, Constant::Int(1))?;
//! let sum = f.new_reg(Type::Int)?;
//! f.emit(pulsar_script_vm::Instr::Binary { op: BinOp::Add, dst: sum, a: x, b: one });
//! f.ret(Some(sum));
//! module.finish_function(add, f)?;
//!
//! let module = module.finish()?; // verified
//! assert_eq!(module.functions[0].name, "add_one");
//! # Ok::<(), pulsar_script_vm::module_builder::BuildError>(())
//! ```
//!
//! # The shape
//!
//! - [`ModuleBuilder`] owns what is module-wide: the constant pool, imports,
//!   variables, the declared functions, events and subscriptions.
//!   [`ModuleBuilder::declare_function`] gives a function its index before any
//!   body is lowered, so calls can refer to functions declared later.
//! - [`FunctionBuilder`] owns what is function-wide: typed registers, the
//!   instruction list, [`Label`]s, the loop stack, the scopes of named locals
//!   and the current [`SourceLoc`] debug info is recorded against. It is a
//!   separate value, not borrowed from the module builder, so a lowering can
//!   hold both while it walks a syntax tree.
//! - Operations that touch both (loading a constant, calling a native, reading
//!   a variable) are methods of [`ModuleBuilder`] that take the
//!   [`FunctionBuilder`].
//!
//! The builder does not decide what is a type error in the source language: it
//! checks what makes a module wrong (a register of another type, a call with
//! another signature, a jump nowhere) and returns a [`BuildError`] naming it;
//! the front end reports its own diagnostic, with the position, for source
//! mistakes. [`ModuleBuilder::finish`] verifies the module, so a language
//! cannot ship a module the VM would refuse.

use std::collections::HashMap;
use std::sync::Arc;

use crate::error::VerifyError;
use crate::module::{
    BinOp, Constant, DebugInfo, EventDecl, EventRef, Function, Import, Instr, Module, Reg,
    SourceLoc, Subscription, SubscriptionScope, UnOp, Variable,
};
use crate::native::{NativeFn, NativeRegistry};
use crate::types::{Type, TypeRegistry};
use crate::verify::verify;

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

/// A function declared in a [`ModuleBuilder`]: its index in
/// [`Module::functions`], known before its body is lowered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FunctionHandle(u32);

impl FunctionHandle {
    /// The index calls (`Instr::Call { func }`) and subscriptions use.
    pub fn index(self) -> u32 {
        self.0
    }
}

/// A position in a function's code, referred to before it is known.
/// Bind it ([`FunctionBuilder::bind`]) where the code it names begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Label(u32);

struct Declared {
    name: String,
    exported: bool,
    params: Vec<Type>,
    ret: Type,
    body: Option<Body>,
}

struct Body {
    registers: Vec<Type>,
    code: Vec<Instr>,
    debug: Option<DebugInfo>,
}

/// Builds one [`Module`].
pub struct ModuleBuilder<'n> {
    name: String,
    natives: &'n NativeRegistry,
    constants: Vec<Constant>,
    imports: Vec<Import>,
    import_index: HashMap<String, u32>,
    variables: Vec<Variable>,
    variable_index: HashMap<String, u32>,
    functions: Vec<Declared>,
    function_index: HashMap<String, u32>,
    events: Vec<EventDecl>,
    subscriptions: Vec<Subscription>,
    class_version: u32,
}

impl<'n> ModuleBuilder<'n> {
    /// A builder for the module called `name`, calling natives from `natives`
    /// (the registry the module will link against).
    pub fn new(name: impl Into<String>, natives: &'n NativeRegistry) -> Self {
        Self {
            name: name.into(),
            natives,
            constants: Vec::new(),
            imports: Vec::new(),
            import_index: HashMap::new(),
            variables: Vec::new(),
            variable_index: HashMap::new(),
            functions: Vec::new(),
            function_index: HashMap::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            class_version: 0,
        }
    }

    /// The registry natives are resolved against: front ends use its query
    /// API (`functions()`, `methods_for(type)`) to find what a name means.
    pub fn natives(&self) -> &'n NativeRegistry {
        self.natives
    }

    /// The native registered under the qualified name `name` (`math::sin`).
    pub fn native(&self, name: &str) -> Option<&'n Arc<NativeFn>> {
        self.natives.get(name)
    }

    /// The script type an engine type name stands for: a component
    /// ([`Type::Component`]) or a value type ([`Type::Object`]).
    pub fn engine_type(&self, name: &str) -> Option<Type> {
        let types = TypeRegistry::global();
        if types.component(name).is_some() {
            Some(Type::Component(name.to_owned()))
        } else if types.value_types().any(|v| v == name) {
            Some(Type::Object(name.to_owned()))
        } else {
            None
        }
    }

    // ---- the class's identity ---------------------------------------------

    /// The class's schema version ([`Module::class_version`]): the front end
    /// raises it when the state layout changes in a way `migrate` handles.
    pub fn set_class_version(&mut self, version: u32) {
        self.class_version = version;
    }

    // ---- constants, imports ----------------------------------------------

    /// The pool index of `constant`, adding it when it is new.
    pub fn constant(&mut self, constant: Constant) -> u32 {
        if let Some(i) = self.constants.iter().position(|c| *c == constant) {
            return i as u32;
        }
        self.constants.push(constant);
        (self.constants.len() - 1) as u32
    }

    /// The import index of the native `name`, adding it (with the registry's
    /// signature) when it is new.
    pub fn import(&mut self, name: &str) -> Result<u32, BuildError> {
        if let Some(&i) = self.import_index.get(name) {
            return Ok(i);
        }
        let native = self
            .natives
            .get(name)
            .ok_or_else(|| BuildError::UnknownNative(name.to_owned()))?;
        self.imports.push(Import {
            name: name.to_owned(),
            sig: native.sig.clone(),
        });
        let index = (self.imports.len() - 1) as u32;
        self.import_index.insert(name.to_owned(), index);
        Ok(index)
    }

    // ---- variables --------------------------------------------------------

    /// Declare per-instance state: a variable of type `ty`, starting at
    /// `default` (the type's default when `None`). `id` is the field's stable
    /// identity, kept by the front end across renames, so state carries over a
    /// reload by id ([`crate::migrate`]); `None` matches by name. Returns the
    /// variable's index (`Instr::LoadVar { var }`).
    pub fn add_variable(
        &mut self,
        name: impl Into<String>,
        ty: Type,
        default: Option<Constant>,
        id: Option<String>,
    ) -> Result<u32, BuildError> {
        let name = name.into();
        if self.variable_index.contains_key(&name) {
            return Err(BuildError::Duplicate {
                kind: "variable",
                name,
            });
        }
        let index = self.variables.len() as u32;
        self.variable_index.insert(name.clone(), index);
        self.variables.push(Variable {
            name,
            ty,
            default,
            id,
        });
        Ok(index)
    }

    /// The index and type of the variable called `name`.
    pub fn variable(&self, name: &str) -> Option<(u32, &Type)> {
        let &index = self.variable_index.get(name)?;
        Some((index, &self.variables[index as usize].ty))
    }

    /// `dst = variables[var]`, into a new register of the variable's type.
    pub fn load_var(&self, f: &mut FunctionBuilder, var: u32) -> Result<Reg, BuildError> {
        let ty = self
            .variables
            .get(var as usize)
            .ok_or(BuildError::NoSuchVariable(var))?
            .ty
            .clone();
        let dst = f.new_reg(ty)?;
        f.emit(Instr::LoadVar { dst, var });
        Ok(dst)
    }

    /// `variables[var] = src`; `src` must be a register of the variable's type.
    pub fn store_var(&self, f: &mut FunctionBuilder, var: u32, src: Reg) -> Result<(), BuildError> {
        let variable = self
            .variables
            .get(var as usize)
            .ok_or(BuildError::NoSuchVariable(var))?;
        let found = f.reg_type(src)?.clone();
        if found != variable.ty {
            return Err(BuildError::VariableType {
                name: variable.name.clone(),
                expected: variable.ty.clone(),
                found,
            });
        }
        f.emit(Instr::StoreVar { var, src });
        Ok(())
    }

    // ---- events and subscriptions --------------------------------------------

    /// Declare an event the module raises; registered on the engine's event
    /// hub when the module loads. Its name is unique engine-wide, so front
    /// ends qualify it (`<Class>.<Event>`).
    pub fn declare_event(&mut self, event: EventDecl) -> Result<(), BuildError> {
        if self.events.iter().any(|e| e.name == event.name) {
            return Err(BuildError::Duplicate {
                kind: "event",
                name: event.name,
            });
        }
        self.events.push(event);
        Ok(())
    }

    /// Run `handler` when `event` arrives on `scope`. The handler returns
    /// `unit` and takes a prefix of the event's fields; the verifier and the
    /// linker check it.
    pub fn subscribe(
        &mut self,
        event: EventRef,
        handler: FunctionHandle,
        scope: SubscriptionScope,
    ) {
        self.subscriptions.push(Subscription {
            event,
            handler: handler.0,
            scope,
        });
    }

    // ---- functions ---------------------------------------------------------

    /// Declare a function and reserve its index, so calls can name it before
    /// its body exists. Parameters occupy registers `0..params.len()`.
    pub fn declare_function(
        &mut self,
        name: impl Into<String>,
        params: Vec<Type>,
        ret: Type,
        exported: bool,
    ) -> Result<FunctionHandle, BuildError> {
        let name = name.into();
        if self.function_index.contains_key(&name) {
            return Err(BuildError::Duplicate {
                kind: "function",
                name,
            });
        }
        let index = self.functions.len() as u32;
        self.function_index.insert(name.clone(), index);
        self.functions.push(Declared {
            name,
            exported,
            params,
            ret,
            body: None,
        });
        Ok(FunctionHandle(index))
    }

    /// The function declared as `name`.
    pub fn function(&self, name: &str) -> Option<FunctionHandle> {
        self.function_index.get(name).map(|&i| FunctionHandle(i))
    }

    /// A declared function's parameter and return types.
    pub fn signature(&self, function: FunctionHandle) -> (&[Type], &Type) {
        let declared = &self.functions[function.0 as usize];
        (&declared.params, &declared.ret)
    }

    /// Start the body of a declared function: its parameters are registers
    /// `0..n` ([`FunctionBuilder::param_reg`]).
    pub fn begin_function(&self, function: FunctionHandle) -> FunctionBuilder {
        let declared = &self.functions[function.0 as usize];
        FunctionBuilder::new(
            declared.name.clone(),
            declared.params.clone(),
            declared.ret.clone(),
        )
    }

    /// Finish a function's body. Every label must be bound, every loop left
    /// and every jump must land on an instruction.
    pub fn finish_function(
        &mut self,
        function: FunctionHandle,
        f: FunctionBuilder,
    ) -> Result<(), BuildError> {
        let declared = &mut self.functions[function.0 as usize];
        if declared.name != f.name {
            return Err(BuildError::WrongFunction(f.name));
        }
        let FunctionBuilder {
            name,
            registers,
            code,
            debug,
            labels,
            loops,
            error,
            ..
        } = f;
        if let Some(error) = error {
            return Err(error);
        }
        if labels.iter().any(|l| l.target.is_none() && !l.fixups.is_empty()) {
            return Err(BuildError::UnboundLabel { function: name });
        }
        if !loops.is_empty() {
            return Err(BuildError::UnclosedLoop { function: name });
        }
        for (pc, instr) in code.iter().enumerate() {
            let past = |target: u32| target as usize >= code.len();
            let bad = match instr {
                Instr::Jump { target } => past(*target),
                Instr::Branch {
                    then, otherwise, ..
                } => past(*then) || past(*otherwise),
                _ => false,
            };
            if bad {
                return Err(BuildError::JumpPastEnd { function: name, pc });
            }
        }
        declared.body = Some(Body {
            registers,
            code,
            debug: (!debug.ranges.is_empty()).then_some(debug),
        });
        Ok(())
    }

    // ---- operations over a function and the module -----------------------------

    /// `dst = constant` into a new register of the constant's type; returns
    /// the register and its type.
    pub fn load_constant(
        &mut self,
        f: &mut FunctionBuilder,
        constant: Constant,
    ) -> Result<(Reg, Type), BuildError> {
        let ty = constant.ty();
        let index = self.constant(constant);
        let dst = f.new_reg(ty.clone())?;
        f.emit(Instr::Const { dst, index });
        Ok((dst, ty))
    }

    /// Call a module function with `args` (registers), checked against its
    /// declared parameters. The result is in a new register of the function's
    /// return type, or `None` for a `unit` function.
    pub fn call(
        &self,
        f: &mut FunctionBuilder,
        function: FunctionHandle,
        args: &[Reg],
    ) -> Result<Option<Reg>, BuildError> {
        let declared = &self.functions[function.0 as usize];
        check_arguments(f, &declared.name, &declared.params, args)?;
        let dst = (declared.ret != Type::Unit)
            .then(|| f.new_reg(declared.ret.clone()))
            .transpose()?;
        f.emit(Instr::Call {
            func: function.0,
            args: args.to_vec(),
            dst,
        });
        Ok(dst)
    }

    /// Call the native `name` with `args` (registers), checked against the
    /// registry's signature, importing it on first use. The result is in a new
    /// register of the native's return type, or `None` for a `unit` native. A
    /// method of a value or component is a native whose first parameter is the
    /// receiver: pass the receiver first.
    pub fn call_native(
        &mut self,
        f: &mut FunctionBuilder,
        name: &str,
        args: &[Reg],
    ) -> Result<Option<Reg>, BuildError> {
        let native = self
            .natives
            .get(name)
            .ok_or_else(|| BuildError::UnknownNative(name.to_owned()))?;
        let params: Vec<Type> = native.sig.params.iter().map(|p| p.ty.clone()).collect();
        let ret = native.sig.ret.clone();
        check_arguments(f, name, &params, args)?;
        let import = self.import(name)?;
        let dst = (ret != Type::Unit).then(|| f.new_reg(ret)).transpose()?;
        f.emit(Instr::CallNative {
            import,
            args: args.to_vec(),
            dst,
        });
        Ok(dst)
    }

    /// The finished module, verified. Every declared function must have a body.
    pub fn finish(self) -> Result<Module, BuildError> {
        let mut functions = Vec::with_capacity(self.functions.len());
        for declared in self.functions {
            let Some(body) = declared.body else {
                return Err(BuildError::FunctionNotDefined(declared.name));
            };
            functions.push(Function {
                name: declared.name,
                exported: declared.exported,
                params: declared.params,
                ret: declared.ret,
                registers: body.registers,
                code: body.code,
                debug: body.debug,
            });
        }
        let mut module = Module::new(self.name);
        module.class_version = self.class_version;
        module.constants = self.constants;
        module.imports = self.imports;
        module.variables = self.variables;
        module.functions = functions;
        module.events = self.events;
        module.subscriptions = self.subscriptions;
        verify(&module).map_err(BuildError::Verify)?;
        Ok(module)
    }
}

fn check_arguments(
    f: &FunctionBuilder,
    what: &str,
    params: &[Type],
    args: &[Reg],
) -> Result<(), BuildError> {
    if args.len() != params.len() {
        return Err(BuildError::ArgumentCount {
            what: what.to_owned(),
            expected: params.len(),
            found: args.len(),
        });
    }
    for (index, (arg, param)) in args.iter().zip(params).enumerate() {
        let found = f.reg_type(*arg)?;
        if found != param {
            return Err(BuildError::ArgumentType {
                what: what.to_owned(),
                index,
                expected: param.clone(),
                found: found.clone(),
            });
        }
    }
    Ok(())
}

/// A named local of a function: its register, type and whether it may be
/// assigned (a `const` may not).
#[derive(Clone, Debug, PartialEq)]
pub struct Local {
    pub reg: Reg,
    pub ty: Type,
    pub mutable: bool,
}

#[derive(Clone, Copy)]
enum Slot {
    Jump,
    Then,
    Otherwise,
}

#[derive(Default)]
struct LabelState {
    target: Option<u32>,
    fixups: Vec<(usize, Slot)>,
}

struct LoopTargets {
    break_to: Label,
    continue_to: Label,
}

/// Builds one function: registers, instructions, labels, loops, scopes and
/// debug info. Made by [`ModuleBuilder::begin_function`], handed back to
/// [`ModuleBuilder::finish_function`].
pub struct FunctionBuilder {
    name: String,
    params: Vec<Type>,
    ret: Type,
    registers: Vec<Type>,
    code: Vec<Instr>,
    debug: DebugInfo,
    labels: Vec<LabelState>,
    loops: Vec<LoopTargets>,
    scopes: Vec<HashMap<String, Local>>,
    loc: Option<SourceLoc>,
    /// The first misuse of a label, reported when the function is finished.
    error: Option<BuildError>,
}

impl FunctionBuilder {
    fn new(name: String, params: Vec<Type>, ret: Type) -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::NativeRegistry;

    fn builder() -> (NativeRegistry, ()) {
        (NativeRegistry::with_engine_natives(), ())
    }

    fn one_function(
        module: &mut ModuleBuilder<'_>,
        name: &str,
        params: Vec<Type>,
        ret: Type,
    ) -> (FunctionHandle, FunctionBuilder) {
        let handle = module
            .declare_function(name, params, ret, true)
            .expect("declared");
        let f = module.begin_function(handle);
        (handle, f)
    }

    #[test]
    fn constants_and_imports_are_deduplicated() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let a = module.constant(Constant::Int(7));
        let b = module.constant(Constant::Str("x".into()));
        assert_eq!(module.constant(Constant::Int(7)), a);
        assert_eq!(module.constant(Constant::Str("x".into())), b);
        assert_ne!(a, b);

        let first = module.import("math::sqrt").expect("a registered native");
        assert_eq!(module.import("math::sqrt"), Ok(first));
        assert_eq!(
            module.import("math::no_such"),
            Err(BuildError::UnknownNative("math::no_such".into()))
        );
    }

    #[test]
    fn parameters_are_the_first_registers_and_registers_are_typed() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (_, mut f) = one_function(&mut module, "f", vec![Type::Int, Type::Str], Type::Unit);
        assert_eq!(f.param_reg(0), 0);
        assert_eq!(f.param_reg(1), 1);
        assert_eq!(f.reg_type(1), Ok(&Type::Str));
        let r = f.new_reg(Type::Float).expect("a register");
        assert_eq!(r, 2);
        assert_eq!(f.reg_type(r), Ok(&Type::Float));
        assert_eq!(f.reg_type(9), Err(BuildError::NoSuchRegister(9)));
    }

    #[test]
    fn registers_run_out_with_an_error() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (_, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
        let mut result = Ok(0);
        for _ in 0..=usize::from(Reg::MAX) {
            result = f.new_reg(Type::Int);
            if result.is_err() {
                break;
            }
        }
        assert_eq!(result, Err(BuildError::TooManyRegisters));
    }

    #[test]
    fn a_forward_jump_is_patched_when_its_label_is_bound() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (handle, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
        let end = f.new_label();
        let jump = f.jump(end);
        f.emit(Instr::Return { value: None });
        f.bind(end);
        f.ret(None);
        assert!(matches!(f.code[jump], Instr::Jump { target: 2 }));
        module.finish_function(handle, f).expect("finishes");
        module.finish().expect("verifies");
    }

    #[test]
    fn a_backward_jump_to_a_bound_label_needs_no_patching() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (_, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
        f.ret(None);
        let top = f.label_here();
        let at = f.jump(top);
        assert!(matches!(f.code[at], Instr::Jump { target: 1 }));
    }

    #[test]
    fn a_branch_patches_each_of_its_targets() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (handle, mut f) = one_function(&mut module, "f", vec![Type::Bool], Type::Unit);
        let (then, otherwise) = (f.new_label(), f.new_label());
        let cond = f.param_reg(0);
        let branch = f.branch(cond, then, otherwise);
        f.bind(then);
        f.ret(None);
        f.bind(otherwise);
        f.ret(None);
        assert!(matches!(
            f.code[branch],
            Instr::Branch {
                then: 1,
                otherwise: 2,
                ..
            }
        ));
        module.finish_function(handle, f).expect("finishes");
    }

    #[test]
    fn loops_send_break_and_continue_to_their_labels() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (handle, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
        assert_eq!(f.break_jump(), Err(BuildError::NotInLoop("break")));
        assert_eq!(f.continue_jump(), Err(BuildError::NotInLoop("continue")));
        let (end, top) = (f.new_label(), f.label_here());
        f.enter_loop(end, top);
        assert!(f.in_loop());
        let brk = f.break_jump().expect("in a loop");
        let cont = f.continue_jump().expect("in a loop");
        f.exit_loop();
        f.jump(top);
        f.bind(end);
        f.ret(None);
        assert!(matches!(f.code[brk], Instr::Jump { target: 3 }));
        assert!(matches!(f.code[cont], Instr::Jump { target: 0 }));
        module.finish_function(handle, f).expect("finishes");
    }

    #[test]
    fn an_unbound_label_an_open_loop_and_a_jump_past_the_end_are_refused() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);

        let (handle, mut f) = one_function(&mut module, "unbound", vec![], Type::Unit);
        let nowhere = f.new_label();
        f.jump(nowhere);
        f.ret(None);
        assert!(matches!(
            module.finish_function(handle, f),
            Err(BuildError::UnboundLabel { .. })
        ));

        let (handle, mut f) = one_function(&mut module, "open", vec![], Type::Unit);
        let (a, b) = (f.label_here(), f.label_here());
        f.enter_loop(a, b);
        f.ret(None);
        assert!(matches!(
            module.finish_function(handle, f),
            Err(BuildError::UnclosedLoop { .. })
        ));

        let (handle, mut f) = one_function(&mut module, "past", vec![], Type::Unit);
        let end = f.new_label();
        f.jump(end);
        f.bind(end);
        assert!(matches!(
            module.finish_function(handle, f),
            Err(BuildError::JumpPastEnd { pc: 0, .. })
        ));

        let (handle, mut f) = one_function(&mut module, "twice", vec![], Type::Unit);
        let label = f.label_here();
        f.bind(label);
        f.ret(None);
        assert!(matches!(
            module.finish_function(handle, f),
            Err(BuildError::LabelBoundTwice { .. })
        ));
    }

    #[test]
    fn unreachable_gives_a_function_that_always_returns_an_end() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (handle, mut f) = one_function(&mut module, "f", vec![Type::Bool], Type::Int);
        let (yes, no, end) = (f.new_label(), f.new_label(), f.new_label());
        let cond = f.param_reg(0);
        f.branch(cond, yes, no);
        f.bind(yes);
        let (one, _) = module
            .load_constant(&mut f, Constant::Int(1))
            .expect("loads");
        f.ret(Some(one));
        f.jump(end);
        f.bind(no);
        let (two, _) = module
            .load_constant(&mut f, Constant::Int(2))
            .expect("loads");
        f.ret(Some(two));
        f.bind(end);
        f.unreachable();
        module.finish_function(handle, f).expect("finishes");
        module.finish().expect("verifies");
    }

    #[test]
    fn native_calls_are_checked_against_the_registry_signature() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (_, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
        let (x, _) = module
            .load_constant(&mut f, Constant::Float(4.0))
            .expect("loads");
        let (s, _) = module
            .load_constant(&mut f, Constant::Str("s".into()))
            .expect("loads");
        let root = module
            .call_native(&mut f, "math::sqrt", &[x])
            .expect("a good call")
            .expect("a result");
        assert_eq!(f.reg_type(root), Ok(&Type::Float));
        assert!(matches!(
            module.call_native(&mut f, "math::sqrt", &[]),
            Err(BuildError::ArgumentCount { expected: 1, found: 0, .. })
        ));
        assert!(matches!(
            module.call_native(&mut f, "math::sqrt", &[s]),
            Err(BuildError::ArgumentType { index: 0, .. })
        ));
        assert!(matches!(
            module.call_native(&mut f, "math::nope", &[x]),
            Err(BuildError::UnknownNative(_))
        ));
        assert_eq!(module.imports.len(), 1);
    }

    #[test]
    fn module_calls_are_checked_and_can_name_a_later_function() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let caller = module
            .declare_function("caller", vec![], Type::Int, true)
            .expect("declared");
        let callee = module
            .declare_function("callee", vec![Type::Int], Type::Int, false)
            .expect("declared");
        assert_eq!(
            module.declare_function("callee", vec![], Type::Unit, false),
            Err(BuildError::Duplicate {
                kind: "function",
                name: "callee".into()
            })
        );
        assert_eq!(module.function("callee"), Some(callee));
        assert_eq!(module.signature(callee).0, &[Type::Int]);

        let mut f = module.begin_function(caller);
        let (n, _) = module
            .load_constant(&mut f, Constant::Int(2))
            .expect("loads");
        assert!(matches!(
            module.call(&mut f, callee, &[]),
            Err(BuildError::ArgumentCount { .. })
        ));
        let result = module
            .call(&mut f, callee, &[n])
            .expect("a good call")
            .expect("a result");
        f.ret(Some(result));
        module.finish_function(caller, f).expect("finishes");

        // `finish` refuses a function with no body.
        assert_eq!(
            module.finish().err(),
            Some(BuildError::FunctionNotDefined("callee".into()))
        );
    }

    #[test]
    fn variables_have_stable_ids_and_typed_access() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        module.set_class_version(3);
        let count = module
            .add_variable("count", Type::Int, Some(Constant::Int(5)), Some("id-1".into()))
            .expect("added");
        assert_eq!(
            module.add_variable("count", Type::Int, None, None),
            Err(BuildError::Duplicate {
                kind: "variable",
                name: "count".into()
            })
        );
        assert_eq!(module.variable("count"), Some((count, &Type::Int)));

        let get = module
            .declare_function("get", vec![], Type::Int, true)
            .expect("declared");
        let mut f = module.begin_function(get);
        let value = module.load_var(&mut f, count).expect("loads");
        let (text, _) = module
            .load_constant(&mut f, Constant::Str("x".into()))
            .expect("loads");
        assert!(matches!(
            module.store_var(&mut f, count, text),
            Err(BuildError::VariableType { .. })
        ));
        module.store_var(&mut f, count, value).expect("stores");
        f.ret(Some(value));
        module.finish_function(get, f).expect("finishes");

        let module = module.finish().expect("verifies");
        assert_eq!(module.class_version, 3);
        assert_eq!(module.variables[0].id.as_deref(), Some("id-1"));
        assert_eq!(module.variables[0].default, Some(Constant::Int(5)));
    }

    #[test]
    fn events_and_subscriptions_are_recorded() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let event = EventDecl {
            name: "T.Ping".into(),
            fields: vec![crate::module::EventField::new("n", Type::Int)],
        };
        module.declare_event(event.clone()).expect("declared");
        assert!(matches!(
            module.declare_event(event),
            Err(BuildError::Duplicate { kind: "event", .. })
        ));
        let on_ping = module
            .declare_function("on_ping", vec![Type::Int], Type::Unit, true)
            .expect("declared");
        let mut f = module.begin_function(on_ping);
        f.ret(None);
        module.finish_function(on_ping, f).expect("finishes");
        module.subscribe(
            EventRef::Name("T.Ping".into()),
            on_ping,
            SubscriptionScope::Self_,
        );
        let module = module.finish().expect("verifies");
        assert_eq!(module.events.len(), 1);
        assert_eq!(module.subscriptions[0].handler, on_ping.index());
    }

    #[test]
    fn debug_info_follows_the_current_location() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let handle = module
            .declare_function("f", vec![], Type::Unit, true)
            .expect("declared");
        let mut f2 = module.begin_function(handle);
        let a = SourceLoc {
            file: "a.x".into(),
            line: Some(1),
            column: Some(1),
            ..SourceLoc::default()
        };
        let b = SourceLoc {
            line: Some(2),
            ..a.clone()
        };
        let t = f2.new_reg(Type::Float).expect("a register");
        f2.set_loc(a.clone());
        f2.emit(Instr::Now { dst: t });
        f2.emit(Instr::Now { dst: t });
        f2.set_loc(b.clone());
        f2.ret(None);
        module.finish_function(handle, f2).expect("finishes");
        let module = module.finish().expect("verifies");
        let function = &module.functions[0];
        assert_eq!(function.location(0), Some(&a));
        assert_eq!(function.location(1), Some(&a));
        assert_eq!(function.location(2), Some(&b));
        assert_eq!(function.debug.as_ref().expect("debug").ranges.len(), 2);
    }

    #[test]
    fn a_function_without_locations_has_no_debug_info() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (handle, mut f) = one_function(&mut module, "f", vec![], Type::Unit);
        f.ret(None);
        module.finish_function(handle, f).expect("finishes");
        assert!(module.finish().expect("verifies").functions[0]
            .debug
            .is_none());
    }

    #[test]
    fn scopes_shadow_and_close() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (_, mut f) = one_function(&mut module, "f", vec![Type::Int], Type::Unit);
        f.bind_local("p", f.param_reg(0), Type::Int, true);
        assert!(f.declared_in_scope("p"));
        f.push_scope();
        assert!(!f.declared_in_scope("p"));
        let inner = f.declare_local("p", Type::Str, false).expect("declared");
        assert_eq!(f.lookup("p").map(|l| (l.reg, l.mutable)), Some((inner, false)));
        f.pop_scope();
        assert_eq!(f.lookup("p").map(|l| l.ty.clone()), Some(Type::Int));
        assert!(f.lookup("q").is_none());
    }

    #[test]
    fn operand_rules_are_the_verifiers() {
        assert_eq!(binary_result(BinOp::Add, &Type::Str), Some(Type::Str));
        assert_eq!(binary_result(BinOp::Sub, &Type::Str), None);
        assert_eq!(binary_result(BinOp::Lt, &Type::Float), Some(Type::Bool));
        assert_eq!(binary_result(BinOp::Eq, &Type::Entity), Some(Type::Bool));
        assert_eq!(binary_result(BinOp::And, &Type::Bool), Some(Type::Bool));
        assert_eq!(binary_result(BinOp::And, &Type::Int), None);
        assert_eq!(unary_result(UnOp::Neg, &Type::Float), Some(Type::Float));
        assert_eq!(unary_result(UnOp::Not, &Type::Int), None);
        assert_eq!(unary_result(UnOp::ToStr, &Type::Bool), Some(Type::Str));
    }

    #[test]
    fn typed_unary_and_binary_helpers_allocate_the_result() {
        let (natives, _) = builder();
        let mut module = ModuleBuilder::new("T", &natives);
        let (_, mut f) = one_function(&mut module, "f", vec![Type::Int, Type::Int, Type::Str], Type::Unit);
        let (a, b, s) = (f.param_reg(0), f.param_reg(1), f.param_reg(2));
        let sum = f.binary(BinOp::Add, a, b).expect("ok").expect("applies");
        assert_eq!(f.reg_type(sum), Ok(&Type::Int));
        let lt = f.binary(BinOp::Lt, a, b).expect("ok").expect("applies");
        assert_eq!(f.reg_type(lt), Ok(&Type::Bool));
        assert_eq!(f.binary(BinOp::Add, a, s), Ok(None));
        let wide = f.unary(UnOp::IntToFloat, a).expect("ok").expect("applies");
        assert_eq!(f.reg_type(wide), Ok(&Type::Float));
        assert_eq!(f.unary(UnOp::Not, a), Ok(None));
    }

    #[test]
    fn engine_types_resolve_by_name() {
        let (natives, _) = builder();
        let module = ModuleBuilder::new("T", &natives);
        assert_eq!(module.engine_type("No_Such_Type"), None);
    }
}
