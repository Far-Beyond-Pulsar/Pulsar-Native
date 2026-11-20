//! `exec_output!` handling: rewriting the markers and finding a control-flow node's exec pins.

use quote::quote;
use syn::{Expr, FnArg, ItemFn, Pat, ReturnType, Stmt};

use crate::script_native::capability_call;
use crate::script_types::{is_script_native_type, native_slot};

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
        let is_loop = matches!(
            expr,
            syn::Expr::ForLoop(_) | syn::Expr::While(_) | syn::Expr::Loop(_)
        );
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
pub(crate) fn control_flow_selector(
    input: &ItemFn,
    name: &str,
    category: &str,
    doc: &str,
    native_only: bool,
    capability: Option<&str>,
) -> proc_macro2::TokenStream {
    use syn::visit_mut::VisitMut;

    let mut body = (*input.block).clone();
    let mut rewriter = ExecOutputRewriter {
        labels: Vec::new(),
        loop_depth: 0,
        in_loop: false,
    };
    rewriter.visit_block_mut(&mut body);
    if rewriter.in_loop || rewriter.labels.is_empty() {
        return quote! {};
    }

    let mut params = Vec::new();
    let mut fn_params = Vec::new();
    let mut sig_params = Vec::new();
    let mut extracts = Vec::new();
    for (index, arg) in input.sig.inputs.iter().enumerate() {
        let FnArg::Typed(typed) = arg else {
            return quote! {};
        };
        let Pat::Ident(ident) = &*typed.pat else {
            return quote! {};
        };
        let pat = &ident.ident;
        let Some((slot_ty, borrowed)) = native_slot(&typed.ty) else {
            return quote! {};
        };
        let pass = if borrowed {
            quote! { &__bp_arg }
        } else {
            quote! { __bp_arg }
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

/// Find all exec_output!() labels in a function
pub(crate) fn find_exec_output_labels(func: &ItemFn) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::visit_mut::VisitMut;

    fn func(src: &str) -> ItemFn {
        syn::parse_str(src).unwrap()
    }

    fn rewrite(src: &str) -> (ExecOutputRewriter, ItemFn) {
        let mut f = func(src);
        let mut rewriter = ExecOutputRewriter {
            labels: Vec::new(),
            loop_depth: 0,
            in_loop: false,
        };
        rewriter.visit_item_fn_mut(&mut f);
        (rewriter, f)
    }

    #[test]
    fn exec_output_labels_are_found_in_nested_blocks_deduplicated_in_order() {
        let f = func(
            r#"fn branch(c: bool) {
                if c { exec_output!("True"); } else { exec_output!("False"); }
                match 1 { 0 => { exec_output!("Zero"); } _ => { exec_output!("True"); } }
                for _ in 0..2 { exec_output!("Body"); }
            }"#,
        );
        assert_eq!(
            find_exec_output_labels(&f),
            vec!["True", "False", "Zero", "Body"]
        );
    }

    #[test]
    fn function_without_exec_outputs_has_no_labels() {
        let f = func("fn add(a: i32, b: i32) -> i32 { a + b }");
        assert!(find_exec_output_labels(&f).is_empty());
    }

    #[test]
    fn rewriter_assigns_stable_indices_per_label_and_removes_the_macro() {
        let (rewriter, f) = rewrite(
            r#"fn node(c: bool) {
                if c { exec_output!("A"); } else { exec_output!("B"); }
                exec_output!("A");
            }"#,
        );
        assert_eq!(rewriter.labels, vec!["A", "B"]);
        assert!(!rewriter.in_loop);
        let body = quote!(#f).to_string();
        assert!(!body.contains("exec_output"), "{body}");
        assert_eq!(body.matches("__bp_fire_count").count(), 3);
    }

    #[test]
    fn rewriter_flags_exec_output_inside_loops() {
        let (rewriter, _) = rewrite(r#"fn node() { for _ in 0..3 { exec_output!("Body"); } }"#);
        assert!(rewriter.in_loop);
        let (rewriter, _) = rewrite(r#"fn node() { loop { while true { exec_output!("X"); } } }"#);
        assert!(rewriter.in_loop);
    }

    #[test]
    fn rewriter_ignores_other_macros() {
        let (rewriter, f) = rewrite(r#"fn node() { println!("hi"); }"#);
        assert!(rewriter.labels.is_empty());
        assert!(quote!(#f).to_string().contains("println"));
    }
}
