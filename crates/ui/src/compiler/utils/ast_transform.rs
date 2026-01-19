//! # AST Utilities
//!
//! Helpers for parsing and transforming Rust AST nodes.
//!
//! This module provides utilities for working with Rust code at the AST level,
//! particularly for extracting function bodies and replacing `exec_output!()` calls
//! with actual node code during control flow inlining.

use syn::{
    visit_mut::{self, VisitMut},
    Block, Expr, ExprMacro, ItemFn, Stmt,
};
use std::collections::HashMap;

/// Replace all `exec_output!("Label")` calls in a function with code blocks
pub struct ExecOutputReplacer {
    /// Maps exec output labels to replacement code
    replacements: HashMap<String, String>,
}

impl ExecOutputReplacer {
    pub fn new(replacements: HashMap<String, String>) -> Self {
        Self { replacements }
    }

    /// Replace exec_output!() calls in a function
    pub fn replace_in_function(mut self, func: ItemFn) -> Result<ItemFn, String> {
        let mut func = func;
        self.visit_item_fn_mut(&mut func);
        Ok(func)
    }

    /// Replace exec_output!() calls in a block
    pub fn replace_in_block(mut self, block: Block) -> Result<Block, String> {
        let mut block = block;
        self.visit_block_mut(&mut block);
        Ok(block)
    }
}

impl VisitMut for ExecOutputReplacer {
    fn visit_stmt_mut(&mut self, stmt: &mut Stmt) {
        // Handle statement-level macros and expressions
        match stmt {
            Stmt::Expr(expr, _) => {
                self.visit_expr_mut(expr);
            }
            Stmt::Macro(stmt_macro) => {
                // Check if this is an exec_output! statement
                if stmt_macro.mac.path.is_ident("exec_output") {
                    if let Ok(label) = syn::parse2::<syn::LitStr>(stmt_macro.mac.tokens.clone()) {
                        let label_value = label.value();

                        // Get replacement code for this label
                        if let Some(replacement_code) = self.replacements.get(&label_value) {
                            // Parse replacement code as statements
                            if let Ok(parsed_stmts) = syn::parse_str::<syn::File>(&format!("fn dummy() {{{}}}", replacement_code)) {
                                // Extract statements from the parsed function
                                if let Some(syn::Item::Fn(item_fn)) = parsed_stmts.items.first() {
                                    // Replace the macro statement with the first replacement statement
                                    if let Some(first_stmt) = item_fn.block.stmts.first() {
                                        *stmt = first_stmt.clone();
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        visit_mut::visit_stmt_mut(self, stmt);
    }

    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // Check if this is an exec_output!() call FIRST, before recursing
        if let Expr::Macro(ExprMacro { mac, .. }) = expr {
            if mac.path.is_ident("exec_output") {
                tracing::info!("[AST_UTILS] Found exec_output!() macro");
                // Extract the label from the macro call
                if let Ok(label) = syn::parse2::<syn::LitStr>(mac.tokens.clone()) {
                    let label_value = label.value();
                    tracing::info!("[AST_UTILS] Exec output label: '{}'", label_value);

                    // Get replacement code for this label
                    if let Some(replacement_code) = self.replacements.get(&label_value) {
                        tracing::info!("[AST_UTILS] Found replacement for '{}': '{}'", label_value, replacement_code);
                        // Parse replacement code as a block
                        match syn::parse_str::<Expr>(replacement_code) {
                            Ok(replacement_expr) => {
                                *expr = replacement_expr;
                            }
                            Err(_) => {
                                // If it doesn't parse as an expression, try as a block
                                if let Ok(block) = syn::parse_str::<Block>(&format!("{{{}}}", replacement_code)) {
                                    *expr = Expr::Block(syn::ExprBlock {
                                        attrs: vec![],
                                        label: None,
                                        block,
                                    });
                                }
                                // If still fails, leave as empty block
                                else {
                                    *expr = Expr::Block(syn::ExprBlock {
                                        attrs: vec![],
                                        label: None,
                                        block: syn::parse_quote!({}),
                                    });
                                }
                            }
                        }
                    } else {
                        // No replacement provided - use empty block
                        tracing::warn!("[AST_UTILS] NO replacement found for label '{}'", label_value);
                        tracing::info!("[AST_UTILS] Available replacements: {:?}", self.replacements.keys().collect::<Vec<_>>());
                        *expr = Expr::Block(syn::ExprBlock {
                            attrs: vec![],
                            label: None,
                            block: syn::parse_quote!({}),
                        });
                    }
                }
                return; // Don't recurse after replacing
            }
        }

        // Recurse into children for non-exec_output expressions
        visit_mut::visit_expr_mut(self, expr);
    }
}

/// Replace function parameters with actual values
pub struct ParameterSubstitutor {
    /// Maps parameter names to replacement expressions
    substitutions: HashMap<String, String>,
}

impl ParameterSubstitutor {
    pub fn new(substitutions: HashMap<String, String>) -> Self {
        Self { substitutions }
    }

    /// Substitute parameters in a function body
    pub fn substitute_in_function(mut self, func: ItemFn) -> Result<ItemFn, String> {
        let mut func = func;
        self.visit_item_fn_mut(&mut func);
        Ok(func)
    }
}

impl VisitMut for ParameterSubstitutor {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // Check if this is a path referencing a parameter
        if let Expr::Path(expr_path) = expr {
            if let Some(ident) = expr_path.path.get_ident() {
                let name = ident.to_string();
                if let Some(replacement) = self.substitutions.get(&name) {
                    // Parse replacement and substitute
                    if let Ok(replacement_expr) = syn::parse_str::<Expr>(replacement) {
                        *expr = replacement_expr;
                        return; // Don't recurse into replacement
                    }
                }
            }
        }

        // Recurse into children
        visit_mut::visit_expr_mut(self, expr);
    }
}

/// Extract just the function body, removing the function wrapper
pub fn extract_function_body(func: &ItemFn) -> String {
    // Convert block to code string
    let block = &func.block;
    let code = quote::quote!(#block).to_string();

    // Remove outer braces and trim
    if code.starts_with('{') && code.ends_with('}') {
        let inner = &code[1..code.len() - 1];
        inner.trim().to_string()
    } else {
        code
    }
}

/// Format and indent code
pub fn indent_code(code: &str, indent_level: usize) -> String {
    let indent = "    ".repeat(indent_level);
    code.lines()
        .map(|line| {
            if line.trim().is_empty() {
                String::new()
            } else {
                format!("{}{}", indent, line)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Re-export format_rust_code from the generation module
pub use crate::compiler::generation::formatting::format_rust_code;

/// Apply both exec_output replacement and parameter substitution
pub fn inline_control_flow_function(
    func_source: &str,
    exec_replacements: HashMap<String, String>,
    param_substitutions: HashMap<String, String>,
) -> Result<String, String> {
    // Parse function from source string
    let func: ItemFn = syn::parse_str(func_source)
        .map_err(|e| format!("Failed to parse function: {}", e))?;

    // Step 1: Replace exec_output!() calls
    let replacer = ExecOutputReplacer::new(exec_replacements);
    let func = replacer.replace_in_function(func)?;

    // Step 2: Substitute parameters with actual values
    let substitutor = ParameterSubstitutor::new(param_substitutions);
    let func = substitutor.substitute_in_function(func)?;

    // Step 3: Extract function body
    let body = extract_function_body(&func);

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_function_body() {
        let code = r#"
            fn test() {
                let x = 5;
                tracing::info!("{}", x);
            }
        "#;

        let func: ItemFn = syn::parse_str(code).unwrap();
        let body = extract_function_body(&func);

        tracing::info!("Extracted body: {}", body);
        assert!(body.contains("let x = 5"));
        assert!(body.contains("println"), "Body should contain println macro");
        assert!(!body.starts_with("fn"));
    }

    #[test]
    fn test_exec_output_replacement() {
        let code = r#"
            fn branch(condition: bool) {
                if condition {
                    exec_output!("True");
                } else {
                    exec_output!("False");
                }
            }
        "#;

        let func: ItemFn = syn::parse_str(code).unwrap();

        let mut replacements = HashMap::new();
        replacements.insert("True".to_string(), "tracing::info!(\"true branch\");".to_string());
        replacements.insert("False".to_string(), "tracing::info!(\"false branch\");".to_string());

        let replacer = ExecOutputReplacer::new(replacements);
        let result = replacer.replace_in_function(func).unwrap();

        let code_str = quote::quote!(#result).to_string();
        assert!(code_str.contains("println"));
        assert!(!code_str.contains("exec_output"));
    }

    #[test]
    fn test_parameter_substitution() {
        let code = r#"
            fn add(a: i64, b: i64) -> i64 {
                a + b
            }
        "#;

        let func: ItemFn = syn::parse_str(code).unwrap();

        let mut substitutions = HashMap::new();
        substitutions.insert("a".to_string(), "5".to_string());
        substitutions.insert("b".to_string(), "10".to_string());

        let substitutor = ParameterSubstitutor::new(substitutions);
        let result = substitutor.substitute_in_function(func).unwrap();

        let code_str = quote::quote!(#result).to_string();
        assert!(code_str.contains("5"));
        assert!(code_str.contains("10"));
    }

    #[test]
    fn test_indent_code() {
        let code = "let x = 5;\ntracing::info!(\"{}\", x);";
        let indented = indent_code(code, 2);

        assert!(indented.starts_with("        ")); // 8 spaces (2 levels * 4 spaces)
        assert!(indented.lines().all(|line| line.is_empty() || line.starts_with("        ")));
    }
}
