//! A tiny assembler for writing test modules by hand.

#![allow(dead_code)]

use std::sync::Arc;

use pulsar_scenedb::{Entity, World};
use pulsar_script_vm::{
    Budget, Constant, Function, Host, Import, Instance, Instr, Module, NativeRegistry, Param,
    Program, ScriptError, Signature, Type, Value, Variable, Vm,
};

pub struct Asm {
    pub module: Module,
}

impl Asm {
    pub fn new() -> Self {
        Self { module: Module::new("test") }
    }

    pub fn constant(&mut self, constant: Constant) -> u32 {
        if let Some(i) = self.module.constants.iter().position(|c| *c == constant) {
            return i as u32;
        }
        self.module.constants.push(constant);
        (self.module.constants.len() - 1) as u32
    }

    pub fn import(&mut self, name: &str, params: Vec<Param>, ret: Type) -> u32 {
        self.module.imports.push(Import { name: name.into(), sig: Signature::new(params, ret) });
        (self.module.imports.len() - 1) as u32
    }

    pub fn var(&mut self, name: &str, ty: Type, default: Option<Constant>) -> u32 {
        self.module.variables.push(Variable { name: name.into(), ty, default });
        (self.module.variables.len() - 1) as u32
    }

    /// Add an exported function; `registers` lists the non-parameter
    /// registers after the parameters.
    pub fn function(
        &mut self,
        name: &str,
        params: Vec<Type>,
        ret: Type,
        registers: Vec<Type>,
        code: Vec<Instr>,
    ) -> u32 {
        let mut all = params.clone();
        all.extend(registers);
        self.module.functions.push(Function {
            name: name.into(),
            exported: true,
            params,
            ret,
            registers: all,
            code,
            debug: None,
        });
        (self.module.functions.len() - 1) as u32
    }

    pub fn link(&self, registry: &NativeRegistry) -> Program {
        Program::link(Arc::new(self.module.clone()), registry).expect("link")
    }
}

/// Everything needed to run programs against a world.
pub struct Harness {
    pub world: World,
    pub entity: Entity,
    pub vm: Vm,
}

impl Harness {
    pub fn new() -> Self {
        let mut world = World::new();
        let entity = world.spawn();
        Self { world, entity, vm: Vm::new() }
    }

    pub fn call(
        &mut self,
        program: &Program,
        instance: &mut Instance,
        name: &str,
        args: &[Value],
    ) -> Result<Value, ScriptError> {
        let func = program.entry(name).expect("entry point");
        let mut host = Host::new(&mut self.world, self.entity);
        self.vm.call(program, instance, func, args, &mut host, &mut Budget::new(1_000_000))
    }

    /// Run a function once on a fresh instance.
    pub fn run(&mut self, program: &Program, name: &str, args: &[Value]) -> Result<Value, ScriptError> {
        let mut instance = program.instantiate();
        self.call(program, &mut instance, name, args)
    }
}

pub fn int(i: i64) -> Value {
    Value::Int(i)
}

pub fn float(f: f64) -> Value {
    Value::Float(f)
}
