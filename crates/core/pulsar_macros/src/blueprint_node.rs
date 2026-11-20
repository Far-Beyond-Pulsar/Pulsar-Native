//! `#[blueprint]`: registers a function as a Blueprint node and, where it can, a script native.

use crate::attrs::{extract_bp_imports, extract_string_value};
use crate::exec_pins::find_exec_output_labels;
use crate::generics::substitute_generics_with_unit;
use crate::script_native::{native_capability, script_native_registration};
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn, Pat, ReturnType};

pub(crate) fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
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

    // An intrinsic is implemented by the Blueprint compiler, never run: no
    // script native is registered for it (see the `intrinsic` attribute).
    // `args_str` is a pretty-printed token stream whose spacing and line breaks
    // vary, so compare it without whitespace.
    let intrinsic = args_str
        .split_whitespace()
        .collect::<String>()
        .contains("intrinsic:true");
    let capability = native_capability(&args_str, &category_str);
    let explicit_outputs: Vec<String> = extract_string_value(&args_str, "outputs")
        .map(|list| {
            list.split(',')
                .filter_map(|pair| {
                    Some(pair.split(':').next()?.trim().to_owned()).filter(|n| !n.is_empty())
                })
                .collect()
        })
        .unwrap_or_default();
    let script_native = if intrinsic {
        quote! {}
    } else {
        script_native_registration(
            &input,
            &fn_name_str,
            node_type_str,
            &category_str,
            &docs.join("\n"),
            args_str.contains("wasm_safe : false") || args_str.contains("wasm_safe:false"),
            capability.as_deref(),
            &explicit_outputs,
        )
    };

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
    // An intrinsic's authored body only declares the node (its `exec_output!`
    // markers became the exec pins above); what is emitted cannot run.
    let emitted = if intrinsic {
        let mut stub = input.clone();
        let message =
            format!("`{fn_name_str}` is a Blueprint compiler intrinsic and has no executable body");
        stub.block = Box::new(syn::parse_quote!({ unreachable!(#message) }));
        stub
    } else {
        input.clone()
    };
    let fn_definition = if native_only {
        quote! {
            #[cfg(not(target_arch = "wasm32"))]
            #[allow(dead_code, unused_variables)]
            #emitted
        }
    } else {
        quote! { #[allow(dead_code, unused_variables)] #emitted }
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
