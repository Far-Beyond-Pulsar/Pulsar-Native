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

    // Extract output data pins
    let outputs_str = extract_string_value(&args_str, "outputs");
    let output_params: Vec<proc_macro2::TokenStream> = if let Some(out_str) = outputs_str {
        out_str
            .split(',')
            .filter_map(|pair| {
                let pair = pair.trim();
                if pair.is_empty() {
                    return None;
                }
                let parts: Vec<&str> = pair.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let name = parts[0].trim();
                    let ty = parts[1].trim();
                    Some(quote! {
                        crate::NodeParameter {
                            name: #name,
                            ty: #ty,
                            size: 0,
                            align: 0,
                            type_info_fn: None,
                        }
                    })
                } else {
                    None
                }
            })
            .collect()
    } else {
        Vec::new()
    };
    let output_params_array = if output_params.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#output_params),*] }
    };

    // Extract conversion metadata
    let conversion_str = extract_string_value(&args_str, "conversion");
    let conversion_expr = if let Some(conv_str) = conversion_str {
        let parts: Vec<&str> = conv_str.splitn(2, "->").collect();
        if parts.len() == 2 {
            let from_type = parts[0].trim();
            let to_type = parts[1].trim();
            // Split on comma for optional lossless flag
            let (to_type_clean, lossless) = if let Some(comma_pos) = to_type.find(',') {
                let typ = to_type[..comma_pos].trim();
                let rest = to_type[comma_pos + 1..].trim();
                let ll = rest == "lossless" || rest == "true";
                (typ, ll)
            } else {
                (to_type, true)
            };
            quote! {
                Some(crate::registry::ConversionMetadata {
                    from_type: #from_type,
                    to_type: #to_type_clean,
                    lossless: #lossless,
                })
            }
        } else {
            quote! { None }
        }
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

    let capability = native_capability(&args_str, &category_str);
    let script_native = script_native_registration(
        &input,
        &fn_name_str,
        node_type_str,
        &category_str,
        &docs.join("\n"),
        args_str.contains("wasm_safe : false") || args_str.contains("wasm_safe:false"),
        capability.as_deref(),
    );

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

    // native_only: true (via wasm_safe: false) — node uses OS/threading APIs unavailable
    // in a cdylib context; wrap definition to exclude from those builds.
    let native_only =
        args_str.contains("wasm_safe : false") || args_str.contains("wasm_safe:false");

    let registry_ident = syn::Ident::new(
        &format!("__BLUEPRINT_NODE__{}", fn_name_str.to_uppercase()),
        fn_name.span(),
    );
    let node_type_ident = syn::Ident::new(node_type_str, fn_name.span());

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
            output_params: #output_params_array,
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
            conversion: #conversion_expr,
        };
    };

    let expanded = quote! {
        #fn_definition
        #registry_registration
        #script_native
    };

    TokenStream::from(expanded)
}

/// Types a `#[blueprint]` function may use (by value) to also become a
/// script VM native. Anything else keeps the node Blueprint-only.
const SCRIPT_NATIVE_TYPES: &[&str] = &[
    "bool", "i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "usize", "f32", "f64",
    "String",
];

fn is_script_native_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(path) => path
            .path
            .get_ident()
            .is_some_and(|ident| SCRIPT_NATIVE_TYPES.contains(&ident.to_string().as_str())),
        syn::Type::Tuple(tuple) => tuple.elems.is_empty(),
        _ => false,
    }
}

/// Rewrites `exec_output!("Label")` into recording which output fired, and
/// notes whether any sits inside a loop (such a node may fire repeatedly
/// between which the graph must run, so it cannot be a selector).
struct ExecOutputRewriter {
    labels: Vec<String>,
    loop_depth: usize,
    in_loop: bool,
}

impl ExecOutputRewriter {
    fn replacement(&mut self, mac: &syn::Macro) -> Option<syn::Expr> {
        if !mac.path.is_ident("exec_output") {
            return None;
        }
        let label = syn::parse2::<syn::LitStr>(mac.tokens.clone()).ok()?.value();
        if self.loop_depth > 0 {
            self.in_loop = true;
        }
        let index = match self.labels.iter().position(|l| *l == label) {
            Some(i) => i,
            None => {
                self.labels.push(label);
                self.labels.len() - 1
            }
        } as i64;
        Some(syn::parse_quote!({
            *__bp_fired = #index;
            *__bp_fire_count += 1;
        }))
    }
}

impl syn::visit_mut::VisitMut for ExecOutputRewriter {
    fn visit_expr_mut(&mut self, expr: &mut syn::Expr) {
        let is_loop = matches!(expr, syn::Expr::ForLoop(_) | syn::Expr::While(_) | syn::Expr::Loop(_));
        if let syn::Expr::Macro(m) = expr {
            if let Some(replacement) = self.replacement(&m.mac) {
                *expr = replacement;
                return;
            }
        }
        if is_loop {
            self.loop_depth += 1;
        }
        syn::visit_mut::visit_expr_mut(self, expr);
        if is_loop {
            self.loop_depth -= 1;
        }
    }

    fn visit_stmt_mut(&mut self, stmt: &mut Stmt) {
        if let Stmt::Macro(m) = stmt {
            if let Some(replacement) = self.replacement(&m.mac) {
                *stmt = Stmt::Expr(replacement, Some(Default::default()));
                return;
            }
        }
        syn::visit_mut::visit_stmt_mut(self, stmt);
    }
}

/// A control-flow node as a script VM *selector* native: its own body runs
/// with each `exec_output!` recording which exec output fired, and the
/// native returns that output's index (-1: none) for the compiler to jump
/// to. A node that returns a value passes it out through a trailing
/// `inout result` parameter. The labels, in index order, are the native's
/// `exec_outputs` attribute. Nodes whose `exec_output!` sits in a loop (or
/// with unrepresentable types) get no selector; the compiler implements
/// the built-in loops itself.
fn control_flow_selector(
    input: &ItemFn,
    name: &str,
    category: &str,
    doc: &str,
    native_only: bool,
    capability: Option<&str>,
) -> proc_macro2::TokenStream {
    use syn::visit_mut::VisitMut;

    let mut body = (*input.block).clone();
    let mut rewriter = ExecOutputRewriter { labels: Vec::new(), loop_depth: 0, in_loop: false };
    rewriter.visit_block_mut(&mut body);
    if rewriter.in_loop || rewriter.labels.is_empty() {
        return quote! {};
    }

    let mut params = Vec::new();
    let mut fn_params = Vec::new();
    let mut sig_params = Vec::new();
    let mut extracts = Vec::new();
    for (index, arg) in input.sig.inputs.iter().enumerate() {
        let FnArg::Typed(typed) = arg else { return quote! {} };
        let Pat::Ident(ident) = &*typed.pat else { return quote! {} };
        let pat = &ident.ident;
        let (slot_ty, pass): (syn::Type, proc_macro2::TokenStream) = if is_str_ref(&typed.ty) {
            (syn::parse_quote!(::std::string::String), quote! { &__bp_arg })
        } else if is_script_native_type(&typed.ty) {
            ((*typed.ty).clone(), quote! { __bp_arg })
        } else {
            return quote! {};
        };
        let ty = &typed.ty;
        fn_params.push(quote! { #pat: #ty });
        sig_params.push(quote! {
            ::pulsar_script_vm::Param::new(<#slot_ty as ::pulsar_script_vm::ScriptValue>::script_type())
        });
        extracts.push(quote! {{
            let __bp_arg = <#slot_ty as ::pulsar_script_vm::ScriptValue>::from_value(&args[#index])
                .ok_or_else(|| ::pulsar_script_vm::ScriptError::native(concat!("bad argument ", #index)))?;
            #pass
        }});
        params.push(ident.ident.to_string().trim_start_matches('_').to_string());
    }
    let ret_ty: syn::Type = match &input.sig.output {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, ty) if is_script_native_type(ty) => (**ty).clone(),
        ReturnType::Type(..) => return quote! {},
    };
    let has_result = !matches!(&ret_ty, syn::Type::Tuple(t) if t.elems.is_empty());
    let result_index = params.len();
    if has_result {
        params.push("result".to_string());
        sig_params.push(quote! {
            ::pulsar_script_vm::Param::inout(<#ret_ty as ::pulsar_script_vm::ScriptValue>::script_type())
        });
    }
    if params.len() > 8 {
        return quote! {};
    }
    let store_result = if has_result {
        quote! { args[#result_index] = ::pulsar_script_vm::ScriptValue::into_value(__bp_ret); }
    } else {
        quote! { let _ = __bp_ret; }
    };
    let labels = rewriter.labels.join(",");
    let capability = capability_call(capability);
    let native_name = format!("std::{name}");
    let selector = quote::format_ident!("__bp_select_{}", input.sig.ident);
    let cfg = if native_only {
        quote! { #[cfg(all(feature = "script-natives", not(target_arch = "wasm32")))] }
    } else {
        quote! { #[cfg(feature = "script-natives")] }
    };
    quote! {
        #cfg
        #[doc(hidden)]
        #[allow(non_snake_case, unused_mut, unused_variables, unreachable_code, clippy::needless_return)]
        fn #selector(__bp_fired: &mut i64, __bp_fire_count: &mut u32, #(#fn_params),*) -> #ret_ty {
            #body
        }

        #cfg
        ::pulsar_script_vm::__private::inventory::submit! {
            ::pulsar_script_vm::NativeRegistration {
                build: || ::pulsar_script_vm::NativeFn::builder(#native_name)
                    .doc(#doc)
                    .attr("category", #category)
                    #capability
                    .attr("exec_outputs", #labels)
                    .params::<&str>([#(#params),*])
                    .build_raw(
                        ::pulsar_script_vm::Signature::new(
                            [#(#sig_params),*],
                            ::pulsar_script_vm::Type::Int,
                        ),
                        ::std::boxed::Box::new(|_host, args| {
                            let mut __bp_fired: i64 = -1;
                            let mut __bp_fire_count: u32 = 0;
                            let __bp_ret = #selector(&mut __bp_fired, &mut __bp_fire_count, #(#extracts),*);
                            if __bp_fire_count > 1 {
                                return ::std::result::Result::Err(::pulsar_script_vm::ScriptError::native(
                                    "fired more than one exec output in one call",
                                ));
                            }
                            #store_result
                            ::std::result::Result::Ok(::pulsar_script_vm::Value::Int(__bp_fired))
                        }),
                    ),
            }
        }
    }
}

fn is_str_ref(ty: &syn::Type) -> bool {
    matches!(ty, syn::Type::Reference(r) if r.mutability.is_none()
        && matches!(&*r.elem, syn::Type::Path(p) if p.path.is_ident("str")))
}

/// Register a pure or plain function node as the script VM native
/// `std::<name>` (feature `script-natives`), when its signature is
/// representable: non-generic, at most six parameters, every parameter a
/// scalar, `String` or `&str`, and the return type a scalar or `String`. Control-flow and event nodes are
/// compiler intrinsics, not natives.
fn script_native_registration(
    input: &ItemFn,
    name: &str,
    node_type: &str,
    category: &str,
    doc: &str,
    native_only: bool,
    capability: Option<&str>,
) -> proc_macro2::TokenStream {
    if node_type == "control_flow" && input.sig.generics.params.is_empty() {
        return control_flow_selector(input, name, category, doc, native_only, capability);
    }
    let capability = capability_call(capability);
    if !matches!(node_type, "pure" | "fn_") || !input.sig.generics.params.is_empty() {
        return quote! {};
    }
    let mut params = Vec::new();
    // Closure parameters and the call's arguments: `&str` parameters take a
    // `String` and pass a borrow.
    let mut closure_params = Vec::new();
    let mut call_args = Vec::new();
    for (index, arg) in input.sig.inputs.iter().enumerate() {
        let FnArg::Typed(typed) = arg else { return quote! {} };
        let Pat::Ident(ident) = &*typed.pat else { return quote! {} };
        let arg_ident = quote::format_ident!("a{index}");
        if is_str_ref(&typed.ty) {
            closure_params.push(quote! { #arg_ident: ::std::string::String });
            call_args.push(quote! { &#arg_ident });
        } else if is_script_native_type(&typed.ty) {
            let ty = &typed.ty;
            closure_params.push(quote! { #arg_ident: #ty });
            call_args.push(quote! { #arg_ident });
        } else {
            return quote! {};
        }
        params.push(ident.ident.to_string().trim_start_matches('_').to_string());
    }
    if params.len() > 6 {
        return quote! {};
    }
    if let ReturnType::Type(_, ty) = &input.sig.output {
        if !is_script_native_type(ty) {
            return quote! {};
        }
    }
    let fn_ident = &input.sig.ident;
    let native_name = format!("std::{name}");
    let pure = if node_type == "pure" { quote! { .side_effect_free() } } else { quote! {} };
    let cfg = if native_only {
        quote! { #[cfg(all(feature = "script-natives", not(target_arch = "wasm32")))] }
    } else {
        quote! { #[cfg(feature = "script-natives")] }
    };
    quote! {
        #cfg
        ::pulsar_script_vm::__private::inventory::submit! {
            ::pulsar_script_vm::NativeRegistration {
                build: || ::pulsar_script_vm::NativeFn::builder(#native_name)
                    .doc(#doc)
                    .attr("category", #category)
                    #capability
                    .params::<&str>([#(#params),*])
                    #pure
                    .build(|#(#closure_params),*| #fn_ident(#(#call_args),*)),
            }
        }
    }
}

/// The capability (#869) a node's script native needs: an explicit
/// `capability: "..."` argument (empty for none), otherwise one implied by
/// the category (file IO, processes and shells, networking, environment).
fn native_capability(args: &str, category: &str) -> Option<String> {
    if let Some(explicit) = extract_string_value(args, "capability") {
        return (!explicit.is_empty()).then_some(explicit);
    }
    let implied = match category {
        "File I/O" => "fs",
        "Process" | "Shell" => "process",
        "HTTP" | "Network" => "net",
        "Env" => "env",
        _ => return None,
    };
    Some(implied.to_string())
}

fn capability_call(capability: Option<&str>) -> proc_macro2::TokenStream {
    match capability {
        Some(capability) => quote! { .capability(#capability) },
        None => quote! {},
    }
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
