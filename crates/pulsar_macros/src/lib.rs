//! # Pulsar Blueprint Macros
//!
//! Procedural macros for defining blueprint nodes in Rust.
//!
//! ## Macros
//!
//! - `#[blueprint]` - Mark a function as a blueprint node and auto-register it
//! - `#[bp_import]` - Declare external crate imports for a blueprint node
//! - `exec_output!()` - Define execution output points in control flow nodes

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemFn, Pat, ReturnType, FnArg, Stmt, Expr};

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
///     println!("[DEBUG] {}", message);
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
    let node_type_str = if args_str.contains("NodeTypes :: pure") || args_str.contains("NodeTypes::pure") {
        "pure"
    } else if args_str.contains("NodeTypes :: fn_") || args_str.contains("NodeTypes::fn_") {
        "fn_"
    } else if args_str.contains("NodeTypes :: control_flow") || args_str.contains("NodeTypes::control_flow") {
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
    let params: Vec<_> = input.sig.inputs.iter().filter_map(|arg| {
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
    }).collect();

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
    let docs: Vec<String> = input.attrs.iter()
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
    let first_heading_idx = docs.iter().position(|line| line.trim_start().starts_with('#'));
    
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

    // Generate the registration const
    let registry_ident = syn::Ident::new(
        &format!("__BLUEPRINT_NODE__{}", fn_name_str.to_uppercase()),
        fn_name.span()
    );

    let node_type_ident = syn::Ident::new(node_type_str, fn_name.span());

    let expanded = quote! {
        #[allow(dead_code)]
        #input

        #[::linkme::distributed_slice(crate::BLUEPRINT_REGISTRY)]
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
    labels.into_iter().filter(|l| seen.insert(l.clone())).collect()
}

fn find_exec_in_block(block: &syn::Block, labels: &mut Vec<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Expr(expr, _) => find_exec_in_expr(expr, labels),
            Stmt::Macro(stmt_macro) => {
                if stmt_macro.mac.path.is_ident("exec_output") {
                    if let Ok(label) = syn::parse2::<syn::LitStr>(stmt_macro.mac.tokens.clone()) {
                        labels.push(label.value());
                    }
                }
            }
            _ => {}
        }
    }
}

fn find_exec_in_expr(expr: &Expr, labels: &mut Vec<String>) {
    match expr {
        Expr::Macro(macro_expr) => {
            if macro_expr.mac.path.is_ident("exec_output") {
                if let Ok(label) = syn::parse2::<syn::LitStr>(macro_expr.mac.tokens.clone()) {
                    labels.push(label.value());
                }
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
    let constructor_name = extract_string_value(&args_str, "unwrapped_name")
        .unwrap_or_else(|| {
            // Default: strip 'P' prefix if present (PArc -> Arc)
            type_name_str.strip_prefix('P').unwrap_or(&type_name_str).to_string()
        });

    // Parse parameters count
    let params_count = extract_number_value(&args_str, "params").unwrap_or(1);

    // Extract category
    let category = extract_string_value(&args_str, "category")
        .unwrap_or_else(|| "Other".to_string());

    // Extract description
    let description = extract_string_value(&args_str, "description")
        .unwrap_or_else(|| format!("{} type constructor", constructor_name));

    // Extract example
    let example = extract_string_value(&args_str, "example")
        .unwrap_or_else(|| {
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
        type_name.span()
    );

    let expanded = quote! {
        #[allow(dead_code)]
        #input

        #[::linkme::distributed_slice(crate::TYPE_CONSTRUCTOR_REGISTRY)]
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
