//! # Pure Node Code Generation
//!
//! Strategy for generating code from pure nodes (no side effects, no exec pins).
//!
//! Pure nodes are mathematical or data transformation functions that:
//! - Take inputs and produce outputs
//! - Have no side effects
//! - Can be safely inlined as expressions
//!
//! ## Generation Strategy
//!
//! Pure nodes are generated as **inline expressions** where they're used.
//! This eliminates function call overhead and enables compiler optimizations.
//!
//! Pure nodes are handled during the execution chain generation phase. When
//! encountered in `generate_exec_chain`, they are skipped since they have no
//! execution flow. Instead, their values are resolved by the `DataResolver`
//! during the data flow analysis phase and are inlined as expressions when
//! generating code for function and control flow nodes.
//!
//! ## Example
//!
//! Graph: `multiply(add(2, 3), 4)`
//!
//! Generated: `let result = multiply(add(2, 3), 4);`
//!
//! Both `add` and `multiply` are inlined as expressions.
//!
//! ## Implementation Notes
//!
//! Pure node handling is distributed across the codebase:
//! - **Data Resolution**: `DataResolver` in the analysis phase generates
//!   inline expressions for pure nodes recursively
//! - **Execution Chain**: Pure nodes are skipped in `CodeGenerator::generate_exec_chain`
//!   since they have no execution flow
//! - **Variable Getters**: Getter nodes (e.g., `get_health`) are treated as
//!   pure nodes and handled similarly
