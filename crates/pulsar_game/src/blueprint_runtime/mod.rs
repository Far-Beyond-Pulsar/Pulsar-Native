//! Blueprint runtime system for game execution.
//!
//! This module provides bytecode-based blueprint execution for development workflow,
//! with support for hot-reload, in-editor playtesting, and visual debugging.

pub mod byte_arena;
pub mod bytecode_compiler;
pub mod compiled_bytecode;
pub mod dispatcher;
pub mod executor;
pub mod instance;

pub use byte_arena::ByteArena;
pub use bytecode_compiler::BytecodeCompiler;
pub use compiled_bytecode::{CompiledBytecode, VariableDescriptor};
pub use dispatcher::{BlueprintDispatcher, BlueprintEvent, ExecutionMode};
pub use executor::BlueprintExecutor;
pub use instance::{BlueprintExecutionMode, BlueprintInstance};
