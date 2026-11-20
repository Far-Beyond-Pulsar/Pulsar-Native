use super::parser::ParsedCrate;
use super::types::*;
use quote::ToTokens;
/// Documentation extractor module
///
/// Extracts documentation from parsed AST nodes. This is the core of the documentation
/// system - it walks the AST and builds comprehensive documentation structures.
use std::error::Error;
use syn::{
    Attribute, Item, ItemConst, ItemEnum, ItemFn, ItemMacro, ItemStruct, ItemTrait, ItemType,
};

/// Extract exact source code for a syntax node - uses prettyplease for formatted output
fn extract_source_from_span<T: ToTokens>(node: &T, _source_code: &str) -> String {
    // Convert to token stream first
    let tokens = quote::quote!(#node);

    // Try to parse as a complete item and format it
    // If it fails, fall back to the compact token representation
    if let Ok(mut file) = syn::parse_file(&tokens.to_string()) {
        // Remove Item::Verbatim entries — prettyplease cannot handle them
        file.items
            .retain(|item| !matches!(item, syn::Item::Verbatim(_)));
        prettyplease::unparse(&file)
    } else {
        // Not a complete file, just return formatted tokens
        // This handles expressions, types, etc that aren't complete items
        tokens.to_string()
    }
}

/// Extract documentation from a parsed crate
///
/// # Arguments
/// * `parsed_crate` - The parsed crate with all AST nodes
///
/// # Returns
/// Complete crate documentation
// Build script: generates docs at compile time inside the build, reading and writing the real workspace and target dir, not a project.
#[allow(clippy::disallowed_methods)]
pub fn extract_documentation(
    parsed_crate: &ParsedCrate,
) -> Result<CrateDocumentation, Box<dyn Error>> {
    let mut docs = CrateDocumentation {
        name: parsed_crate.crate_info.name.clone(),
        version: parsed_crate.crate_info.version.clone(),
        description: parsed_crate.crate_info.description.clone(),
        structs: Vec::new(),
        enums: Vec::new(),
        traits: Vec::new(),
        functions: Vec::new(),
        macros: Vec::new(),
        constants: Vec::new(),
        type_aliases: Vec::new(),
    };

    // Extract from each file
    for file in &parsed_crate.files {
        let source_code = std::fs::read_to_string(&file.path)?;
        extract_from_items(&file.ast.items, &mut docs, &source_code);
    }

    Ok(docs)
}

/// Extract documentation from a list of items
fn extract_from_items(items: &[Item], docs: &mut CrateDocumentation, source_code: &str) {
    for item in items {
        match item {
            Item::Struct(item_struct) => {
                docs.structs.push(extract_struct(item_struct, source_code));
            }
            Item::Enum(item_enum) => {
                docs.enums.push(extract_enum(item_enum, source_code));
            }
            Item::Trait(item_trait) => {
                docs.traits.push(extract_trait(item_trait, source_code));
            }
            Item::Fn(item_fn) => {
                docs.functions.push(extract_function(item_fn, source_code));
            }
            Item::Macro(item_macro) => {
                docs.macros.push(extract_macro(item_macro, source_code));
            }
            Item::Const(item_const) => {
                docs.constants
                    .push(extract_constant(item_const, source_code));
            }
            Item::Type(item_type) => {
                docs.type_aliases
                    .push(extract_type_alias(item_type, source_code));
            }
            Item::Mod(item_mod) => {
                // Recursively process module
                if let Some((_, items)) = &item_mod.content {
                    extract_from_items(items, docs, source_code);
                }
            }
            _ => {}
        }
    }
}

/// Extract documentation from a struct
fn extract_struct(item: &ItemStruct, source_code: &str) -> StructDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.generics);

    let fields = match &item.fields {
        syn::Fields::Named(fields) => fields
            .named
            .iter()
            .map(|f| FieldDoc {
                name: f
                    .ident
                    .as_ref()
                    .expect("this arm only visits Fields::Named, whose fields have identifiers")
                    .to_string(),
                doc_comment: extract_doc_comments(&f.attrs),
                type_: extract_source_from_span(&f.ty, source_code),
            })
            .collect(),
        syn::Fields::Unnamed(fields) => fields
            .unnamed
            .iter()
            .enumerate()
            .map(|(i, f)| FieldDoc {
                name: format!("{}", i),
                doc_comment: extract_doc_comments(&f.attrs),
                type_: extract_source_from_span(&f.ty, source_code),
            })
            .collect(),
        syn::Fields::Unit => Vec::new(),
    };

    StructDoc {
        name: item.ident.to_string(),
        doc_comment,
        visibility,
        generics,
        fields,
        source_code: extract_source_from_span(item, source_code),
    }
}

/// Extract documentation from an enum
fn extract_enum(item: &ItemEnum, source_code: &str) -> EnumDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.generics);

    let variants = item
        .variants
        .iter()
        .map(|v| {
            let fields = match &v.fields {
                syn::Fields::Named(_) => VariantFields::Struct,
                syn::Fields::Unnamed(fields) => VariantFields::Tuple(
                    fields
                        .unnamed
                        .iter()
                        .map(|f| extract_source_from_span(&f.ty, source_code))
                        .collect(),
                ),
                syn::Fields::Unit => VariantFields::Unit,
            };

            VariantDoc {
                name: v.ident.to_string(),
                doc_comment: extract_doc_comments(&v.attrs),
                fields,
            }
        })
        .collect();

    EnumDoc {
        name: item.ident.to_string(),
        doc_comment,
        visibility,
        generics,
        variants,
        source_code: extract_source_from_span(item, source_code),
    }
}

/// Extract documentation from a trait
fn extract_trait(item: &ItemTrait, source_code: &str) -> TraitDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);

    let methods = item
        .items
        .iter()
        .filter_map(|trait_item| match trait_item {
            syn::TraitItem::Fn(method) => Some(extract_trait_method(method, source_code)),
            _ => None,
        })
        .collect();

    TraitDoc {
        name: item.ident.to_string(),
        doc_comment,
        visibility,
        methods,
        source_code: extract_source_from_span(item, source_code),
    }
}

/// Extract documentation from a function
fn extract_function(item: &ItemFn, source_code: &str) -> FunctionDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.sig.generics);

    let parameters = item
        .sig
        .inputs
        .iter()
        .map(|arg| match arg {
            syn::FnArg::Typed(pat_type) => ParameterDoc {
                name: extract_source_from_span(&*pat_type.pat, source_code),
                type_: extract_source_from_span(&*pat_type.ty, source_code),
            },
            syn::FnArg::Receiver(_) => ParameterDoc {
                name: "self".to_string(),
                type_: "Self".to_string(),
            },
        })
        .collect();

    let return_type = match &item.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(extract_source_from_span(&**ty, source_code)),
    };

    FunctionDoc {
        name: item.sig.ident.to_string(),
        doc_comment,
        visibility,
        generics,
        parameters,
        return_type,
        is_async: item.sig.asyncness.is_some(),
        is_unsafe: item.sig.unsafety.is_some(),
        is_const: item.sig.constness.is_some(),
        source_code: extract_source_from_span(item, source_code),
    }
}

/// Extract documentation from a macro
fn extract_macro(item: &ItemMacro, source_code: &str) -> MacroDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let name = item
        .ident
        .as_ref()
        .map(|i| i.to_string())
        .unwrap_or_default();

    MacroDoc {
        name,
        doc_comment,
        source_code: extract_source_from_span(item, source_code),
    }
}

/// Extract documentation from a constant
fn extract_constant(item: &ItemConst, source_code: &str) -> ConstantDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);

    ConstantDoc {
        name: item.ident.to_string(),
        doc_comment,
        visibility,
        type_: extract_source_from_span(&*item.ty, source_code),
        value: Some(extract_source_from_span(&*item.expr, source_code)),
    }
}

/// Extract documentation from a type alias
fn extract_type_alias(item: &ItemType, source_code: &str) -> TypeAliasDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);

    TypeAliasDoc {
        name: item.ident.to_string(),
        doc_comment,
        visibility,
        target_type: extract_source_from_span(&*item.ty, source_code),
    }
}

/// Extract method from trait item
fn extract_trait_method(item: &syn::TraitItemFn, source_code: &str) -> MethodDoc {
    let doc_comment = extract_doc_comments(&item.attrs);

    let self_param = item.sig.inputs.iter().find_map(|arg| {
        if let syn::FnArg::Receiver(recv) = arg {
            Some(if recv.reference.is_some() {
                SelfParam::Reference
            } else {
                SelfParam::Value
            })
        } else {
            None
        }
    });

    let parameters = item
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat_type) => Some(ParameterDoc {
                name: extract_source_from_span(&*pat_type.pat, source_code),
                type_: extract_source_from_span(&*pat_type.ty, source_code),
            }),
            _ => None,
        })
        .collect();

    let return_type = match &item.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(extract_source_from_span(&**ty, source_code)),
    };

    MethodDoc {
        name: item.sig.ident.to_string(),
        doc_comment,
        self_param,
        parameters,
        return_type,
        source_code: extract_source_from_span(item, source_code),
    }
}

// Helper functions

/// Extract doc comments from attributes
fn extract_doc_comments(attrs: &[Attribute]) -> Option<String> {
    let mut doc_lines = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(meta) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &meta.value {
                    if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                        doc_lines.push(lit_str.value());
                    }
                }
            }
        }
    }

    if doc_lines.is_empty() {
        return None;
    }

    // Process the doc comment lines to preserve formatting
    // Rustdoc strips the leading space after /// but preserves code block indentation
    let processed = process_doc_comment_lines(&doc_lines);

    if processed.trim().is_empty() {
        None
    } else {
        Some(processed)
    }
}

/// Process doc comment lines to properly preserve code blocks and formatting
fn process_doc_comment_lines(lines: &[String]) -> String {
    let mut result = Vec::new();
    let mut in_code_block = false;

    for line in lines {
        // Check if this line starts or ends a code block
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            result.push(line.to_string());
        } else if in_code_block {
            // Inside code block: preserve all whitespace exactly as-is
            result.push(line.to_string());
        } else {
            // Outside code block: preserve the line but allow rustdoc-style processing
            // Empty lines are preserved
            result.push(line.to_string());
        }
    }

    result.join("\n")
}

/// Extract visibility from syn::Visibility
fn extract_visibility(vis: &syn::Visibility) -> Visibility {
    match vis {
        syn::Visibility::Public(_) => Visibility::Public,
        syn::Visibility::Restricted(restricted) => {
            let path = restricted.path.to_token_stream().to_string();
            if path == "crate" {
                Visibility::PublicCrate
            } else if path == "super" {
                Visibility::PublicSuper
            } else {
                Visibility::PublicIn(path)
            }
        }
        syn::Visibility::Inherited => Visibility::Private,
    }
}

/// Extract generics information
fn extract_generics(generics: &syn::Generics) -> Vec<Generic> {
    generics
        .params
        .iter()
        .filter_map(|param| match param {
            syn::GenericParam::Type(type_param) => Some(Generic {
                name: type_param.ident.to_string(),
                bounds: type_param
                    .bounds
                    .iter()
                    .map(|b| b.to_token_stream().to_string())
                    .collect(),
            }),
            _ => None,
        })
        .collect()
}
