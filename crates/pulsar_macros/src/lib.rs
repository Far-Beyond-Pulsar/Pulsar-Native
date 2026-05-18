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
use syn::{parse_macro_input, Expr, FnArg, ItemFn, Pat, ReturnType, Stmt};

/// Convert an SVG filename to a PascalCase identifier.
///
/// Convention: lowercase the filename, strip `.svg`, split on `-`,
/// capitalize the first letter of each segment, join.
/// Underscores are preserved as-is (e.g. `android_dark.svg` → `Android_dark`).
fn filename_to_pascal(filename: &str) -> String {
    let name = filename.strip_suffix(".svg").unwrap_or(filename);
    let lowered = name.to_lowercase();
    lowered
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect()
}

/// Generate an `IconName` enum and its `path()` method by scanning a directory of SVG files.
///
/// Accepts a path relative to the calling crate's `CARGO_MANIFEST_DIR`.
/// Each `.svg` file becomes an enum variant using PascalCase conversion.
///
/// # Example
///
/// ```ignore
/// generate_icon_enum!("../../assets/icons");
/// ```
#[proc_macro]
pub fn generate_icon_enum(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::LitStr);
    let relative_path = input.value();

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let icons_dir = std::path::Path::new(&manifest_dir).join(&relative_path);

    let mut entries: Vec<(String, String)> = Vec::new();

    let dir = std::fs::read_dir(&icons_dir).unwrap_or_else(|e| {
        panic!(
            "generate_icon_enum: failed to read directory '{}': {}",
            icons_dir.display(),
            e
        )
    });

    for entry in dir {
        let entry = entry.expect("failed to read directory entry");
        let filename = entry.file_name().to_string_lossy().to_string();
        if filename.ends_with(".svg") {
            let variant_name = filename_to_pascal(&filename);
            let path = format!("icons/{}", filename);
            entries.push((variant_name, path));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let variants: Vec<proc_macro2::Ident> = entries
        .iter()
        .map(|(name, _)| proc_macro2::Ident::new(name, proc_macro2::Span::call_site()))
        .collect();
    let paths: Vec<&str> = entries.iter().map(|(_, p)| p.as_str()).collect();

    let expanded = quote! {
        #[derive(IntoElement, Clone, Debug)]
        pub enum IconName {
            #(#variants,)*
        }

        impl IconName {
            pub fn path(self) -> SharedString {
                match self {
                    #(Self::#variants => #paths,)*
                }
                .into()
            }
        }
    };

    TokenStream::from(expanded)
}

/// Mark a function as a blueprint node and automatically register it.
///
/// # Attributes
///
/// - `type`: Node type - `NodeTypes::pure`, `NodeTypes::fn_`, `NodeTypes::control_flow`, or `NodeTypes::event`
/// - `color`: Optional hex color for the node in the UI (e.g., `"#ff0000"`)
/// - `category`: Optional category for grouping nodes (e.g., `"Math"`)
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
    let input = parse_macro_input!(input as ItemFn);
    let args_str = args.to_string();

    // Extract function information
    let fn_name = &input.sig.ident;
    let fn_name_str = fn_name.to_string();

    // Parse node type
    let node_type_str =
        if args_str.contains("NodeTypes :: pure") || args_str.contains("NodeTypes::pure") {
            "pure"
        } else if args_str.contains("NodeTypes :: fn_") || args_str.contains("NodeTypes::fn_") {
            "fn_"
        } else if args_str.contains("NodeTypes :: control_flow")
            || args_str.contains("NodeTypes::control_flow")
        {
            "control_flow"
        } else if args_str.contains("NodeTypes :: event") || args_str.contains("NodeTypes::event") {
            "event"
        } else {
            "fn_" // Default
        };

    // Extract category
    let category = extract_string_value(&args_str, "category");
    let category_str = category.unwrap_or_else(|| "General".to_string());

    // Extract color
    let color = extract_string_value(&args_str, "color");
    let color_opt = if let Some(c) = color {
        quote! { Some(#c) }
    } else {
        quote! { None }
    };

    // Extract parameters
    let params: Vec<_> = input
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(ident) = &*pat_type.pat {
                    let param_name = ident.ident.to_string();
                    let ty = &*pat_type.ty;
                    let param_type = quote!(#ty).to_string();
                    return Some(quote! {
                        crate::NodeParameter {
                            name: #param_name,
                            ty: #param_type,
                        }
                    });
                }
            }
            None
        })
        .collect();

    // Extract return type
    let return_type = match &input.sig.output {
        ReturnType::Default => quote! { None },
        ReturnType::Type(_, ty) => {
            let ty_str = quote!(#ty).to_string();
            quote! { Some(#ty_str) }
        }
    };

    // Find exec_output calls
    let exec_outputs = find_exec_output_labels(&input);
    let exec_outputs_array = if exec_outputs.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#exec_outputs),*] }
    };

    // Determine exec inputs based on node type
    let exec_inputs = match node_type_str {
        "Pure" | "Event" => quote! { &[] },
        _ => quote! { &["exec"] },
    };

    // Build documentation from doc comments (/// or #[doc = "..."])
    let docs: Vec<String> = input
        .attrs
        .iter()
        .filter_map(|attr| {
            // Doc comments become #[doc = "..."] attributes
            if attr.path().is_ident("doc") {
                if let syn::Meta::NameValue(nv) = &attr.meta {
                    if let syn::Expr::Lit(expr_lit) = &nv.value {
                        if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                            return Some(lit_str.value().trim().to_string());
                        }
                    }
                }
            }
            None
        })
        .collect();

    // Create a clean function without macro attributes for source code display
    let mut clean_input = input.clone();
    clean_input.attrs.retain(|attr| attr.path().is_ident("doc"));
    clean_input.attrs.clear(); // Remove all attributes including doc comments
    let fn_source = quote!(#clean_input).to_string();

    // Find first heading in docs (line starting with #)
    let first_heading_idx = docs
        .iter()
        .position(|line| line.trim_start().starts_with('#'));

    let mut final_docs = Vec::new();

    if let Some(heading_idx) = first_heading_idx {
        // Add docs before first heading
        final_docs.extend(docs[..heading_idx].iter().cloned());

        // Add source code block
        if !final_docs.is_empty() {
            final_docs.push("".to_string()); // Empty line separator
        }
        final_docs.push("```rust".to_string());
        final_docs.push(fn_source.clone());
        final_docs.push("```".to_string());

        // Add rest of docs (from heading onwards)
        final_docs.push("".to_string()); // Empty line separator
        final_docs.extend(docs[heading_idx..].iter().cloned());
    } else {
        // No heading found, add all docs first, then source
        final_docs.extend(docs);
        if !final_docs.is_empty() {
            final_docs.push("".to_string()); // Empty line separator
        }
        final_docs.push("```rust".to_string());
        final_docs.push(fn_source.clone());
        final_docs.push("```".to_string());
    }

    let docs_array = quote! { &[#(#final_docs),*] };

    // Extract bp_import attributes
    let imports = extract_bp_imports(&input);
    let imports_array = if imports.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#imports),*] }
    };

    // wasm_safe: false → macro emits #[cfg(not(target_arch = "wasm32"))] around the fn
    let wasm_safe = !args_str.contains("wasm_safe : false") && !args_str.contains("wasm_safe:false");

    let registry_ident = syn::Ident::new(
        &format!("__BLUEPRINT_NODE__{}", fn_name_str.to_uppercase()),
        fn_name.span(),
    );
    let node_type_ident = syn::Ident::new(node_type_str, fn_name.span());

    // WASM export wrapper — emitted under cfg(wasm32) for WASM-ABI-compatible signatures
    let wasm_export = if wasm_safe {
        generate_wasm_export(&input, fn_name, &fn_name_str)
    } else {
        quote! {}
    };

    // Native dispatch shim — __bp_dispatch_<name>, always on non-wasm32 builds.
    // The pulsar_bp_executor loads these symbols from the native cdylib by name.
    let dispatch_shim = if wasm_safe {
        generate_dispatch_shim(&input, fn_name, &fn_name_str)
    } else {
        quote! {}
    };

    // For wasm_safe: false nodes, wrap the entire definition in a native-only cfg
    let fn_definition = if wasm_safe {
        quote! { #[allow(dead_code)] #input }
    } else {
        quote! {
            #[cfg(not(target_arch = "wasm32"))]
            #[allow(dead_code)]
            #input
        }
    };

    // linkme registration is native-only (linkme uses linker sections unsupported on wasm32)
    let registry_registration = if wasm_safe {
        quote! {
            #[cfg(feature = "native")]
            #[::linkme::distributed_slice(crate::registry::native_registry::BLUEPRINT_REGISTRY)]
            #[linkme(crate = ::linkme)]
            static #registry_ident: crate::NodeMetadata = crate::NodeMetadata {
                name: #fn_name_str,
                node_type: crate::NodeTypes::#node_type_ident,
                params: &[#(#params),*],
                return_type: #return_type,
                exec_inputs: #exec_inputs,
                exec_outputs: #exec_outputs_array,
                function_source: #fn_source,
                documentation: #docs_array,
                category: #category_str,
                color: #color_opt,
                imports: #imports_array,
            };
        }
    } else {
        quote! {
            #[cfg(all(not(target_arch = "wasm32"), feature = "native"))]
            #[::linkme::distributed_slice(crate::registry::native_registry::BLUEPRINT_REGISTRY)]
            #[linkme(crate = ::linkme)]
            static #registry_ident: crate::NodeMetadata = crate::NodeMetadata {
                name: #fn_name_str,
                node_type: crate::NodeTypes::#node_type_ident,
                params: &[#(#params),*],
                return_type: #return_type,
                exec_inputs: #exec_inputs,
                exec_outputs: #exec_outputs_array,
                function_source: #fn_source,
                documentation: #docs_array,
                category: #category_str,
                color: #color_opt,
                imports: #imports_array,
            };
        }
    };

    let expanded = quote! {
        #fn_definition
        #registry_registration
        #wasm_export
        #dispatch_shim
    };

    TokenStream::from(expanded)
}

/// Extract a string value from an attribute string like `category: "Math"`
fn extract_string_value(attr_str: &str, key: &str) -> Option<String> {
    if let Some(key_pos) = attr_str.find(key) {
        if let Some(quote_start) = attr_str[key_pos..].find('"') {
            let quote_start = key_pos + quote_start + 1;
            if let Some(quote_end) = attr_str[quote_start..].find('"') {
                return Some(attr_str[quote_start..quote_start + quote_end].to_string());
            }
        }
    }
    None
}

/// Extract bp_import attributes from a function
fn extract_bp_imports(func: &ItemFn) -> Vec<proc_macro2::TokenStream> {
    let mut imports = Vec::new();

    for attr in &func.attrs {
        if attr.path().is_ident("bp_import") {
            // Parse the import specification
            if let Ok(import_spec) = parse_bp_import_attr(attr) {
                imports.push(import_spec);
            }
        }
    }

    imports
}

/// Parse a bp_import attribute into NodeImport tokens
/// Handles forms like:
/// - #[bp_import(reqwest)]
/// - #[bp_import(reqwest::Client)]
/// - #[bp_import(reqwest::{Client, Error})]
fn parse_bp_import_attr(attr: &syn::Attribute) -> syn::Result<proc_macro2::TokenStream> {
    let tokens = attr.meta.require_list()?.tokens.clone();
    let tokens_str = tokens.to_string();

    // Parse the import path
    // Format can be: "crate_name" or "crate_name :: item" or "crate_name :: { item1 , item2 }"
    let (crate_name, items) = parse_import_path(&tokens_str);

    let items_array = if items.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#items),*] }
    };

    Ok(quote! {
        crate::NodeImport {
            crate_name: #crate_name,
            items: #items_array,
        }
    })
}

/// Parse an import path string like "reqwest::{Client, Error}" into (crate_name, [items])
fn parse_import_path(path_str: &str) -> (String, Vec<String>) {
    let path_str = path_str.trim();

    // Check if there's a :: separator
    if let Some(sep_pos) = path_str.find("::") {
        let crate_name = path_str[..sep_pos].trim().to_string();
        let rest = path_str[sep_pos + 2..].trim();

        // Check if items are in braces
        if rest.starts_with('{') && rest.ends_with('}') {
            // Parse items from braces
            let items_str = &rest[1..rest.len() - 1];
            let items: Vec<String> = items_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (crate_name, items)
        } else {
            // Single item without braces
            (crate_name, vec![rest.to_string()])
        }
    } else {
        // No ::, just a crate name
        (path_str.to_string(), vec![])
    }
}

/// Find all exec_output!() labels in a function
fn find_exec_output_labels(func: &ItemFn) -> Vec<String> {
    let mut labels = Vec::new();
    find_exec_in_block(&func.block, &mut labels);

    // Remove duplicates while preserving order
    let mut seen = std::collections::HashSet::new();
    labels
        .into_iter()
        .filter(|l| seen.insert(l.clone()))
        .collect()
}

fn find_exec_in_block(block: &syn::Block, labels: &mut Vec<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(expr, _) => find_exec_in_expr(expr, labels),
            Stmt::Macro(stmt_macro) if stmt_macro.mac.path.is_ident("exec_output") => {
                if let Ok(label) = syn::parse2::<syn::LitStr>(stmt_macro.mac.tokens.clone()) {
                    labels.push(label.value());
                }
            }
            _ => {}
        }
    }
}

fn find_exec_in_expr(expr: &Expr, labels: &mut Vec<String>) {
    match expr {
        Expr::Macro(macro_expr) if macro_expr.mac.path.is_ident("exec_output") => {
            if let Ok(label) = syn::parse2::<syn::LitStr>(macro_expr.mac.tokens.clone()) {
                labels.push(label.value());
            }
        }
        Expr::Block(block_expr) => find_exec_in_block(&block_expr.block, labels),
        Expr::If(if_expr) => {
            find_exec_in_block(&if_expr.then_branch, labels);
            if let Some((_, else_branch)) = &if_expr.else_branch {
                find_exec_in_expr(else_branch, labels);
            }
        }
        Expr::Match(match_expr) => {
            for arm in &match_expr.arms {
                find_exec_in_expr(&arm.body, labels);
            }
        }
        Expr::Loop(loop_expr) => find_exec_in_block(&loop_expr.body, labels),
        Expr::ForLoop(for_expr) => find_exec_in_block(&for_expr.body, labels),
        Expr::While(while_expr) => find_exec_in_block(&while_expr.body, labels),
        Expr::Unsafe(unsafe_expr) => find_exec_in_block(&unsafe_expr.block, labels),
        _ => {}
    }
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
    let input = parse_macro_input!(input as syn::ItemType);
    let args_str = args.to_string();

    let type_name = &input.ident;
    let type_name_str = type_name.to_string();

    // Extract the unwrapped name (the actual Rust type like "Arc", "Box")
    let constructor_name = extract_string_value(&args_str, "unwrapped_name").unwrap_or_else(|| {
        // Default: strip 'P' prefix if present (PArc -> Arc)
        type_name_str
            .strip_prefix('P')
            .unwrap_or(&type_name_str)
            .to_string()
    });

    // Parse parameters count
    let params_count = extract_number_value(&args_str, "params").unwrap_or(1);

    // Extract category
    let category =
        extract_string_value(&args_str, "category").unwrap_or_else(|| "Other".to_string());

    // Extract description
    let description = extract_string_value(&args_str, "description")
        .unwrap_or_else(|| format!("{} type constructor", constructor_name));

    // Extract example
    let example = extract_string_value(&args_str, "example").unwrap_or_else(|| {
        if params_count == 1 {
            format!("{}<T>", constructor_name)
        } else if params_count == 2 {
            format!("{}<T, E>", constructor_name)
        } else {
            format!("{}<...>", constructor_name)
        }
    });

    // Generate the registration const
    let registry_ident = syn::Ident::new(
        &format!("__TYPE_CONSTRUCTOR__{}", constructor_name.to_uppercase()),
        type_name.span(),
    );

    let expanded = quote! {
        #[allow(dead_code)]
        #input

        #[cfg(feature = "native")]
        #[::linkme::distributed_slice(crate::registry::native_type_registry::TYPE_CONSTRUCTOR_REGISTRY)]
        #[linkme(crate = ::linkme)]
        static #registry_ident: crate::TypeConstructorMetadata = crate::TypeConstructorMetadata {
            name: #constructor_name,
            params_count: #params_count,
            category: #category,
            description: #description,
            example: #example,
        };
    };

    TokenStream::from(expanded)
}

// ── WASM export generation ────────────────────────────────────────────────────

/// Map a Rust type string to its WASM ABI equivalent.
/// Returns `None` for types that cannot be passed through the C ABI to WASM.
fn rust_type_to_wasm(ty: &str) -> Option<&'static str> {
    match ty.trim() {
        "i64" | "u64" => Some("i64"),
        "i32" | "u32" | "i16" | "u16" | "i8" | "u8" | "usize" | "isize" => Some("i32"),
        "f64" => Some("f64"),
        "f32" => Some("f32"),
        "bool" => Some("i32"), // bool → i32 (0/1)
        _ => None,
    }
}

/// Generate the conversion expression from a WASM i32 back to `bool`.
fn wasm_to_rust_param(param_name: &str, rust_ty: &str) -> String {
    match rust_ty.trim() {
        "bool" => format!("({} != 0)", param_name),
        "i8" | "i16" | "i32" => format!("({} as {})", param_name, rust_ty),
        "u8" | "u16" | "u32" | "usize" | "isize" => format!("({} as {})", param_name, rust_ty),
        _ => param_name.to_string(),
    }
}

/// Generate the conversion from the Rust return value to its WASM type.
fn rust_ret_to_wasm(expr: &str, rust_ty: &str) -> String {
    match rust_ty.trim() {
        "bool" => format!("({} as i32)", expr),
        "i8" | "i16" | "u8" | "u16" | "u32" | "usize" | "isize" => {
            format!("({} as i64)", expr)
        }
        _ => expr.to_string(),
    }
}

/// Emit a `#[cfg(target_arch = "wasm32")] #[no_mangle] pub extern "C"` wrapper
/// for the given blueprint function if all its parameter types and return type are
/// WASM-ABI compatible. Returns an empty token stream for functions with complex types.
fn generate_wasm_export(
    func: &ItemFn,
    fn_name: &proc_macro2::Ident,
    fn_name_str: &str,
) -> proc_macro2::TokenStream {
    // Collect (param_name, rust_type_str) for all typed params
    let mut param_info: Vec<(String, String)> = Vec::new();
    for arg in &func.sig.inputs {
        if let FnArg::Typed(pat_type) = arg {
            if let Pat::Ident(ident) = &*pat_type.pat {
                let name = ident.ident.to_string();
                let ty_str = quote::quote!(#pat_type.ty).to_string();
                // Re-extract the type cleanly
                let ty = &*pat_type.ty;
                let ty_str = quote::quote!(#ty).to_string();
                param_info.push((name, ty_str));
            }
        }
    }

    // Determine return type
    let ret_rust_ty = match &func.sig.output {
        ReturnType::Default => None,
        ReturnType::Type(_, ty) => Some(quote::quote!(#ty).to_string()),
    };

    // Check all params are WASM-compatible
    for (_, ty) in &param_info {
        if rust_type_to_wasm(ty).is_none() {
            return quote! {}; // skip — complex type
        }
    }

    // Check return type
    if let Some(ret_ty) = &ret_rust_ty {
        if rust_type_to_wasm(ret_ty).is_none() {
            return quote! {};
        }
    }

    // Build WASM param list: (name: wasm_type, ...)
    let wasm_params: Vec<proc_macro2::TokenStream> = param_info
        .iter()
        .map(|(name, rust_ty)| {
            let wasm_ty = rust_type_to_wasm(rust_ty).unwrap();
            let ident = proc_macro2::Ident::new(name, proc_macro2::Span::call_site());
            let wasm_ty_ident = proc_macro2::Ident::new(wasm_ty, proc_macro2::Span::call_site());
            quote! { #ident: #wasm_ty_ident }
        })
        .collect();

    // Build call arguments with type conversions
    let call_args: Vec<proc_macro2::TokenStream> = param_info
        .iter()
        .map(|(name, rust_ty)| {
            let expr_str = wasm_to_rust_param(name, rust_ty);
            expr_str.parse::<proc_macro2::TokenStream>().unwrap_or_else(|_| {
                let ident = proc_macro2::Ident::new(name, proc_macro2::Span::call_site());
                quote! { #ident }
            })
        })
        .collect();

    let export_name = format!("bp_{}", fn_name_str);

    // All wasm exports live in a private submodule so they don't collide with the
    // original function definition in the same namespace.
    let mod_name_str = format!("__wasm_export_{}", fn_name_str);
    let mod_ident = proc_macro2::Ident::new(&mod_name_str, proc_macro2::Span::call_site());

    match &ret_rust_ty {
        None => {
            quote! {
                #[cfg(target_arch = "wasm32")]
                mod #mod_ident {
                    #[no_mangle]
                    pub unsafe extern "C" fn #fn_name(#(#wasm_params),*) {
                        super::#fn_name(#(#call_args),*);
                    }
                }
            }
        }
        Some(ret_ty) => {
            let wasm_ret = rust_type_to_wasm(ret_ty).unwrap();
            let wasm_ret_ident =
                proc_macro2::Ident::new(wasm_ret, proc_macro2::Span::call_site());
            let call_expr = quote! { super::#fn_name(#(#call_args),*) };
            let call_str = quote! { #call_expr }.to_string();
            let ret_expr = rust_ret_to_wasm(&call_str, ret_ty)
                .parse::<proc_macro2::TokenStream>()
                .unwrap_or(call_expr.clone());

            quote! {
                #[cfg(target_arch = "wasm32")]
                mod #mod_ident {
                    #[no_mangle]
                    pub unsafe extern "C" fn #fn_name(#(#wasm_params),*) -> #wasm_ret_ident {
                        #ret_expr
                    }
                }
            }
        }
    }
}

// ── Native dispatch shim generation ──────────────────────────────────────────
//
// Each `#[blueprint]` function with a numeric/bool signature gets a
// `__bp_dispatch_<name>` symbol emitted alongside it (native builds only).
//
// ABI:  unsafe extern "C" fn(inputs: *const u64, output: *mut u64)
//   - inputs:  contiguous u64 slot values, one per parameter
//   - output:  pointer to a single u64 for the return value (ignored for void)
//
// The shim reads each parameter as its real Rust type from raw u64 bits,
// calls the actual function, and writes the result back as raw bits.
// The executor's hot loop passes slot pointers directly — zero conversion.

fn dispatch_read(ty: &str, idx: usize) -> proc_macro2::TokenStream {
    let i = proc_macro2::Literal::usize_unsuffixed(idx);
    match ty.trim() {
        "f64"            => quote! { f64::from_bits(*inputs.add(#i)) },
        "f32"            => quote! { f32::from_bits(*inputs.add(#i) as u32) },
        "bool"           => quote! { *inputs.add(#i) != 0 },
        "i64"            => quote! { *inputs.add(#i) as i64 },
        "u64"            => quote! { *inputs.add(#i) },
        "i32"            => quote! { *inputs.add(#i) as i32 },
        "u32"            => quote! { *inputs.add(#i) as u32 },
        "i16"            => quote! { *inputs.add(#i) as i16 },
        "u16"            => quote! { *inputs.add(#i) as u16 },
        "i8"             => quote! { *inputs.add(#i) as i8 },
        "u8"             => quote! { *inputs.add(#i) as u8 },
        "isize"          => quote! { *inputs.add(#i) as isize },
        "usize"          => quote! { *inputs.add(#i) as usize },
        _                => return quote! {}, // unsupported — caller must skip
    }
}

fn dispatch_write(ty: &str, result: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    match ty.trim() {
        "f64"   => quote! { *output = (#result).to_bits(); },
        "f32"   => quote! { *output = (#result).to_bits() as u64; },
        "bool"  => quote! { *output = (#result) as u64; },
        "i64"   => quote! { *output = (#result) as u64; },
        "u64"   => quote! { *output = #result; },
        "i32"   => quote! { *output = (#result) as u32 as u64; },
        "u32"   => quote! { *output = (#result) as u64; },
        "i16"   => quote! { *output = (#result) as u16 as u64; },
        "u16"   => quote! { *output = (#result) as u64; },
        "i8"    => quote! { *output = (#result) as u8 as u64; },
        "u8"    => quote! { *output = (#result) as u64; },
        "isize" => quote! { *output = (#result) as u64; },
        "usize" => quote! { *output = (#result) as u64; },
        "()"    => quote! { let _ = #result; },
        _       => return quote! {},
    }
}

fn is_dispatch_compatible(ty: &str) -> bool {
    matches!(ty.trim(),
        "i8"|"i16"|"i32"|"i64"|"u8"|"u16"|"u32"|"u64"
        |"f32"|"f64"|"bool"|"isize"|"usize"
    )
}

fn generate_dispatch_shim(
    func: &ItemFn,
    fn_name: &proc_macro2::Ident,
    fn_name_str: &str,
) -> proc_macro2::TokenStream {
    // Collect (ident, type_str) for all typed params
    let mut params: Vec<(proc_macro2::Ident, String)> = Vec::new();
    for arg in &func.sig.inputs {
        if let FnArg::Typed(pt) = arg {
            if let Pat::Ident(ident) = &*pt.pat {
                let ty_str = { let ty = &*pt.ty; quote::quote!(#ty).to_string() };
                params.push((ident.ident.clone(), ty_str));
            }
        }
    }

    // Check all params are dispatchable
    for (_, ty) in &params {
        if !is_dispatch_compatible(ty) {
            return quote! {}; // skip — complex type
        }
    }

    // Return type
    let ret_ty_str = match &func.sig.output {
        ReturnType::Default => "()".to_string(),
        ReturnType::Type(_, ty) => quote::quote!(#ty).to_string(),
    };
    if ret_ty_str != "()" && !is_dispatch_compatible(&ret_ty_str) {
        return quote! {};
    }

    // Build read expressions
    let reads: Vec<proc_macro2::TokenStream> = params.iter().enumerate()
        .map(|(i, (_, ty))| dispatch_read(ty, i))
        .collect();
    let param_idents: Vec<&proc_macro2::Ident> = params.iter().map(|(id, _)| id).collect();

    let call_expr = quote! { #fn_name(#(#reads),*) };

    let write_expr = dispatch_write(&ret_ty_str, call_expr);

    let shim_name_str = format!("__bp_dispatch_{}", fn_name_str);
    let shim_ident = proc_macro2::Ident::new(&shim_name_str, fn_name.span());

    quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #[no_mangle]
        pub unsafe extern "C" fn #shim_ident(inputs: *const u64, output: *mut u64) {
            #write_expr
        }
    }
}

/// Extract a number value from an attribute string like `params: 1`
fn extract_number_value(attr_str: &str, key: &str) -> Option<usize> {
    if let Some(key_pos) = attr_str.find(key) {
        let after_key = &attr_str[key_pos + key.len()..];
        if let Some(colon_pos) = after_key.find(':') {
            let after_colon = &after_key[colon_pos + 1..];
            // Find the first sequence of digits
            let digits: String = after_colon
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(num) = digits.parse() {
                return Some(num);
            }
        }
    }
    None
}
