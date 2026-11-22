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

mod error;
mod function;

#[cfg(test)]
mod tests;

pub use error::BuildError;
pub use function::{binary_result, unary_result, FunctionBuilder, Label, Local};

use std::collections::HashMap;
use std::sync::Arc;

use crate::module::{
    Constant, DebugInfo, EventDecl, EventRef, Function, Import, Instr, Module, Reg, Subscription,
    SubscriptionScope, Variable,
};
use crate::native::{NativeFn, NativeRegistry};
use crate::types::{Type, TypeRegistry};
use crate::verify::verify;

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
        if labels
            .iter()
            .any(|l| l.target.is_none() && !l.fixups.is_empty())
        {
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
