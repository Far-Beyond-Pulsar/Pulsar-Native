//! `#[blueprint]`: registers a function as a Blueprint node and, where it can, a script native.

use crate::attrs::{extract_bp_imports, extract_string_value};
use crate::exec_pins::find_exec_output_labels;
use crate::generics::substitute_generics_with_unit;
use crate::script_native::{native_capability, script_native_registration};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use std::collections::HashSet;
use syn::{parse_macro_input, FnArg, ItemFn, Pat, ReturnType};

/// The unbound type parameters of the node function, which size and alignment
/// are computed around.
///
/// For a generic function each unbound type parameter is substituted with `()`
/// before `size_of`/`align_of` are baked in:
///   - bare `T`   gives `size_of::<()>()` = 0 (signal: resolve this slot via graph traversal)
///   - `Vec<T>`   gives `size_of::<Vec<()>>()` = 24 (wrapper size, fixed regardless of T)
///   - `Arc<T>`   gives `size_of::<Arc<()>>()` = 8
///
/// No lookup table is needed: the Rust compiler evaluates everything at
/// compile time.
struct GenericContext {
    is_generic: bool,
    param_names: HashSet<String>,
}

impl GenericContext {
    fn of(input: &ItemFn) -> Self {
        let param_names = input
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
        Self {
            is_generic: !input.sig.generics.params.is_empty(),
            param_names,
        }
    }
}

/// The baked-in return type fields of a node's metadata.
struct ReturnTokens {
    return_type: TokenStream2,
    size: TokenStream2,
    align: TokenStream2,
    type_info_fn: TokenStream2,
}

/// Everything the `NodeMetadata` registration static is built from.
struct NodeParts {
    node_type: &'static str,
    params: Vec<TokenStream2>,
    output_params_array: TokenStream2,
    returns: ReturnTokens,
    exec_inputs: TokenStream2,
    exec_outputs_array: TokenStream2,
    fn_source: String,
    docs_array: TokenStream2,
    category: String,
    color_opt: TokenStream2,
    imports_array: TokenStream2,
    conversion_expr: TokenStream2,
    native_only: bool,
}

pub(crate) fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as ItemFn);
    let args_str = args.to_string();

    let fn_name = &input.sig.ident;
    let fn_name_str = fn_name.to_string();

    let node_type_str = node_type_name(&args_str);
    let category_str =
        extract_string_value(&args_str, "category").unwrap_or_else(|| "General".to_string());
    let color_opt = if let Some(c) = extract_string_value(&args_str, "color") {
        quote! { Some(#c) }
    } else {
        quote! { None }
    };

    let generics = GenericContext::of(&input);
    let params = node_params(&input, &generics);
    let returns = return_tokens(&input, &generics);

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

    let docs = doc_lines(&input);

    // An intrinsic is implemented by the Blueprint compiler, never run: no
    // script native is registered for it (see the `intrinsic` attribute).
    // `args_str` is a pretty-printed token stream whose spacing and line breaks
    // vary, so compare it without whitespace.
    let intrinsic = args_str
        .split_whitespace()
        .collect::<String>()
        .contains("intrinsic:true");
    let script_native = if intrinsic {
        quote! {}
    } else {
        script_native_tokens(&input, &args_str, node_type_str, &category_str, &docs)
    };

    // Create a clean function without macro attributes for source code display
    let mut clean_input = input.clone();
    clean_input.attrs.retain(|attr| attr.path().is_ident("doc"));
    clean_input.attrs.clear(); // Remove all attributes including doc comments
    let fn_source = quote!(#clean_input).to_string();

    let final_docs = docs_with_source(&docs, &fn_source);
    let docs_array = quote! { &[#(#final_docs),*] };

    let imports = extract_bp_imports(&input);
    let imports_array = if imports.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#imports),*] }
    };

    // native_only: true (via wasm_safe: false) — node uses OS/threading APIs unavailable
    // in a cdylib context; wrap definition to exclude from those builds.
    let native_only = is_native_only(&args_str);

    let fn_definition = fn_definition(&input, intrinsic, native_only);
    let parts = NodeParts {
        node_type: node_type_str,
        params,
        output_params_array: output_params_array(&args_str),
        returns,
        exec_inputs,
        exec_outputs_array,
        fn_source,
        docs_array,
        category: category_str,
        color_opt,
        imports_array,
        conversion_expr: conversion_tokens(&args_str),
        native_only,
    };
    let registry_registration = registry_registration(&fn_name_str, fn_name.span(), &parts);

    let expanded = quote! {
        #fn_definition
        #registry_registration
        #script_native
    };

    TokenStream::from(expanded)
}

/// The node type named in the macro arguments, `fn_` when none is.
fn node_type_name(args_str: &str) -> &'static str {
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
    }
}

/// `wasm_safe: false`: the node uses OS or threading APIs a cdylib build lacks.
fn is_native_only(args_str: &str) -> bool {
    args_str.contains("wasm_safe : false") || args_str.contains("wasm_safe:false")
}

/// The output data pins declared by `outputs = "name: type, ..."`, as the
/// `&[...]` array expression.
fn output_params_array(args_str: &str) -> TokenStream2 {
    let output_params: Vec<TokenStream2> =
        if let Some(out_str) = extract_string_value(args_str, "outputs") {
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
    if output_params.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#output_params),*] }
    }
}

/// The type conversion metadata declared by `conversion = "From -> To[, lossless]"`.
fn conversion_tokens(args_str: &str) -> TokenStream2 {
    let Some(conv_str) = extract_string_value(args_str, "conversion") else {
        return quote! { None };
    };
    let parts: Vec<&str> = conv_str.splitn(2, "->").collect();
    if parts.len() != 2 {
        return quote! { None };
    }
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
}

/// The `NodeParameter` of each named input, with `size_of`/`align_of` baked in
/// from the actual type token.
fn node_params(input: &ItemFn, generics: &GenericContext) -> Vec<TokenStream2> {
    let is_generic = generics.is_generic;
    input
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
                        let subst = substitute_generics_with_unit(ty, &generics.param_names);
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
        .collect()
}

/// The return type and its baked-in `size_of`/`align_of`.
///
/// A void (default or `()`) return has size 0, align 1, and `!` (never) is
/// treated as void. A generic function substitutes each type parameter with
/// `()` to get the wrapper size.
fn return_tokens(input: &ItemFn, generics: &GenericContext) -> ReturnTokens {
    let void = || ReturnTokens {
        return_type: quote! { None },
        size: quote! { 0usize },
        align: quote! { 1usize },
        type_info_fn: quote! { None },
    };
    match &input.sig.output {
        ReturnType::Default => void(),
        ReturnType::Type(_, ty) => {
            let ty_str = quote!(#ty).to_string();
            let ty_trimmed = ty_str.trim();
            if ty_trimmed == "()" || ty_trimmed == "!" {
                void()
            } else if generics.is_generic {
                let subst = substitute_generics_with_unit(ty, &generics.param_names);
                ReturnTokens {
                    return_type: quote! { Some(#ty_str) },
                    size: quote! { ::std::mem::size_of::<#subst>() },
                    align: quote! { ::std::mem::align_of::<#subst>() },
                    type_info_fn: quote! { None }, // Generic return types resolved at instantiation
                }
            } else {
                // Generate type_info accessor for non-generic return types
                let type_info_fn = quote! {
                    Some(|| {
                        use ::pulsar_reflection::type_traits::Reflectable as _;
                        ::pulsar_reflection::RUNTIME_TYPE_REGISTRY.get::<#ty>()
                    })
                };
                ReturnTokens {
                    return_type: quote! { Some(#ty_str) },
                    size: quote! { ::std::mem::size_of::<#ty>() },
                    align: quote! { ::std::mem::align_of::<#ty>() },
                    type_info_fn,
                }
            }
        }
    }
}

/// The function's doc comments (`///` or `#[doc = "..."]`), trimmed.
fn doc_lines(input: &ItemFn) -> Vec<String> {
    input
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
        .collect()
}

/// The script native registration for a non-intrinsic node.
fn script_native_tokens(
    input: &ItemFn,
    args_str: &str,
    node_type_str: &str,
    category_str: &str,
    docs: &[String],
) -> TokenStream2 {
    let capability = native_capability(args_str, category_str);
    let explicit_outputs: Vec<String> = extract_string_value(args_str, "outputs")
        .map(|list| {
            list.split(',')
                .filter_map(|pair| {
                    Some(pair.split(':').next()?.trim().to_owned()).filter(|n| !n.is_empty())
                })
                .collect()
        })
        .unwrap_or_default();
    script_native_registration(
        input,
        &input.sig.ident.to_string(),
        node_type_str,
        category_str,
        &docs.join("\n"),
        is_native_only(args_str),
        capability.as_deref(),
        &explicit_outputs,
    )
}

/// The documentation lines with the function's source in a code block placed
/// before the first heading, or after all the text when there is none.
fn docs_with_source(docs: &[String], fn_source: &str) -> Vec<String> {
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
        final_docs.push(fn_source.to_owned());
        final_docs.push("```".to_string());

        // Add rest of docs (from heading onwards)
        final_docs.push("".to_string()); // Empty line separator
        final_docs.extend(docs[heading_idx..].iter().cloned());
    } else {
        // No heading found, add all docs first, then source
        final_docs.extend(docs.iter().cloned());
        if !final_docs.is_empty() {
            final_docs.push("".to_string()); // Empty line separator
        }
        final_docs.push("```rust".to_string());
        final_docs.push(fn_source.to_owned());
        final_docs.push("```".to_string());
    }
    final_docs
}

/// The function as emitted. native_only nodes are wrapped so they compile out
/// in non-native (cdylib) builds. An intrinsic's authored body only declares
/// the node (its `exec_output!` markers became the exec pins); what is emitted
/// cannot run.
fn fn_definition(input: &ItemFn, intrinsic: bool, native_only: bool) -> TokenStream2 {
    let emitted = if intrinsic {
        let mut stub = input.clone();
        let message = format!(
            "`{}` is a Blueprint compiler intrinsic and has no executable body",
            input.sig.ident
        );
        stub.block = Box::new(syn::parse_quote!({ unreachable!(#message) }));
        stub
    } else {
        input.clone()
    };
    if native_only {
        quote! {
            #[cfg(not(target_arch = "wasm32"))]
            #[allow(dead_code, unused_variables)]
            #emitted
        }
    } else {
        quote! { #[allow(dead_code, unused_variables)] #emitted }
    }
}

/// The `NodeMetadata` static in the linkme registry. The registry needs linkme
/// (native feature only).
fn registry_registration(
    fn_name_str: &str,
    span: proc_macro2::Span,
    parts: &NodeParts,
) -> TokenStream2 {
    let registry_ident = syn::Ident::new(
        &format!("__BLUEPRINT_NODE__{}", fn_name_str.to_uppercase()),
        span,
    );
    let node_type_ident = syn::Ident::new(parts.node_type, span);
    let registry_cfg = if parts.native_only {
        quote! { #[cfg(all(not(target_arch = "wasm32"), feature = "native"))] }
    } else {
        quote! { #[cfg(feature = "native")] }
    };
    let NodeParts {
        params,
        output_params_array,
        returns,
        exec_inputs,
        exec_outputs_array,
        fn_source,
        docs_array,
        category,
        color_opt,
        imports_array,
        conversion_expr,
        ..
    } = parts;
    let ReturnTokens {
        return_type,
        size: return_size,
        align: return_align,
        type_info_fn: return_type_info_fn,
    } = returns;
    quote! {
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
            category: #category,
            color: #color_opt,
            imports: #imports_array,
            conversion: #conversion_expr,
        };
    }
}
