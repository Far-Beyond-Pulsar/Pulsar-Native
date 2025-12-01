/// Documentation extractor module
/// 
/// Extracts documentation from parsed AST nodes. This is the core of the documentation
/// system - it walks the AST and builds comprehensive documentation structures.

use std::error::Error;
use syn::{Item, ItemStruct, ItemEnum, ItemTrait, ItemFn, ItemMacro, ItemConst, ItemType, Attribute};
use syn::spanned::Spanned;
use quote::ToTokens;
use super::parser::ParsedCrate;
use super::types::*;

/// Extract exact source code for a syntax node - LITERAL extraction from source
fn extract_source_from_span<T: ToTokens>(node: &T, source_code: &str) -> String {
    // Get the span of the node
    let span = node.span();
    
    // Extract positions (1-indexed lines, 0-indexed columns)
    let start = span.start();
    let end = span.end();
    
    // Convert source to char indices for proper UTF-8 handling
    let chars: Vec<char> = source_code.chars().collect();
    
    // Track position in char array
    let mut char_offset = 0;
    let mut start_char_idx = None;
    let mut end_char_idx = None;
    
    for (line_num, line) in source_code.lines().enumerate() {
        let line_index = line_num + 1; // lines() is 0-indexed, span is 1-indexed
        let line_char_count = line.chars().count();
        
        if line_index == start.line && start_char_idx.is_none() {
            if start.column <= line_char_count {
                start_char_idx = Some(char_offset + start.column);
            } else {
                start_char_idx = Some(char_offset + line_char_count);
            }
        }
        
        if line_index == end.line && end_char_idx.is_none() {
            if end.column <= line_char_count {
                end_char_idx = Some(char_offset + end.column);
            } else {
                end_char_idx = Some(char_offset + line_char_count);
            }
        }
        
        char_offset += line_char_count + 1; // +1 for the newline character
        
        if start_char_idx.is_some() && end_char_idx.is_some() {
            break;
        }
    }
    
    // Extract the exact slice using char indices
    if let (Some(start_idx), Some(end_idx)) = (start_char_idx, end_char_idx) {
        if start_idx < chars.len() && end_idx <= chars.len() && start_idx < end_idx {
            return chars[start_idx..end_idx].iter().collect();
        }
    }
    
    // Fallback to token stream if span extraction fails
    node.to_token_stream().to_string()
}

/// Extract documentation from a parsed crate
/// 
/// # Arguments
/// * `parsed_crate` - The parsed crate with all AST nodes
/// 
/// # Returns
/// Complete crate documentation
pub fn extract_documentation(parsed_crate: &ParsedCrate) -> Result<CrateDocumentation, Box<dyn Error>> {
    let mut docs = CrateDocumentation {
        name: parsed_crate.crate_info.name.clone(),
        version: parsed_crate.crate_info.version.clone(),
        description: parsed_crate.crate_info.description.clone(),
        modules: Vec::new(),
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
        extract_from_items(&file.ast.items, &mut docs, &file.path, vec![], &source_code);
    }
    
    Ok(docs)
}

/// Extract documentation from a list of items
fn extract_from_items(
    items: &[Item],
    docs: &mut CrateDocumentation,
    file_path: &std::path::Path,
    current_path: Vec<String>,
    source_code: &str,
) {
    for item in items {
        match item {
            Item::Struct(item_struct) => {
                docs.structs.push(extract_struct(item_struct, file_path, &current_path, source_code));
            }
            Item::Enum(item_enum) => {
                docs.enums.push(extract_enum(item_enum, file_path, &current_path, source_code));
            }
            Item::Trait(item_trait) => {
                docs.traits.push(extract_trait(item_trait, file_path, &current_path, source_code));
            }
            Item::Fn(item_fn) => {
                docs.functions.push(extract_function(item_fn, file_path, &current_path, source_code));
            }
            Item::Macro(item_macro) => {
                docs.macros.push(extract_macro(item_macro, file_path, &current_path, source_code));
            }
            Item::Const(item_const) => {
                docs.constants.push(extract_constant(item_const, file_path, &current_path, source_code));
            }
            Item::Type(item_type) => {
                docs.type_aliases.push(extract_type_alias(item_type, file_path, &current_path, source_code));
            }
            Item::Mod(item_mod) => {
                // Recursively process module
                if let Some((_, items)) = &item_mod.content {
                    let mut mod_path = current_path.clone();
                    mod_path.push(item_mod.ident.to_string());
                    extract_from_items(items, docs, file_path, mod_path, source_code);
                }
            }
            _ => {}
        }
    }
}

/// Extract documentation from a struct
fn extract_struct(item: &ItemStruct, file_path: &std::path::Path, path: &[String], source_code: &str) -> StructDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.generics);
    
    let fields = match &item.fields {
        syn::Fields::Named(fields) => {
            fields.named.iter().map(|f| FieldDoc {
                name: f.ident.as_ref().unwrap().to_string(),
                doc_comment: extract_doc_comments(&f.attrs),
                visibility: extract_visibility(&f.vis),
                type_: extract_source_from_span(&f.ty, source_code),
            }).collect()
        }
        syn::Fields::Unnamed(fields) => {
            fields.unnamed.iter().enumerate().map(|(i, f)| FieldDoc {
                name: format!("{}", i),
                doc_comment: extract_doc_comments(&f.attrs),
                visibility: extract_visibility(&f.vis),
                type_: extract_source_from_span(&f.ty, source_code),
            }).collect()
        }
        syn::Fields::Unit => Vec::new(),
    };
    
    StructDoc {
        name: item.ident.to_string(),
        path: path.to_vec(),
        doc_comment,
        visibility,
        generics,
        fields,
        is_tuple_struct: matches!(item.fields, syn::Fields::Unnamed(_)),
        source_location: SourceLocation {
            file: file_path.to_path_buf(),
            line: 0, // Line info not available
            column: 0,
        },
        source_code: extract_source_from_span(item, source_code),
        impls: Vec::new(), // Will be filled later
    }
}

/// Extract documentation from an enum
fn extract_enum(item: &ItemEnum, file_path: &std::path::Path, path: &[String], source_code: &str) -> EnumDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.generics);
    
    let variants = item.variants.iter().map(|v| {
        let fields = match &v.fields {
            syn::Fields::Named(fields) => {
                VariantFields::Struct(fields.named.iter().map(|f| FieldDoc {
                    name: f.ident.as_ref().unwrap().to_string(),
                    doc_comment: extract_doc_comments(&f.attrs),
                    visibility: extract_visibility(&f.vis),
                    type_: extract_source_from_span(&f.ty, source_code),
                }).collect())
            }
            syn::Fields::Unnamed(fields) => {
                VariantFields::Tuple(
                    fields.unnamed.iter()
                        .map(|f| extract_source_from_span(&f.ty, source_code))
                        .collect()
                )
            }
            syn::Fields::Unit => VariantFields::Unit,
        };
        
        VariantDoc {
            name: v.ident.to_string(),
            doc_comment: extract_doc_comments(&v.attrs),
            fields,
        }
    }).collect();
    
    EnumDoc {
        name: item.ident.to_string(),
        path: path.to_vec(),
        doc_comment,
        visibility,
        generics,
        variants,
        source_location: SourceLocation {
            file: file_path.to_path_buf(),
            line: 0, // Line info not available
            column: 0,
        },
        source_code: extract_source_from_span(item, source_code),
        impls: Vec::new(),
    }
}

/// Extract documentation from a trait
fn extract_trait(item: &ItemTrait, file_path: &std::path::Path, path: &[String], source_code: &str) -> TraitDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.generics);
    
    let supertraits = item.supertraits.iter()
        .map(|t| t.to_token_stream().to_string())
        .collect();
    
    let mut associated_types = Vec::new();
    let mut methods = Vec::new();
    
    for trait_item in &item.items {
        match trait_item {
            syn::TraitItem::Type(ty) => {
                associated_types.push(AssociatedTypeDoc {
                    name: ty.ident.to_string(),
                    doc_comment: extract_doc_comments(&ty.attrs),
                    bounds: ty.bounds.iter().map(|b| b.to_token_stream().to_string()).collect(),
                    default: ty.default.as_ref().map(|(_, ty)| ty.to_token_stream().to_string()),
                });
            }
            syn::TraitItem::Fn(method) => {
                methods.push(extract_trait_method(method, source_code));
            }
            _ => {}
        }
    }
    
    TraitDoc {
        name: item.ident.to_string(),
        path: path.to_vec(),
        doc_comment,
        visibility,
        generics,
        supertraits,
        associated_types,
        methods,
        source_location: SourceLocation {
            file: file_path.to_path_buf(),
            line: 0, // Line info not available
            column: 0,
        },
        source_code: extract_source_from_span(item, source_code),
    }
}

/// Extract documentation from a function
fn extract_function(item: &ItemFn, file_path: &std::path::Path, path: &[String], source_code: &str) -> FunctionDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.sig.generics);
    
    let parameters = item.sig.inputs.iter().map(|arg| {
        match arg {
            syn::FnArg::Typed(pat_type) => {
                ParameterDoc {
                    name: extract_source_from_span(&*pat_type.pat, source_code),
                    type_: extract_source_from_span(&*pat_type.ty, source_code),
                }
            }
            syn::FnArg::Receiver(_) => {
                ParameterDoc {
                    name: "self".to_string(),
                    type_: "Self".to_string(),
                }
            }
        }
    }).collect();
    
    let return_type = match &item.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(extract_source_from_span(&**ty, source_code)),
    };
    
    FunctionDoc {
        name: item.sig.ident.to_string(),
        path: path.to_vec(),
        doc_comment,
        visibility,
        generics,
        parameters,
        return_type,
        is_async: item.sig.asyncness.is_some(),
        is_unsafe: item.sig.unsafety.is_some(),
        is_const: item.sig.constness.is_some(),
        source_location: SourceLocation {
            file: file_path.to_path_buf(),
            line: 0,
            column: 0,
        },
        source_code: extract_source_from_span(item, source_code),
    }
}

/// Extract documentation from a macro
fn extract_macro(item: &ItemMacro, file_path: &std::path::Path, path: &[String], source_code: &str) -> MacroDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let name = item.ident.as_ref().map(|i| i.to_string()).unwrap_or_default();
    
    MacroDoc {
        name,
        path: path.to_vec(),
        doc_comment,
        visibility: Visibility::Public, // Macros are typically public
        source_location: SourceLocation {
            file: file_path.to_path_buf(),
            line: 0, // Macro span handling is complex
            column: 0,
        },
        source_code: extract_source_from_span(item, source_code),
        example_usage: Vec::new(),
    }
}

/// Extract documentation from a constant
fn extract_constant(item: &ItemConst, file_path: &std::path::Path, path: &[String], source_code: &str) -> ConstantDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    
    ConstantDoc {
        name: item.ident.to_string(),
        path: path.to_vec(),
        doc_comment,
        visibility,
        type_: extract_source_from_span(&*item.ty, source_code),
        value: Some(extract_source_from_span(&*item.expr, source_code)),
        source_location: SourceLocation {
            file: file_path.to_path_buf(),
            line: 0, // Line info not available
            column: 0,
        },
    }
}

/// Extract documentation from a type alias
fn extract_type_alias(item: &ItemType, file_path: &std::path::Path, path: &[String], source_code: &str) -> TypeAliasDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let visibility = extract_visibility(&item.vis);
    let generics = extract_generics(&item.generics);
    
    TypeAliasDoc {
        name: item.ident.to_string(),
        path: path.to_vec(),
        doc_comment,
        visibility,
        generics,
        target_type: extract_source_from_span(&*item.ty, source_code),
        source_location: SourceLocation {
            file: file_path.to_path_buf(),
            line: 0, // Line info not available
            column: 0,
        },
    }
}

/// Extract method from trait item
fn extract_trait_method(item: &syn::TraitItemFn, source_code: &str) -> MethodDoc {
    let doc_comment = extract_doc_comments(&item.attrs);
    let generics = extract_generics(&item.sig.generics);
    
    let self_param = item.sig.inputs.iter().find_map(|arg| {
        if let syn::FnArg::Receiver(recv) = arg {
            Some(if recv.reference.is_some() {
                SelfParam::Reference { mutable: recv.mutability.is_some() }
            } else {
                SelfParam::Value
            })
        } else {
            None
        }
    });
    
    let parameters = item.sig.inputs.iter().filter_map(|arg| {
        match arg {
            syn::FnArg::Typed(pat_type) => Some(ParameterDoc {
                name: extract_source_from_span(&*pat_type.pat, source_code),
                type_: extract_source_from_span(&*pat_type.ty, source_code),
            }),
            _ => None,
        }
    }).collect();
    
    let return_type = match &item.sig.output {
        syn::ReturnType::Default => None,
        syn::ReturnType::Type(_, ty) => Some(extract_source_from_span(&**ty, source_code)),
    };
    
    MethodDoc {
        name: item.sig.ident.to_string(),
        doc_comment,
        visibility: Visibility::Public,
        generics,
        self_param,
        parameters,
        return_type,
        is_async: item.sig.asyncness.is_some(),
        is_unsafe: item.sig.unsafety.is_some(),
        is_const: item.sig.constness.is_some(),
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
        None
    } else {
        Some(doc_lines.join("\n").trim().to_string())
    }
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
    generics.params.iter().filter_map(|param| {
        match param {
            syn::GenericParam::Type(type_param) => {
                Some(Generic {
                    name: type_param.ident.to_string(),
                    bounds: type_param.bounds.iter()
                        .map(|b| b.to_token_stream().to_string())
                        .collect(),
                    default: type_param.default.as_ref()
                        .map(|ty| ty.to_token_stream().to_string()),
                })
            }
            _ => None,
        }
    }).collect()
}

