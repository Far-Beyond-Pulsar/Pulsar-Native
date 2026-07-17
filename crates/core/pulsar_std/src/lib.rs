//! # Pulsar Standard Library
//!
//! Built-in blueprint nodes for the Pulsar visual programming system.
//!
//! All nodes are defined as Rust functions with the `#[blueprint]` attribute macro.
//!

use std::sync::atomic::{AtomicBool, Ordering};

/// Runtime flag for allowing unsafe shell process execution.
/// Set via [`set_unsafe_process_allowed`]; checked by shell blueprint nodes.
static UNSAFE_PROCESS_ALLOWED: AtomicBool = AtomicBool::new(false);

/// Enable or disable shell process blueprint nodes at runtime.
pub fn set_unsafe_process_allowed(allowed: bool) {
    UNSAFE_PROCESS_ALLOWED.store(allowed, Ordering::Relaxed);
}

/// Returns `true` if shell process blueprint nodes are currently allowed.
pub fn unsafe_process_allowed() -> bool {
    UNSAFE_PROCESS_ALLOWED.load(Ordering::Relaxed)
}

// Registry infrastructure
pub mod registry;
pub mod type_constructors;
pub use registry::*;

// ── Runtime type descriptor ───────────────────────────────────────────────────
//
// `TypeSlot` carries the size and alignment of a generic type parameter `T`
// resolved at graph-compile time.  Every dispatch function receives a pointer
// to an array of these as its third argument; concrete (non-generic) functions
// simply ignore it.

/// ABI-stable runtime descriptor for a single type parameter.
///
/// Stored inside the byte arena (via `Instruction::InitTypeSlot`) and passed
/// as the third argument to every `__bp_dispatch_*` symbol.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeSlot {
    /// `std::mem::size_of::<T>()` resolved at graph-compile time.
    pub size: usize,
    /// `std::mem::align_of::<T>()` resolved at graph-compile time.
    pub align: usize,
}

/// Sugar for returning multiple values from a blueprint node with named output pins.
///
/// Expands to `return (expr1, expr2, ...)` and generates `#[output]` metadata
/// (picked up by the enclosing `#[blueprint]` macro).
///
/// # Syntax
///
/// ```ignore
/// bp_return!(label1: expr1, label2: expr2, ...)
/// ```
///
/// # Example
///
/// ```ignore
/// #[blueprint(type: NodeTypes::pure, category: "Math")]
/// fn div_mod(a: i64, b: i64) -> (i64, i64) {
///     bp_return!(quotient: a / b, remainder: a % b);
/// }
/// ```
///
/// This is equivalent to:
///
/// ```ignore
/// #[output(name = "quotient")]
/// #[output(name = "remainder")]
/// #[blueprint(type: NodeTypes::pure, category: "Math")]
/// fn div_mod(a: i64, b: i64) -> (i64, i64) {
///     (a / b, a % b)
/// }
/// ```
#[macro_export]
macro_rules! bp_return {
    ($($label:ident : $expr:expr),+ $(,)?) => {
        return ($($expr),+)
    };
}

// Re-export macros
pub use pulsar_macros::{blueprint, blueprint_type, bp_import, conversion, exec_output, output};

// =============================================================================
// Node Type Enum (for blueprint attribute)
// =============================================================================

/// Node type for the `#[blueprint(type: ...)]` attribute
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeTypes {
    /// Pure function: no side effects, no exec pins, only data flow
    pure,

    /// Function with side effects: one exec in, one exec out
    fn_,

    /// Control flow: one exec in, multiple exec outs via exec_output!()
    control_flow,

    /// Event: defines an entry point function (e.g., main, begin_play)
    /// Events define the outer function signature and have exec_output!("Body")
    event,
}

// =============================================================================
// Modular Node Organization
// =============================================================================

pub mod engine;
pub use engine::*;

// experimental contains Lua scripting and other native-only nodes
#[cfg(feature = "native")]
pub mod experimental;
#[cfg(feature = "native")]
pub use experimental::*;

// This is how engine detects Your nodes, enter your node folder name (it must have an mod.rs)
// pub mod foldername;
// pub use foldername::*;
// ^this tells pulsar to use it in its nodes

//
