//! Linking a verified [`Module`] against a [`NativeRegistry`] into a
//! runnable [`Program`].

use std::sync::Arc;

use crate::error::LinkError;
use crate::module::{Constant, Module};
use crate::native::{NativeFn, NativeRegistry};
use crate::types::{Type, TypeRegistry};
use crate::value::Value;
use crate::verify::verify;

/// A module bound to the natives it imports, ready to run. Holds its
/// natives (and so any library they come from) alive.
pub struct Program {
    module: Arc<Module>,
    pub(crate) natives: Vec<Arc<NativeFn>>,
    pub(crate) constants: Vec<Value>,
    /// Initial register values per function (defaults for every register).
    pub(crate) registers: Vec<Vec<Value>>,
    variables: Vec<Value>,
    generation: u64,
}

/// Per-instance state of a program: its variables. Read and written
/// through [`Program::var`] / [`Program::set_var`], which keep every value
/// of its declared type.
/// Tied to its module, so it survives relinking (e.g. after a native
/// library reload) but cannot be used with another module's program.
#[derive(Clone, Debug)]
pub struct Instance {
    pub(crate) module: Arc<Module>,
    pub(crate) vars: Vec<Value>,
}

/// Index of a function in a program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FuncId(pub u32);

impl Program {
    /// Verify `module`, check that every type it names exists, and bind its
    /// imports to `registry`'s natives (names and full signatures must
    /// match).
    pub fn link(module: Arc<Module>, registry: &NativeRegistry) -> Result<Self, LinkError> {
        verify(&module)?;
        let types = TypeRegistry::global();
        let default = |ty: &Type| {
            types.default_value(ty).ok_or_else(|| LinkError::UnknownType { name: ty.to_string() })
        };

        let mut natives = Vec::with_capacity(module.imports.len());
        for import in &module.imports {
            for param in &import.sig.params {
                default(&param.ty)?;
            }
            default(&import.sig.ret)?;
            let native = registry
                .get(&import.name)
                .ok_or_else(|| LinkError::MissingNative { name: import.name.clone() })?;
            if native.sig != import.sig {
                return Err(LinkError::SignatureMismatch {
                    name: import.name.clone(),
                    expected: Box::new(import.sig.clone()),
                    found: Box::new(native.sig.clone()),
                });
            }
            natives.push(Arc::clone(native));
        }

        let registers = module
            .functions
            .iter()
            .map(|f| {
                default(&f.ret)?;
                f.registers.iter().map(default).collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        let variables = module
            .variables
            .iter()
            .map(|v| match &v.default {
                Some(constant) => Ok(constant_value(constant)),
                None => default(&v.ty),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let constants = module.constants.iter().map(constant_value).collect();

        Ok(Self { module, natives, constants, registers, variables, generation: registry.generation() })
    }

    pub fn module(&self) -> &Arc<Module> {
        &self.module
    }

    /// The registry generation this was linked against. If the registry's
    /// generation has moved on (a library was reloaded), relink.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A fresh instance with every variable at its default.
    pub fn instantiate(&self) -> Instance {
        Instance { module: Arc::clone(&self.module), vars: self.variables.clone() }
    }

    /// An exported function by name.
    pub fn entry(&self, name: &str) -> Option<FuncId> {
        self.module.function(name).filter(|(_, f)| f.exported).map(|(i, _)| FuncId(i))
    }

    /// Index of variable `name`.
    pub fn variable(&self, name: &str) -> Option<usize> {
        self.module.variables.iter().position(|v| v.name == name)
    }

    /// Variable `index` of `instance`.
    pub fn var<'i>(&self, instance: &'i Instance, index: usize) -> Option<&'i Value> {
        instance.vars.get(index)
    }

    /// Set variable `index` of `instance`, if `value` has its type.
    pub fn set_var(&self, instance: &mut Instance, index: usize, value: Value) -> Result<(), String> {
        let var = self.module.variables.get(index).ok_or_else(|| format!("no variable {index}"))?;
        let fits = match (&value, &var.ty) {
            (Value::Object(obj), Type::Object(name)) => obj.type_name() == name,
            (Value::Component(c), Type::Component(name)) => TypeRegistry::global()
                .component(name)
                .is_some_and(|b| b.component_id() == c.component),
            (value, ty) => value.fits(ty),
        };
        if !fits {
            return Err(format!("`{}` is {}, got {}", var.name, var.ty, value.kind()));
        }
        instance.vars[index] = value;
        Ok(())
    }
}

fn constant_value(constant: &Constant) -> Value {
    match constant {
        Constant::Bool(b) => Value::Bool(*b),
        Constant::Int(i) => Value::Int(*i),
        Constant::Float(f) => Value::Float(*f),
        Constant::Str(s) => Value::Str(s.as_str().into()),
    }
}
