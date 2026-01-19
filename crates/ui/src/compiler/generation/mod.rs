//! # Code Generation Phase
//!
//! Transforms analyzed node graphs into executable Rust source code.
//!
//! This phase takes the results of analysis (data flow, execution flow) and generates
//! compilable Rust code that faithfully implements the Blueprint's behavior.
//!
//! ## Architecture
//!
//! The code generator uses different strategies for different node types:
//!
//! ### Pure Node Strategy (`pure_nodes.rs`)
//! - Generated as **inline expressions**
//! - Recursively inlined where used
//! - No temporary variables (unless result is used multiple times)
//! - Example: `add(multiply(a, b), c)` inlines both operations
//!
//! ### Function Node Strategy (`function_nodes.rs`)
//! - Generated as **sequential function calls**
//! - Each node becomes a statement in the execution chain
//! - Follows exec connections for control flow
//! - Example: `print_string("Hi"); do_something();`
//!
//! ### Control Flow Node Strategy (`control_flow_nodes.rs`)
//! - Must be **inlined** (cannot be called as functions)
//! - Function body is parsed and transformed
//! - `exec_output!("Label")` replaced with connected node code
//! - Parameters substituted with actual values
//! - Example: `if condition { /* True branch */ } else { /* False branch */ }`
//!
//! ## Code Generator Pipeline
//!
//! ```text
//! ┌──────────────────┐
//! │  Event Nodes     │ (Entry points: main, begin_play, etc.)
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │  Generate Pure   │ (Evaluate data dependencies first)
//! │  Evaluations     │ (In topological order)
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │  Generate Exec   │ (Follow execution chain from event)
//! │  Chain           │ (Inline control flow, call functions)
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │  Add Imports     │ (Collect required use statements)
//! └────────┬─────────┘
//!          │
//!          ▼
//! ┌──────────────────┐
//! │  Format & Return │ (Optional: run rustfmt)
//! └──────────────────┘
//! ```
//!
//! ## Example Generated Code
//!
//! ### Input Graph
//! ```text
//! main_event → branch(add(2, 3) > 3)
//!              ├─True→ print("Greater!")
//!              └─False→ print("Not greater")
//! ```
//!
//! ### Generated Code
//! ```rust,ignore
//! use pulsar_std::*;
//!
//! pub fn main() {
//!     // Pure node evaluations
//!     let node_add_result = add(2, 3);
//!     let node_gt_result = greater_than(node_add_result, 3);
//!     
//!     // Execution chain with inlined control flow
//!     if node_gt_result {
//!         print_string("Greater!".to_string());
//!     } else {
//!         print_string("Not greater".to_string());
//!     }
//! }
//! ```

pub mod code_generator;
pub mod pure_nodes;
pub mod function_nodes;
pub mod control_flow_nodes;
pub mod event_nodes;
pub mod imports;
pub mod formatting;

// Re-export main API
pub use code_generator::CodeGenerator;
pub use code_generator::generate_program;
pub use imports::{collect_node_dependencies, collect_node_imports};
pub use event_nodes::generate_file_header;
pub use formatting::format_rust_code;
