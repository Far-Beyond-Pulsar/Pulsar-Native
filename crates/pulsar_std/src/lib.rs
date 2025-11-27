#![allow(warnings)]

//! # Pulsar Standard Library
//!
//! Built-in blueprint nodes for the Pulsar visual programming system.
//!
//! All nodes are defined as Rust functions with the `#[blueprint]` attribute macro.
//!

// Registry infrastructure
pub mod registry;
pub mod type_constructors;
pub use registry::*;

// Re-export macros
pub use pulsar_macros::{blueprint, bp_import, exec_output, blueprint_type};

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
    pub mod experimental;
    pub use experimental::*;
    
    // This is how engine detects Your nodes, enter your node folder name (it must have an mod.rs) 
    // pub mod foldername;
    // pub use foldername::*;
    // ^this tells pulsar to use it in its nodes

//