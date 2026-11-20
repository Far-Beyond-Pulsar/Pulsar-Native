//! # Pulsar Blueprint Macros
//!
//! Procedural macros for defining blueprint nodes in Rust.
//!
//! ## Macros
//!
//! - `#[blueprint]` - Mark a function as a blueprint node and auto-register it
//! - `#[bp_import]` - Declare external crate imports for a blueprint node
//! - `exec_output!()` - Define execution output points in control flow nodes
//! - `generate_icon_enum!()` - Generate an icon enum from SVG files in a directory

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

mod attrs;
mod blueprint_node;
mod derive_into_plot;
mod exec_pins;
mod generics;
mod icons;
mod script_native;
mod script_types;
mod type_constructor;

/// Derive `gpui::IntoElement` and `gpui::Element` for a type that implements `Plot`.
#[proc_macro_derive(IntoPlot)]
pub fn derive_into_plot(input: TokenStream) -> TokenStream {
    derive_into_plot::derive_into_plot(input)
}

/// Generate an `IconName` enum and its `path()` method by scanning a directory of SVG files.
///
/// Accepts a path relative to the calling crate's `CARGO_MANIFEST_DIR`.
/// Each `.svg` file becomes an enum variant using PascalCase conversion.
///
/// # Example
///
/// ```ignore
/// // Note: Icons are now provided by WGPUI-Component
/// // Use ui::assets::Assets to access icons from WGPUI-Component/assets/icons
/// generate_icon_enum!("../../../WGPUI-Component/assets/icons");
/// ```
#[proc_macro]
pub fn generate_icon_enum(input: TokenStream) -> TokenStream {
    icons::expand(input)
}

/// Mark a function as a blueprint node and automatically register it.
///
/// # Attributes
///
/// - `type`: Node type - `NodeTypes::pure`, `NodeTypes::fn_`, `NodeTypes::control_flow`, or `NodeTypes::event`
/// - `color`: Optional hex color for the node in the UI (e.g., `"#ff0000"`)
/// - `category`: Optional category for grouping nodes (e.g., `"Math"`)
/// - `intrinsic`: `true` for a node the Blueprint compiler implements itself,
///   such as one whose state must belong to each script instance (`do_once`,
///   `delay`). Its body only declares the node: the `exec_output!` markers give
///   the exec pins, and nothing is registered as a script native or run (the
///   emitted function panics). A body with real state would be shared by every
///   instance in the process, and a blocking one would stall the game thread.
///
/// # Examples
///
/// ## Pure Node
/// ```ignore
/// #[blueprint(type: NodeTypes::pure, category: "Math")]
/// fn add(a: i64, b: i64) -> i64 {
///     a + b
/// }
/// ```
///
/// ## Function Node
/// ```ignore
/// #[blueprint(type: NodeTypes::fn_, category: "Debug")]
/// fn print_string(message: String) {
///     tracing::trace!("[DEBUG] {}", message);
/// }
/// ```
///
/// ## Control Flow Node
/// ```ignore
/// #[blueprint(type: NodeTypes::control_flow, category: "Flow")]
/// fn branch(condition: bool) {
///     if condition {
///         exec_output!("True");
///     } else {
///         exec_output!("False");
///     }
/// }
/// ```
///
/// ## Node with External Imports
/// ```ignore
/// #[bp_import(reqwest::{Client, Error})]
/// #[bp_import(serde_json)]
/// #[blueprint(type: NodeTypes::fn_, category: "HTTP")]
/// fn http_get(url: String) -> String {
///     let client = Client::new();
///     // ... implementation
/// }
/// ```
#[proc_macro_attribute]
pub fn blueprint(args: TokenStream, input: TokenStream) -> TokenStream {
    blueprint_node::expand(args, input)
}

/// Mark an execution output point in a control flow node.
///
/// This macro is a marker that gets replaced by the compiler during code generation.
/// It should only be used inside functions marked with `#[blueprint(type: NodeTypes::control_flow)]`.
///
/// # Arguments
///
/// - `label`: String literal identifying this execution output (e.g., `"True"`, `"False"`, `"Body"`)
///
/// # Examples
///
/// ```ignore
/// #[blueprint(type: NodeTypes::control_flow)]
/// fn branch(condition: bool) {
///     if condition {
///         exec_output!("True");  // Nodes connected to "True" pin execute here
///     } else {
///         exec_output!("False"); // Nodes connected to "False" pin execute here
///     }
/// }
/// ```
#[proc_macro]
pub fn exec_output(input: TokenStream) -> TokenStream {
    let _label = parse_macro_input!(input as syn::LitStr);

    // At runtime, this expands to nothing
    // The compiler will replace it during code generation
    let expanded = quote! {
        ()
    };

    TokenStream::from(expanded)
}

/// Declare external crate imports for a blueprint node.
///
/// This attribute macro marks dependencies that should be:
/// 1. Added to the generated game's Cargo.toml
/// 2. Imported when the node is inlined in generated code
///
/// # Syntax
///
/// - `#[bp_import(crate_name)]` - Import entire crate
/// - `#[bp_import(crate_name::item)]` - Import specific item
/// - `#[bp_import(crate_name::{item1, item2})]` - Import multiple items
///
/// # Examples
///
/// ```ignore
/// #[bp_import(reqwest::{Client, Error})]
/// #[bp_import(serde_json)]
/// #[blueprint(type: NodeTypes::fn_, category: "HTTP")]
/// fn http_get(url: String) -> String {
///     let client = Client::new();
///     // ...
/// }
/// ```
#[proc_macro_attribute]
pub fn bp_import(_args: TokenStream, input: TokenStream) -> TokenStream {
    // This is a marker attribute - it doesn't transform the code
    // The #[blueprint] macro extracts these attributes
    input
}

/// Register a type constructor for the type system.
///
/// # Attributes
///
/// - `params`: Number of type parameters (e.g., 1 for `Box<T>`, 2 for `Result<T, E>`)
/// - `category`: Category for grouping (e.g., "Smart Pointers", "Collections")
/// - `description`: Optional description text
/// - `example`: Optional example usage
/// - `unwrapped_name`: The actual Rust type name (e.g., "Arc" for PArc)
///
/// # Examples
///
/// ```ignore
/// #[blueprint_type(params: 1, category: "Smart Pointers", description: "Thread-safe reference counting", unwrapped_name: "Arc")]
/// pub type PArc<T> = Arc<T>;
///
/// #[blueprint_type(params: 2, category: "Option & Result", description: "Success or error", unwrapped_name: "Result")]
/// pub type PResult<T, E> = Result<T, E>;
/// ```
#[proc_macro_attribute]
pub fn blueprint_type(args: TokenStream, input: TokenStream) -> TokenStream {
    type_constructor::expand(args, input)
}
