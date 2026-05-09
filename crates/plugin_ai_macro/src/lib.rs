/// Procedural macros for AI tool definition in Pulsar plugins
///
/// This module provides macros that automatically:
/// - Generate AiToolDefinition from function signatures
/// - Extract parameter schemas from types
/// - Generate markdown documentation
/// - Create dispatch code in execute_ai_tool()
use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, DeriveInput, FnArg, ItemFn, Lit, Meta, Pat, ReturnType, Type};

/// Attribute macro for defining an AI tool
///
/// # Usage
///
/// ```rust,ignore
/// // With docs file embedded
/// #[ai_tool(
///     category = "refactoring",
///     timeout_ms = 5000,
///     docs = "docs/refactor_rename_node.md"
/// )]
/// /// Rename all occurrences of a node in the blueprint
/// pub fn refactor_blueprint_rename_node(
///     #[doc = "Old node name"]
///     old_name: String,
///     
///     #[doc = "New node name"]
///     new_name: String,
/// ) -> Result<serde_json::Value, plugin_editor_api::PluginError> {
///     // implementation
/// }
/// ```
///
/// # Attributes
///
/// - `category` (optional): Tool category for organization
/// - `timeout_ms` (optional): Maximum execution time in milliseconds
/// - `docs` (optional): Path to markdown file containing tool documentation
///   - When provided, docs are embedded via include_str!() at compile time
///   - Example: `docs = "src/ai_tools/docs/my_tool.md"`
///
/// # Generated Code
///
/// The macro generates:
/// 1. An AiToolDefinition in a const
/// 2. A wrapper function for JSON parameter extraction
/// 3. A markdown documentation constant (from file if `docs` provided, auto-generated otherwise)
#[proc_macro_attribute]
pub fn ai_tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attrs = parse_macro_input!(attr as syn::AttributeArgs);
    let input = parse_macro_input!(item as ItemFn);

    let fn_name = &input.sig.ident;
    let fn_visibility = &input.vis;
    let fn_asyncness = &input.sig.asyncness;
    let fn_inputs = &input.sig.inputs;
    let fn_output = &input.sig.output;
    let fn_block = &input.block;

    // Extract function documentation
    let doc_comment = extract_doc_comment(&input.attrs);
    let tool_name = fn_name.to_string();
    let tool_name_snake = to_snake_case(&tool_name);

    // Extract parameters and their documentation
    let params = extract_parameters(&input.sig.inputs);
    let param_docs = extract_param_docs(&input.attrs);

    // Parse macro attributes (now includes docs file path)
    let (category, timeout_ms, docs_path) = parse_tool_attrs(&attrs);

    // Generate JSON schema from parameters
    let schema = generate_parameter_schema(&params, &param_docs);

    // Generate the AiToolDefinition const
    let definition_name = format_ident!("TOOL_DEF_{}", tool_name.to_uppercase());

    let definition = quote! {
        #[doc(hidden)]
        pub const #definition_name: &str = stringify!({
            "name": #tool_name_snake,
            "description": #doc_comment,
            "parameters": #schema,
            "category": #category,
            "timeout_ms": #timeout_ms
        });
    };

    // Generate the wrapper function that handles tool execution
    let wrapper_fn_name = format_ident!("{}_ai_tool_wrapper", fn_name);
    let params_from_json = generate_params_from_json(&params);

    let wrapper = quote! {
        #[doc(hidden)]
        pub fn #wrapper_fn_name(
            tool_args: serde_json::Value,
        ) -> Result<serde_json::Value, plugin_editor_api::PluginError> {
            #params_from_json

            // Call the actual tool function
            let result = #fn_name(#(#params),*)?;
            Ok(result)
        }
    };

    // Generate the markdown documentation constant
    let doc_const_name = format_ident!("TOOL_DOC_{}", tool_name.to_uppercase());

    let doc_const = if let Some(path) = docs_path {
        // Use include_str! to embed external markdown file
        let path_lit = syn::LitStr::new(&path, proc_macro2::Span::call_site());
        quote! {
            #[doc(hidden)]
            pub const #doc_const_name: &str = include_str!(#path_lit);
        }
    } else {
        // Auto-generate markdown documentation
        let doc_md = generate_markdown_doc(
            &tool_name_snake,
            &doc_comment,
            &params,
            &param_docs,
            category.as_deref(),
        );
        quote! {
            #[doc(hidden)]
            pub const #doc_const_name: &str = #doc_md;
        }
    };

    // Keep original function
    let original_fn = quote! {
        #fn_visibility #fn_asyncness fn #fn_name(#fn_inputs) #fn_output {
            #fn_block
        }
    };

    let expanded = quote! {
        #definition
        #wrapper
        #doc_const
        #original_fn
    };

    TokenStream::from(expanded)
}

/// Derive macro for plugins that auto-implements EditorPlugin with ai_tools support
///
/// # Usage
///
/// ```rust,ignore
/// #[derive(AiToolProvider)]
/// pub struct MyPlugin {
///     // fields...
/// }
/// ```
#[proc_macro_derive(AiToolProvider, attributes(ai_tool))]
pub fn derive_ai_tool_provider(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // This is a placeholder - actual implementation would:
    // 1. Find all #[ai_tool] marked functions in the module
    // 2. Generate ai_tools() implementation
    // 3. Generate execute_ai_tool() match statement
    // 4. Generate capabilities_for_file() based on file types

    let expanded = quote! {
        // Placeholder - would be implemented in companion module
    };

    TokenStream::from(expanded)
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract doc comments from function attributes
fn extract_doc_comment(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter_map(|attr| {
            if attr.path().is_ident("doc") {
                if let syn::Meta::NameValue(nv) = &attr.meta {
                    if let syn::Expr::Lit(expr_lit) = &nv.value {
                        if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                            return Some(lit_str.value());
                        }
                    }
                }
            }
            None
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract parameter information
fn extract_parameters(
    inputs: &syn::punctuated::Punctuated<FnArg, syn::token::Comma>,
) -> Vec<(String, String)> {
    inputs
        .iter()
        .filter_map(|arg| {
            if let FnArg::Typed(pat_type) = arg {
                if let Pat::Ident(pat_ident) = pat_type.pat.as_ref() {
                    let param_name = pat_ident.ident.to_string();
                    let param_type = quote!(#pat_type.ty).to_string();
                    return Some((param_name, param_type));
                }
            }
            None
        })
        .collect()
}

/// Extract parameter documentation from #[doc = "..."] attributes
fn extract_param_docs(attrs: &[syn::Attribute]) -> std::collections::HashMap<String, String> {
    // Note: In actual implementation, would parse nested attribute docs
    // For now, simplified version
    std::collections::HashMap::new()
}

/// Parse tool macro attributes (category, timeout_ms, etc.)
fn parse_tool_attrs(
    attrs: &syn::punctuated::Punctuated<syn::NestedMeta, syn::token::Comma>,
) -> (Option<String>, u32, Option<String>) {
    let mut category: Option<String> = None;
    let mut timeout_ms: u32 = 5000;
    let mut docs_path: Option<String> = None;

    for nested_meta in attrs {
        if let syn::NestedMeta::Meta(syn::Meta::NameValue(nv)) = nested_meta {
            if nv.path.is_ident("category") {
                if let syn::Lit::Str(s) = &nv.lit {
                    category = Some(s.value());
                }
            } else if nv.path.is_ident("timeout_ms") {
                if let syn::Lit::Int(i) = &nv.lit {
                    if let Ok(n) = i.base10_parse::<u32>() {
                        timeout_ms = n;
                    }
                }
            } else if nv.path.is_ident("docs") {
                if let syn::Lit::Str(s) = &nv.lit {
                    docs_path = Some(s.value());
                }
            }
        }
    }

    (category, timeout_ms, docs_path)
}

/// Generate JSON schema for parameters
fn generate_parameter_schema(
    params: &[(String, String)],
    _param_docs: &std::collections::HashMap<String, String>,
) -> String {
    let properties = params
        .iter()
        .map(|(name, ty)| {
            let json_type = match ty.as_str() {
                "String" => "\"string\"",
                "i32" | "i64" | "u32" | "u64" | "isize" | "usize" => "\"integer\"",
                "f32" | "f64" => "\"number\"",
                "bool" => "\"boolean\"",
                _ => "\"object\"",
            };
            format!(r#""{}": {{"type": {}}}"#, name, json_type)
        })
        .collect::<Vec<_>>()
        .join(",");

    let required = params
        .iter()
        .map(|(n, _)| format!(r#""{}""#, n))
        .collect::<Vec<_>>()
        .join(",");

    format!(
        r#"{{"type": "object", "properties": {{{}}}, "required": [{}]}}"#,
        properties, required
    )
}

/// Generate code to extract parameters from JSON
fn generate_params_from_json(params: &[(String, String)]) -> proc_macro2::TokenStream {
    let extractions = params.iter().map(|(name, ty)| {
        let name_ident = format_ident!("{}", name);
        let ty_ident = format_ident!("{}", ty);

        quote! {
            let #name_ident: #ty_ident = serde_json::from_value(
                tool_args.get(stringify!(#name_ident))
                    .ok_or_else(|| plugin_editor_api::PluginError::Other {
                        message: format!("Missing parameter: {}", stringify!(#name_ident)),
                    })?
                    .clone()
            ).map_err(|e| plugin_editor_api::PluginError::Other {
                message: format!("Invalid parameter type for {}: {}", stringify!(#name_ident), e),
            })?;
        }
    });

    quote! {
        #(#extractions)*
    }
}

/// Generate markdown documentation for a tool
fn generate_markdown_doc(
    tool_name: &str,
    description: &str,
    params: &[(String, String)],
    _param_docs: &std::collections::HashMap<String, String>,
    category: Option<&str>,
) -> String {
    let category_str = category
        .map(|c| format!("**Category**: {}\n\n", c))
        .unwrap_or_default();

    let params_md = if params.is_empty() {
        "No parameters.".to_string()
    } else {
        let items = params
            .iter()
            .map(|(name, ty)| format!("- `{}` ({})", name, ty))
            .collect::<Vec<_>>()
            .join("\n");
        format!("### Parameters\n\n{}", items)
    };

    format!(
        "# {}\n\n{}\n\n{}{}",
        tool_name, category_str, description, params_md
    )
}

/// Convert camelCase to snake_case
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}
