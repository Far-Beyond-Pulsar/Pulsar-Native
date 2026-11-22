//! The script-native registration a `#[blueprint]` function emits, and the capability it needs.

use quote::quote;
use syn::{FnArg, ItemFn, Pat, ReturnType};

use crate::attrs::extract_string_value;
use crate::exec_pins::control_flow_selector;
use crate::script_types::{generic_args, is_ident_in, is_script_native_type, native_slot};

/// Register a pure or plain function node as the script VM native
/// `std::<name>` (feature `script-natives`), when its signature is
/// representable: non-generic, at most six parameters, every parameter and
/// the result a script value (scalars, `String`, `Vec`, arrays, tuples,
/// maps and `Option` of those, `Result<_, String>` as a result; `&str`,
/// `&[T]` and other borrows as parameters). Control-flow and event nodes
/// are compiler intrinsics, not natives.
#[allow(clippy::too_many_arguments)] // one per attribute argument of the node macro
pub(crate) fn script_native_registration(
    input: &ItemFn,
    name: &str,
    node_type: &str,
    category: &str,
    doc: &str,
    native_only: bool,
    capability: Option<&str>,
    explicit_outputs: &[String],
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
        let FnArg::Typed(typed) = arg else {
            return quote! {};
        };
        let Pat::Ident(ident) = &*typed.pat else {
            return quote! {};
        };
        let arg_ident = quote::format_ident!("a{index}");
        let Some((slot_ty, borrowed)) = native_slot(&typed.ty) else {
            return quote! {};
        };
        closure_params.push(quote! { #arg_ident: #slot_ty });
        call_args.push(if borrowed {
            quote! { &#arg_ident }
        } else {
            quote! { #arg_ident }
        });
        params.push(ident.ident.to_string().trim_start_matches('_').to_string());
    }
    if params.len() > 6 {
        return quote! {};
    }
    // A `Result<T, String>` result is kept as a value (`Outcome`); every
    // other result is a script value as it stands.
    let mut wrap_outcome = false;
    let mut output_names: Vec<String> = Vec::new();
    if let ReturnType::Type(_, ty) = &input.sig.output {
        let arity = match &**ty {
            syn::Type::Tuple(tuple) => tuple.elems.len(),
            _ => 0,
        };
        if let Some([ok, err]) = generic_args(ty, "Result").as_deref() {
            if !is_script_native_type(ok) || !is_ident_in(err, &["String"]) {
                return quote! {};
            }
            wrap_outcome = true;
            output_names = ["ok", "value", "error"].map(String::from).to_vec();
        } else if !is_script_native_type(ty) {
            return quote! {};
        } else if generic_args(ty, "Option").is_some() {
            output_names = ["present", "value"].map(String::from).to_vec();
        }
        // Named output pins, when they match a tuple result element for element.
        if arity > 0 && explicit_outputs.len() == arity {
            output_names = explicit_outputs.to_vec();
        }
    }
    let outputs_attr = if output_names.is_empty() {
        quote! {}
    } else {
        let joined = output_names.join(",");
        quote! { .attr("outputs", #joined) }
    };
    let fn_ident = &input.sig.ident;
    let call = quote! { #fn_ident(#(#call_args),*) };
    let call = if wrap_outcome {
        quote! { ::pulsar_script_vm::Outcome(#call) }
    } else {
        call
    };
    let native_name = format!("std::{name}");
    let pure = if node_type == "pure" {
        quote! { .side_effect_free() }
    } else {
        quote! {}
    };
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
                    #outputs_attr
                    .build(|#(#closure_params),*| #call),
            }
        }
    }
}

/// The capability a node's script native needs: an explicit
/// `capability: "..."` argument (empty for none), otherwise one implied by
/// the category (file IO, processes and shells, networking, environment).
pub(crate) fn native_capability(args: &str, category: &str) -> Option<String> {
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

pub(crate) fn capability_call(capability: Option<&str>) -> proc_macro2::TokenStream {
    match capability {
        Some(capability) => quote! { .capability(#capability) },
        None => quote! {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_capability_overrides_category_and_empty_means_none() {
        assert_eq!(
            native_capability(r#"capability: "custom""#, "Math").as_deref(),
            Some("custom")
        );
        assert_eq!(native_capability(r#"capability: """#, "File I/O"), None);
    }

    #[test]
    fn capability_is_implied_by_category() {
        for (category, expected) in [
            ("File I/O", Some("fs")),
            ("Process", Some("process")),
            ("Shell", Some("process")),
            ("HTTP", Some("net")),
            ("Network", Some("net")),
            ("Env", Some("env")),
            ("Math", None),
        ] {
            assert_eq!(
                native_capability("", category).as_deref(),
                expected,
                "{category}"
            );
        }
    }

    #[test]
    fn capability_call_emits_builder_only_when_present() {
        assert_eq!(capability_call(None).to_string(), "");
        assert_eq!(
            capability_call(Some("fs")).to_string().replace(' ', ""),
            ".capability(\"fs\")"
        );
    }
}
