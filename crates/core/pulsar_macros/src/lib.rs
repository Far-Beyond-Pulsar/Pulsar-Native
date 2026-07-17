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

mod derive_into_plot;

/// Derive `gpui::IntoElement` and `gpui::Element` for a type that implements `Plot`.
#[proc_macro_derive(IntoPlot)]
pub fn derive_into_plot(input: TokenStream) -> TokenStream {
    derive_into_plot::derive_into_plot(input)
}

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
/// // Note: Icons are now provided by WGPUI-Component
/// // Use ui::assets::Assets to access icons from WGPUI-Component/assets/icons
/// generate_icon_enum!("../../../WGPUI-Component/assets/icons");
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

    // Extract parameters — bake size_of/align_of at compile time using the actual type token.
    // For generic functions: substitute each unbound type parameter with `()` before computing
    // size_of/align_of.  This means:
    //   • bare T       → size_of::<()>()  = 0  (signal: resolve this slot via graph traversal)
    //   • Vec<T>       → size_of::<Vec<()>>() = 24  (wrapper size, fixed regardless of T)
    //   • Arc<T>       → size_of::<Arc<()>>() = 8
    // No lookup table needed — the Rust compiler evaluates everything at compile time.
    let is_generic = !input.sig.generics.params.is_empty();
    // Collect unbound type-parameter names (e.g. "T", "U") so we can substitute them.
    let generic_param_names: std::collections::HashSet<String> = input
        .sig
        .generics
        .params
        .iter()
        .filter_map(|p| {
            if let syn::GenericParam::Type(tp) = p {
                Some(tp.ident.to_string())
            } else {
                None
            }
        })
        .collect();

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
                    let (size_expr, align_expr) = if is_generic {
                        let subst = substitute_generics_with_unit(ty, &generic_param_names);
                        (
                            quote! { ::std::mem::size_of::<#subst>() },
                            quote! { ::std::mem::align_of::<#subst>() },
                        )
                    } else {
                        (
                            quote! { ::std::mem::size_of::<#ty>() },
                            quote! { ::std::mem::align_of::<#ty>() },
                        )
                    };

                    // Generate type_info accessor function for Reflectable types
                    // For generic types, this will return None (types resolved at instantiation)
                    let type_info_fn = if is_generic {
                        quote! { None }
                    } else {
                        quote! {
                            Some(|| {
                                // Try to get type info via Reflectable trait
                                // This is a compile-time check: if T doesn't implement Reflectable,
                                // the code still compiles but returns None at runtime
                                use ::pulsar_reflection::type_traits::Reflectable as _;
                                ::pulsar_reflection::RUNTIME_TYPE_REGISTRY.get::<#ty>()
                            })
                        }
                    };

                    return Some(quote! {
                        crate::NodeParameter {
                            name: #param_name,
                            ty: #param_type,
                            size: #size_expr,
                            align: #align_expr,
                            type_info_fn: #type_info_fn,
                        }
                    });
                }
            }
            None
        })
        .collect();

    // Extract return type and bake size_of/align_of for the return type at compile time.
    // For void (Default or "()") return: size=0, align=1.
    // For "!" (never/diverging) return: also treated as void.
    // For generic functions: substitute T→() to get the wrapper size.
    let (return_type, return_size, return_align, return_type_info_fn) = match &input.sig.output {
        ReturnType::Default => (
            quote! { None },
            quote! { 0usize },
            quote! { 1usize },
            quote! { None },
        ),
        ReturnType::Type(_, ty) => {
            let ty_str = quote!(#ty).to_string();
            let ty_trimmed = ty_str.trim();
            if ty_trimmed == "()" || ty_trimmed == "!" {
                (
                    quote! { None },
                    quote! { 0usize },
                    quote! { 1usize },
                    quote! { None },
                )
            } else if is_generic {
                let subst = substitute_generics_with_unit(ty, &generic_param_names);
                (
                    quote! { Some(#ty_str) },
                    quote! { ::std::mem::size_of::<#subst>() },
                    quote! { ::std::mem::align_of::<#subst>() },
                    quote! { None }, // Generic return types resolved at instantiation
                )
            } else {
                // Generate type_info accessor for non-generic return types
                let type_info_fn = quote! {
                    Some(|| {
                        use ::pulsar_reflection::type_traits::Reflectable as _;
                        ::pulsar_reflection::RUNTIME_TYPE_REGISTRY.get::<#ty>()
                    })
                };
                (
                    quote! { Some(#ty_str) },
                    quote! { ::std::mem::size_of::<#ty>() },
                    quote! { ::std::mem::align_of::<#ty>() },
                    type_info_fn,
                )
            }
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

    // ── Multi-output detection ────────────────────────────────────────────────
    //
    // Detect named output pins from:
    //   a. `#[output(name = "...")]` attrs (via doc markers `__bp_output:<name>`)
    //   b. `bp_return!(name: expr, ...)` macros in the function body
    //
    // When outputs are present, the return type MUST be a tuple. The arity must
    // match the number of output names. Individual tuple element types are used
    // to compute per-pin size/align for the `OutputParamMeta` array.

    let output_names_from_doc = extract_output_names_from_doc(&input);
    let bp_return_data = find_bp_return_in_body(&input);

    let output_names: Vec<String> = if !output_names_from_doc.is_empty() {
        output_names_from_doc
    } else if let Some((ref labels, _)) = bp_return_data {
        labels.clone()
    } else {
        Vec::new()
    };

    // Generate output_params array tokens
    let output_params_tokens = if output_names.is_empty() {
        quote! { &[] }
    } else {
        // Parse return type as tuple and extract element types
        let tuple_elem_types: Vec<syn::Type> = match &input.sig.output {
            ReturnType::Type(_, ty) => {
                if let syn::Type::Tuple(tup) = ty.as_ref() {
                    if tup.elems.is_empty() {
                        panic!("#[blueprint] function '{}' has output pins but return type is unit `()`", fn_name_str);
                    }
                    tup.elems.iter().cloned().collect()
                } else {
                    panic!(
                        "#[blueprint] function '{}' has output pins but return type is not a tuple. \
                         Multi-output nodes must return a tuple.",
                        fn_name_str
                    );
                }
            }
            ReturnType::Default => {
                panic!(
                    "#[blueprint] function '{}' has output pins but no return type. \
                     Multi-output nodes must return a tuple.",
                    fn_name_str
                );
            }
        };

        if output_names.len() != tuple_elem_types.len() {
            panic!(
                "#[blueprint] function '{}' has {} output pins but the tuple return type has {} elements. \
                 The number of output pins must match the tuple arity.",
                fn_name_str,
                output_names.len(),
                tuple_elem_types.len()
            );
        }

        let output_items: Vec<proc_macro2::TokenStream> = output_names.iter().zip(tuple_elem_types.iter()).map(|(name, elem_ty)| {
            let ty_str = quote!(#elem_ty).to_string();
            let (size_expr, align_expr) = if is_generic {
                let subst = substitute_generics_with_unit(elem_ty, &generic_param_names);
                (
                    quote! { ::std::mem::size_of::<#subst>() },
                    quote! { ::std::mem::align_of::<#subst>() },
                )
            } else {
                (
                    quote! { ::std::mem::size_of::<#elem_ty>() },
                    quote! { ::std::mem::align_of::<#elem_ty>() },
                )
            };
            quote! {
                crate::OutputParamMeta {
                    name: #name,
                    ty: #ty_str,
                    size: #size_expr,
                    align: #align_expr,
                }
            }
        }).collect();

        quote! { &[#(#output_items),*] }
    };

    // native_only: true (via wasm_safe: false) — node uses OS/threading APIs unavailable
    // in a cdylib context; wrap definition to exclude from those builds.
    let native_only =
        args_str.contains("wasm_safe : false") || args_str.contains("wasm_safe:false");

    let registry_ident = syn::Ident::new(
        &format!("__BLUEPRINT_NODE__{}", fn_name_str.to_uppercase()),
        fn_name.span(),
    );
    let node_type_ident = syn::Ident::new(node_type_str, fn_name.span());

    // Native dispatch shim — __bp_dispatch_<name>.
    // pulsar_bp_executor loads these from the compiled cdylib by symbol name.
    let dispatch_shim = if !native_only {
        generate_dispatch_shim(&input, fn_name, &fn_name_str)
    } else {
        quote! {}
    };

    // native_only nodes are wrapped so they compile out in non-native (cdylib) builds
    let fn_definition = if native_only {
        quote! {
            #[cfg(not(target_arch = "wasm32"))]
            #[allow(dead_code)]
            #input
        }
    } else {
        quote! { #[allow(dead_code)] #input }
    };

    // Registry registration requires linkme (native feature only)
    let registry_cfg = if native_only {
        quote! { #[cfg(all(not(target_arch = "wasm32"), feature = "native"))] }
    } else {
        quote! { #[cfg(feature = "native")] }
    };
    let registry_registration = quote! {
        #registry_cfg
        #[::linkme::distributed_slice(crate::registry::native_registry::BLUEPRINT_REGISTRY)]
        #[linkme(crate = ::linkme)]
        static #registry_ident: crate::NodeMetadata = crate::NodeMetadata {
            name: #fn_name_str,
            node_type: crate::NodeTypes::#node_type_ident,
            params: &[#(#params),*],
            return_type: #return_type,
            return_size: #return_size,
            return_align: #return_align,
            return_type_info_fn: #return_type_info_fn,
            exec_inputs: #exec_inputs,
            exec_outputs: #exec_outputs_array,
            function_source: #fn_source,
            documentation: #docs_array,
            category: #category_str,
            color: #color_opt,
            imports: #imports_array,
            output_params: #output_params_tokens,
        };
    };

    let expanded = quote! {
        #fn_definition
        #registry_registration
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

/// Extract output pin names from `#[doc = " __bp_output:<name>"]` markers
/// injected by the `#[output]` attribute macro.
fn extract_output_names_from_doc(func: &ItemFn) -> Vec<String> {
    const PREFIX: &str = " __bp_output:";
    let mut names = Vec::new();
    for attr in &func.attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(nv) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &nv.value {
                    if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                        let val = lit_str.value();
                        if let Some(rest) = val.strip_prefix(PREFIX) {
                            names.push(rest.trim().to_string());
                        }
                    }
                }
            }
        }
    }
    names
}

/// Parse a `bp_return!(label1: expr1, label2: expr2, ...)` macro call.
/// Returns `(labels, expressions)`.
fn parse_bp_return(tokens: &proc_macro2::TokenStream) -> Option<(Vec<String>, Vec<proc_macro2::TokenStream>)> {
    // Format: label1 : expr1 , label2 : expr2 , ...
    // We parse as a series of (ident : expr) pairs separated by commas.
    use syn::parse::{Parse, ParseStream};

    struct BpReturnArgs {
        labels: Vec<String>,
        exprs: Vec<proc_macro2::TokenStream>,
    }

    impl Parse for BpReturnArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            let mut labels = Vec::new();
            let mut exprs = Vec::new();

            while !input.is_empty() {
                // Parse label: ident
                let label: syn::Ident = input.parse()?;
                // Parse colon
                let _colon: syn::Token![:] = input.parse()?;
                // Parse expression
                let expr: syn::Expr = input.parse()?;

                labels.push(label.to_string());
                exprs.push(quote!(#expr));

                // Optional trailing comma
                if !input.is_empty() {
                    let _comma: syn::Token![,] = input.parse()?;
                }
            }

            Ok(BpReturnArgs { labels, exprs })
        }
    }

    syn::parse2::<BpReturnArgs>(tokens.clone()).ok().map(|a| (a.labels, a.exprs))
}

/// Scan the function body for `bp_return!` macro calls and extract output labels + return expressions.
/// Also rewrites the body to replace `bp_return!` with `return (...)` if requested.
fn find_bp_return_in_body(func: &ItemFn) -> Option<(Vec<String>, Vec<proc_macro2::TokenStream>)> {
    for stmt in &func.block.stmts {
        if let Stmt::Macro(stmt_macro) = stmt {
            if stmt_macro.mac.path.is_ident("bp_return") {
                return parse_bp_return(&stmt_macro.mac.tokens);
            }
        }
    }
    for stmt in &func.block.stmts {
        if let Stmt::Expr(expr, _) = stmt {
            if let Expr::Macro(macro_expr) = expr {
                if macro_expr.mac.path.is_ident("bp_return") {
                    return parse_bp_return(&macro_expr.mac.tokens);
                }
            }
        }
    }
    None
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

/// Declare a named output pin on a multi-output blueprint node.
///
/// Must come before `#[blueprint]`. Each `#[output]` maps positionally to a
/// tuple element in the return type.  The number of `#[output]` attrs must
/// match the tuple arity.
///
/// # Examples
///
/// ```ignore
/// #[output(name = "quotient")]
/// #[output(name = "remainder")]
/// #[blueprint(type: NodeTypes::pure, category: "Math")]
/// fn div_mod(a: i64, b: i64) -> (i64, i64) {
///     (a / b, a % b)
/// }
/// ```
///
/// Alternatively, use the `bp_return!` macro inside the function body which
/// generates the same metadata automatically:
///
/// ```ignore
/// #[blueprint(type: NodeTypes::pure, category: "Math")]
/// fn div_mod(a: i64, b: i64) -> (i64, i64) {
///     bp_return!(quotient: a / b, remainder: a % b);
/// }
/// ```
#[proc_macro_attribute]
pub fn output(args: TokenStream, input: TokenStream) -> TokenStream {
    // Parse `name = "..."` from args
    let args_str = args.to_string();
    let name = extract_string_value(&args_str, "name")
        .unwrap_or_else(|| panic!("#[output] requires name = \"...\""));

    // Inject a doc-comment marker that #[blueprint] will scan.
    // We use a doc comment because it survives proc-macro expansion ordering
    // — #[output] is outer, #[blueprint] is inner, and doc attrs on the
    // function item are visible to #[blueprint] after #[output] passes through.
    let marker = format!(" __bp_output:{}", name);

    // The input already has #[blueprint] and the function definition.
    // We need to inject our doc comment attribute BEFORE all other attrs.
    let input_str = input.to_string();
    let result = format!(
        "#[doc = \"{}\"]\n{}",
        marker,
        input_str
    );
    result.parse().unwrap_or_else(|e| {
        panic!("#[output] failed to parse output token stream: {}", e)
    })
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

// ── Generic type-param substitution ──────────────────────────────────────────
//
// Replace every unbound type parameter (e.g. `T`) in a `syn::Type` with `()`.
// Used so the `#[blueprint]` macro can call `size_of::<SubstitutedType>()` in
// a const context to record the *wrapper* size at compile time:
//
//   Vec<T>  →  Vec<()>   →  size_of = 24  (wrapper size, T-independent)
//   T       →  ()        →  size_of = 0   (signal: resolve via graph traversal)
//
// No lookup table, no hardcoded constants — the Rust compiler does the work.

fn substitute_generics_with_unit(
    ty: &syn::Type,
    params: &std::collections::HashSet<String>,
) -> syn::Type {
    use syn::{GenericArgument, PathArguments, Type};
    match ty {
        Type::Path(type_path) => {
            // Bare generic parameter with no arguments (e.g. just `T`)?
            if type_path.qself.is_none() && type_path.path.segments.len() == 1 {
                let seg = &type_path.path.segments[0];
                if matches!(seg.arguments, PathArguments::None)
                    && params.contains(&seg.ident.to_string())
                {
                    return syn::parse_quote!(());
                }
            }
            // Recurse into angle-bracketed generic arguments (e.g. `Vec<T>`).
            let mut new_tp = type_path.clone();
            for seg in new_tp.path.segments.iter_mut() {
                if let PathArguments::AngleBracketed(ref mut ab) = seg.arguments {
                    let mut new_args = syn::punctuated::Punctuated::new();
                    for arg in ab.args.iter() {
                        let new_arg = if let GenericArgument::Type(inner) = arg {
                            GenericArgument::Type(substitute_generics_with_unit(inner, params))
                        } else {
                            arg.clone()
                        };
                        new_args.push(new_arg);
                    }
                    ab.args = new_args;
                }
            }
            Type::Path(new_tp)
        }
        Type::Tuple(tup) => {
            let mut new_tup = tup.clone();
            let mut new_elems = syn::punctuated::Punctuated::new();
            for elem in tup.elems.iter() {
                new_elems.push(substitute_generics_with_unit(elem, params));
            }
            new_tup.elems = new_elems;
            Type::Tuple(new_tup)
        }
        // References and other compound types: recurse where possible, else clone.
        Type::Reference(r) => {
            let mut new_r = r.clone();
            new_r.elem = Box::new(substitute_generics_with_unit(&r.elem, params));
            Type::Reference(new_r)
        }
        other => other.clone(),
    }
}

// ── Concrete-type substitution for generic shim codegen ──────────────────────
//
// Replace every occurrence of a named generic type parameter in a `syn::Type`
// with a caller-supplied concrete type.  Used to generate the typed arms of the
// size-dispatch shim for generic functions.

fn substitute_generics_with_concrete_type(
    ty: &syn::Type,
    type_map: &std::collections::HashMap<String, syn::Type>,
) -> syn::Type {
    use syn::{GenericArgument, PathArguments, Type};
    match ty {
        Type::Path(type_path) => {
            if type_path.qself.is_none() && type_path.path.segments.len() == 1 {
                let seg = &type_path.path.segments[0];
                if matches!(seg.arguments, PathArguments::None) {
                    if let Some(concrete) = type_map.get(&seg.ident.to_string()) {
                        return concrete.clone();
                    }
                }
            }
            let mut new_tp = type_path.clone();
            for seg in new_tp.path.segments.iter_mut() {
                if let PathArguments::AngleBracketed(ref mut ab) = seg.arguments {
                    let mut new_args = syn::punctuated::Punctuated::new();
                    for arg in ab.args.iter() {
                        let new_arg = if let GenericArgument::Type(inner) = arg {
                            GenericArgument::Type(substitute_generics_with_concrete_type(
                                inner, type_map,
                            ))
                        } else {
                            arg.clone()
                        };
                        new_args.push(new_arg);
                    }
                    ab.args = new_args;
                }
            }
            Type::Path(new_tp)
        }
        Type::Tuple(tup) => {
            let mut new_tup = tup.clone();
            let mut new_elems = syn::punctuated::Punctuated::new();
            for elem in tup.elems.iter() {
                new_elems.push(substitute_generics_with_concrete_type(elem, type_map));
            }
            new_tup.elems = new_elems;
            Type::Tuple(new_tup)
        }
        Type::Reference(r) => {
            let mut new_r = r.clone();
            new_r.elem = Box::new(substitute_generics_with_concrete_type(&r.elem, type_map));
            Type::Reference(new_r)
        }
        other => other.clone(),
    }
}

/// Returns true iff `ty` is a bare generic type-parameter name with no arguments
/// (e.g. `T`, `U`).
fn is_bare_generic_param(ty: &syn::Type, param_names: &std::collections::HashSet<String>) -> bool {
    use syn::{PathArguments, Type};
    if let Type::Path(tp) = ty {
        if tp.qself.is_none() && tp.path.segments.len() == 1 {
            let seg = &tp.path.segments[0];
            return matches!(seg.arguments, PathArguments::None)
                && param_names.contains(&seg.ident.to_string());
        }
    }
    false
}

// ── Generic type-erased shim (size-dispatch) ──────────────────────────────────
//
// For a generic function with exactly ONE type parameter T that carries no
// "semantic" bounds (PartialEq, Ord, Hash, Clone, …), the macro emits ONE
// `__bp_dispatch_<name>` symbol that dispatches on `type_slots[0].size` at
// runtime.  Each arm substitutes T → `[u8; N]` and calls the original function
// through a concrete monomorphization.  This avoids per-type-name symbols while
// remaining correct for purely structural operations (Vec<T> push/pop/len/…).
//
// If the function has no bare-T parameters (all sizes compile-time known, e.g.
// `fn array_len<T>(v: Vec<T>) -> usize`), we use T=() for every call: Vec<()>
// shares the same header layout as Vec<T> for any T, so structural queries are
// always correct.

fn generate_generic_dispatch_shim(
    func: &ItemFn,
    fn_name: &proc_macro2::Ident,
    fn_name_str: &str,
) -> proc_macro2::TokenStream {
    // ── Guard: exactly one type param, no semantic bounds ─────────────────────
    let type_params: Vec<&syn::TypeParam> = func
        .sig
        .generics
        .params
        .iter()
        .filter_map(|p| {
            if let syn::GenericParam::Type(tp) = p {
                Some(tp)
            } else {
                None
            }
        })
        .collect();

    if type_params.len() != 1 {
        return quote! {};
    }
    let tp = type_params[0];

    // Reject bounds that require T's methods that [u8; N] doesn't implement
    // (formatting, hashing, user-defined traits, etc.).
    // We allow:
    //   - auto-traits with no vtable: Sized, Send, Sync
    //   - derivable primitive traits that [u8; N] implements for all N:
    //     Clone, Copy, PartialEq, Eq, PartialOrd, Ord
    let has_semantic_bounds = tp.bounds.iter().any(|b| {
        if let syn::TypeParamBound::Trait(tb) = b {
            let name = tb
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            !matches!(
                name.as_str(),
                "Sized"
                    | "Send"
                    | "Sync"
                    | "Clone"
                    | "Copy"
                    | "PartialEq"
                    | "Eq"
                    | "PartialOrd"
                    | "Ord"
            )
        } else {
            false // lifetime bounds are fine
        }
    });
    if has_semantic_bounds {
        return quote! {};
    }

    let generic_param_name = tp.ident.to_string();
    let mut param_names_set = std::collections::HashSet::new();
    param_names_set.insert(generic_param_name.clone());

    // Collect typed params: (arg_index, ident, original_type).
    let mut params: Vec<(usize, proc_macro2::Ident, syn::Type)> = Vec::new();
    for (i, arg) in func.sig.inputs.iter().enumerate() {
        if let FnArg::Typed(pt) = arg {
            if let Pat::Ident(pi) = &*pt.pat {
                params.push((i, pi.ident.clone(), (*pt.ty).clone()));
            }
        }
    }

    let shim_ident =
        proc_macro2::Ident::new(&format!("__bp_dispatch_{}", fn_name_str), fn_name.span());

    // ── Check whether any param / return type is a bare T ─────────────────────
    let has_bare_param = params
        .iter()
        .any(|(_, _, ty)| is_bare_generic_param(ty, &param_names_set));
    let has_bare_return = match &func.sig.output {
        ReturnType::Type(_, ty) => is_bare_generic_param(ty, &param_names_set),
        ReturnType::Default => false,
    };

    // ── No bare T anywhere: use T=() for everything ────────────────────────────
    if !has_bare_param && !has_bare_return {
        let unit_map = {
            let unit_ty: syn::Type = syn::parse_quote!(());
            let mut m = std::collections::HashMap::new();
            m.insert(generic_param_name.clone(), unit_ty);
            m
        };

        let mut let_stmts: Vec<proc_macro2::TokenStream> = Vec::new();
        let mut call_args: Vec<proc_macro2::TokenStream> = Vec::new();
        for (arg_idx, _, orig_ty) in &params {
            let arg_var =
                proc_macro2::Ident::new(&format!("__a{}", arg_idx), proc_macro2::Span::call_site());
            let idx_lit = proc_macro2::Literal::usize_unsuffixed(*arg_idx);
            let concrete_ty = substitute_generics_with_concrete_type(orig_ty, &unit_map);
            let_stmts.push(quote! {
                let #arg_var = ::std::ptr::read(*args.add(#idx_lit) as *const #concrete_ty);
            });
            call_args.push(quote! { #arg_var });
        }

        let call_expr = quote! { #fn_name(#(#call_args),*) };
        let write_stmt = match &func.sig.output {
            ReturnType::Default => quote! { #call_expr; },
            ReturnType::Type(_, ret_ty) => {
                let ty_s = quote!(#ret_ty).to_string();
                if ty_s.trim() == "()" || ty_s.trim() == "!" {
                    quote! { #call_expr; }
                } else {
                    let concrete_ret = substitute_generics_with_concrete_type(ret_ty, &unit_map);
                    quote! {
                        let __r = #call_expr;
                        ::std::ptr::write(ret as *mut #concrete_ret, __r);
                    }
                }
            }
        };

        return quote! {
            #[cfg(not(target_arch = "wasm32"))]
            #[no_mangle]
            pub unsafe extern "C" fn #shim_ident(
                args:        *const *const u8,
                ret:         *mut u8,
                _type_slots: *const u8,  // not needed — no bare T params
            ) {
                #(#let_stmts)*
                #write_stmt
            }
        };
    }

    // ── Has bare T: generate size-dispatch on type_slots[0].size ──────────────
    //
    // Arm sizes cover all common blueprint element types.
    // Each arm substitutes T → [u8; N] (or () for N=0) and calls the original
    // function with those concrete types.  Because [u8; N] has the same size and
    // alignment as any N-byte T, structural operations (push, pop, etc.) produce
    // identical machine code.

    let dispatch_sizes: &[(&str, &str)] = &[
        ("0", "()"),
        ("1", "[u8; 1]"),
        ("2", "[u8; 2]"),
        ("4", "[u8; 4]"),
        ("8", "[u8; 8]"),
        ("12", "[u8; 12]"),
        ("16", "[u8; 16]"),
        ("24", "[u8; 24]"),
        ("32", "[u8; 32]"),
    ];

    let mut match_arms: Vec<proc_macro2::TokenStream> = Vec::new();

    for &(size_str, ty_str) in dispatch_sizes {
        let concrete_ty: syn::Type = syn::parse_str(ty_str)
            .unwrap_or_else(|_| panic!("internal: bad type str '{}'", ty_str));
        let size_lit: proc_macro2::TokenStream = size_str.parse().expect("bad size literal");

        let mut type_map = std::collections::HashMap::new();
        type_map.insert(generic_param_name.clone(), concrete_ty.clone());

        let mut let_stmts: Vec<proc_macro2::TokenStream> = Vec::new();
        let mut call_args: Vec<proc_macro2::TokenStream> = Vec::new();

        for (arg_idx, _, orig_ty) in &params {
            let arg_var =
                proc_macro2::Ident::new(&format!("__a{}", arg_idx), proc_macro2::Span::call_site());
            let idx_lit = proc_macro2::Literal::usize_unsuffixed(*arg_idx);
            let concrete_arg_ty = substitute_generics_with_concrete_type(orig_ty, &type_map);

            let_stmts.push(quote! {
                let #arg_var = ::std::ptr::read(*args.add(#idx_lit) as *const #concrete_arg_ty);
            });
            call_args.push(quote! { #arg_var });
        }

        let call_expr = quote! { #fn_name(#(#call_args),*) };
        let write_stmt = match &func.sig.output {
            ReturnType::Default => quote! { #call_expr; },
            ReturnType::Type(_, ret_ty) => {
                let ty_s = quote!(#ret_ty).to_string();
                if ty_s.trim() == "()" || ty_s.trim() == "!" {
                    quote! { #call_expr; }
                } else {
                    let concrete_ret = substitute_generics_with_concrete_type(ret_ty, &type_map);
                    quote! {
                        let __r = #call_expr;
                        ::std::ptr::write(ret as *mut #concrete_ret, __r);
                    }
                }
            }
        };

        match_arms.push(quote! {
            #size_lit => { #(#let_stmts)* #write_stmt }
        });
    }

    let panic_msg = format!(
        "__bp_dispatch_{}: unsupported T size {{}} align {{}}",
        fn_name_str
    );
    match_arms.push(quote! {
        __n => panic!(#panic_msg, __ts0.size, __ts0.align),
    });

    quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #[no_mangle]
        pub unsafe extern "C" fn #shim_ident(
            args:       *const *const u8,
            ret:        *mut u8,
            type_slots: *const crate::TypeSlot,
        ) {
            let __ts0 = *type_slots.add(0);
            match __ts0.size {
                #(#match_arms)*
            }
        }
    }
}

// ── Native dispatch symbol generation ────────────────────────────────────────
//
// Each `#[blueprint]` function gets a `__bp_dispatch_<name>` symbol (native only).
//
// ABI:  unsafe extern "C" fn(args: *const *const u8, ret: *mut u8, type_slots: *const TypeSlot)
//   - args[i]:     pointer into the byte arena at the i-th input's arena offset
//   - ret:         pointer into the byte arena at the output's arena offset
//                  (null / ignored for void-returning functions)
//   - type_slots:  array of TypeSlot values resolved at graph-compile time, one
//                  per generic type parameter T.  Concrete functions ignore this.
//
// The symbol reads each argument directly from the arena via ptr::read,
// calls the actual function, and writes the result back via ptr::write.
// There is exactly ONE type boundary — here — and nowhere else in the runtime.

fn generate_dispatch_shim(
    func: &ItemFn,
    fn_name: &proc_macro2::Ident,
    fn_name_str: &str,
) -> proc_macro2::TokenStream {
    // Generic functions — route to the type-erased size-dispatch shim generator.
    if !func.sig.generics.params.is_empty() {
        return generate_generic_dispatch_shim(func, fn_name, fn_name_str);
    }

    // Collect (ident, syn::Type) for all typed params
    let mut params: Vec<(proc_macro2::Ident, syn::Type)> = Vec::new();
    for arg in &func.sig.inputs {
        if let FnArg::Typed(pt) = arg {
            if let Pat::Ident(ident) = &*pt.pat {
                params.push((ident.ident.clone(), (*pt.ty).clone()));
            }
        }
    }

    // Build ptr::read expressions — one per argument
    let reads: Vec<proc_macro2::TokenStream> = params
        .iter()
        .enumerate()
        .map(|(i, (_, ty))| {
            let idx = proc_macro2::Literal::usize_unsuffixed(i);
            quote! { ::std::ptr::read(*args.add(#idx) as *const #ty) }
        })
        .collect();

    let call_expr = quote! { #fn_name(#(#reads),*) };

    // Write result — omit ptr::write entirely for void and diverging functions
    let body = match &func.sig.output {
        ReturnType::Default => quote! { #call_expr; },
        ReturnType::Type(_, ty) => {
            let ty_str = quote::quote!(#ty).to_string();
            let ty_trimmed = ty_str.trim();
            if ty_trimmed == "()" || ty_trimmed == "!" {
                quote! { #call_expr; }
            } else {
                quote! {
                    let __result = #call_expr;
                    ::std::ptr::write(ret as *mut #ty, __result);
                }
            }
        }
    };

    let shim_ident =
        proc_macro2::Ident::new(&format!("__bp_dispatch_{}", fn_name_str), fn_name.span());

    quote! {
        #[cfg(not(target_arch = "wasm32"))]
        #[no_mangle]
        pub unsafe extern "C" fn #shim_ident(
            args:        *const *const u8,
            ret:         *mut u8,
            _type_slots: *const u8,  // TypeSlot array — ignored by concrete functions
        ) {
            #body
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
