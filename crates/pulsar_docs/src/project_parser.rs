/// Runtime project documentation parser
///
/// Parses Rust source files from a project directory at runtime to generate
/// documentation from doc comments. Uses full AST parsing via syn for accuracy.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use syn::{Item, Attribute, Visibility};
use walkdir::WalkDir;
use serde::{Deserialize, Serialize};

/// Project documentation structure
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectDocumentation {
    pub project_name: String,
    pub project_path: PathBuf,
    pub modules: Vec<ModuleDoc>,
    pub structs: Vec<StructDoc>,
    pub enums: Vec<EnumDoc>,
    pub traits: Vec<TraitDoc>,
    pub functions: Vec<FunctionDoc>,
    pub constants: Vec<ConstantDoc>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModuleDoc {
    pub name: String,
    pub path: Vec<String>,
    pub doc_comment: Option<String>,
    pub visibility: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StructDoc {
    pub name: String,
    pub path: Vec<String>,
    pub doc_comment: Option<String>,
    pub visibility: String,
    pub fields: Vec<FieldDoc>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FieldDoc {
    pub name: String,
    pub ty: String,
    pub doc_comment: Option<String>,
    pub visibility: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct EnumDoc {
    pub name: String,
    pub path: Vec<String>,
    pub doc_comment: Option<String>,
    pub visibility: String,
    pub variants: Vec<VariantDoc>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VariantDoc {
    pub name: String,
    pub doc_comment: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TraitDoc {
    pub name: String,
    pub path: Vec<String>,
    pub doc_comment: Option<String>,
    pub visibility: String,
    pub methods: Vec<MethodDoc>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MethodDoc {
    pub name: String,
    pub signature: String,
    pub doc_comment: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FunctionDoc {
    pub name: String,
    pub path: Vec<String>,
    pub signature: String,
    pub doc_comment: Option<String>,
    pub visibility: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConstantDoc {
    pub name: String,
    pub path: Vec<String>,
    pub ty: String,
    pub doc_comment: Option<String>,
    pub visibility: String,
}

/// Parse project documentation from a project root directory
pub fn parse_project_docs(project_path: &Path) -> Result<ProjectDocumentation, Box<dyn Error>> {
    let project_name = project_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let src_dir = project_path.join("src");
    if !src_dir.exists() {
        return Err("Project src directory not found".into());
    }

    let mut modules = Vec::new();
    let mut structs = Vec::new();
    let mut enums = Vec::new();
    let mut traits = Vec::new();
    let mut functions = Vec::new();
    let mut constants = Vec::new();

    // Walk through all .rs files in src/
    for entry in WalkDir::new(&src_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("rs"))
    {
        let file_path = entry.path();

        // Parse the file
        match parse_rust_file(file_path, &src_dir) {
            Ok(file_docs) => {
                modules.extend(file_docs.modules);
                structs.extend(file_docs.structs);
                enums.extend(file_docs.enums);
                traits.extend(file_docs.traits);
                functions.extend(file_docs.functions);
                constants.extend(file_docs.constants);
            }
            Err(e) => {
                tracing::warn!("Warning: Failed to parse {}: {}", file_path.display(), e);
            }
        }
    }

    Ok(ProjectDocumentation {
        project_name,
        project_path: project_path.to_path_buf(),
        modules,
        structs,
        enums,
        traits,
        functions,
        constants,
    })
}

struct FileDocumentation {
    modules: Vec<ModuleDoc>,
    structs: Vec<StructDoc>,
    enums: Vec<EnumDoc>,
    traits: Vec<TraitDoc>,
    functions: Vec<FunctionDoc>,
    constants: Vec<ConstantDoc>,
}

/// Parse a single Rust file
fn parse_rust_file(file_path: &Path, src_root: &Path) -> Result<FileDocumentation, Box<dyn Error>> {
    let content = fs::read_to_string(file_path)?;
    let ast = syn::parse_file(&content)?;

    // Build module path from file path
    let relative_path = file_path.strip_prefix(src_root)?;
    let module_path = build_module_path(relative_path);

    let mut file_docs = FileDocumentation {
        modules: Vec::new(),
        structs: Vec::new(),
        enums: Vec::new(),
        traits: Vec::new(),
        functions: Vec::new(),
        constants: Vec::new(),
    };

    // Extract documentation from all items
    for item in &ast.items {
        extract_item_docs(item, &module_path, &mut file_docs);
    }

    Ok(file_docs)
}

/// Build module path from file path
fn build_module_path(relative_path: &Path) -> Vec<String> {
    let mut path_parts = Vec::new();

    for component in relative_path.components() {
        if let Some(part) = component.as_os_str().to_str() {
            if part != "mod.rs" && part != "lib.rs" && part != "main.rs" {
                let part_clean = part.trim_end_matches(".rs");
                if !part_clean.is_empty() {
                    path_parts.push(part_clean.to_string());
                }
            }
        }
    }

    path_parts
}

/// Extract documentation from a syntax item
fn extract_item_docs(item: &Item, current_path: &[String], file_docs: &mut FileDocumentation) {
    match item {
        Item::Mod(item_mod) => {
            let doc_comment = extract_doc_comments(&item_mod.attrs);
            let visibility = visibility_to_string(&item_mod.vis);

            let mut mod_path = current_path.to_vec();
            mod_path.push(item_mod.ident.to_string());

            file_docs.modules.push(ModuleDoc {
                name: item_mod.ident.to_string(),
                path: current_path.to_vec(),
                doc_comment,
                visibility,
            });

            // Recursively process items in inline modules
            if let Some((_, items)) = &item_mod.content {
                for sub_item in items {
                    extract_item_docs(sub_item, &mod_path, file_docs);
                }
            }
        }
        Item::Struct(item_struct) => {
            let doc_comment = extract_doc_comments(&item_struct.attrs);
            let visibility = visibility_to_string(&item_struct.vis);

            let fields = item_struct.fields.iter().map(|field| {
                FieldDoc {
                    name: field.ident.as_ref().map(|i| i.to_string()).unwrap_or_else(|| "unnamed".to_string()),
                    ty: quote::quote!(#field.ty).to_string(),
                    doc_comment: extract_doc_comments(&field.attrs),
                    visibility: visibility_to_string(&field.vis),
                }
            }).collect();

            file_docs.structs.push(StructDoc {
                name: item_struct.ident.to_string(),
                path: current_path.to_vec(),
                doc_comment,
                visibility,
                fields,
            });
        }
        Item::Enum(item_enum) => {
            let doc_comment = extract_doc_comments(&item_enum.attrs);
            let visibility = visibility_to_string(&item_enum.vis);

            let variants = item_enum.variants.iter().map(|variant| {
                VariantDoc {
                    name: variant.ident.to_string(),
                    doc_comment: extract_doc_comments(&variant.attrs),
                }
            }).collect();

            file_docs.enums.push(EnumDoc {
                name: item_enum.ident.to_string(),
                path: current_path.to_vec(),
                doc_comment,
                visibility,
                variants,
            });
        }
        Item::Trait(item_trait) => {
            let doc_comment = extract_doc_comments(&item_trait.attrs);
            let visibility = visibility_to_string(&item_trait.vis);

            let methods = item_trait.items.iter().filter_map(|trait_item| {
                if let syn::TraitItem::Fn(method) = trait_item {
                    Some(MethodDoc {
                        name: method.sig.ident.to_string(),
                        signature: quote::quote!(#method.sig).to_string(),
                        doc_comment: extract_doc_comments(&method.attrs),
                    })
                } else {
                    None
                }
            }).collect();

            file_docs.traits.push(TraitDoc {
                name: item_trait.ident.to_string(),
                path: current_path.to_vec(),
                doc_comment,
                visibility,
                methods,
            });
        }
        Item::Fn(item_fn) => {
            let doc_comment = extract_doc_comments(&item_fn.attrs);
            let visibility = visibility_to_string(&item_fn.vis);
            let signature = quote::quote!(#item_fn.sig).to_string();

            file_docs.functions.push(FunctionDoc {
                name: item_fn.sig.ident.to_string(),
                path: current_path.to_vec(),
                signature,
                doc_comment,
                visibility,
            });
        }
        Item::Const(item_const) => {
            let doc_comment = extract_doc_comments(&item_const.attrs);
            let visibility = visibility_to_string(&item_const.vis);

            file_docs.constants.push(ConstantDoc {
                name: item_const.ident.to_string(),
                path: current_path.to_vec(),
                ty: quote::quote!(#item_const.ty).to_string(),
                doc_comment,
                visibility,
            });
        }
        _ => {
            // Ignore other item types for now
        }
    }
}

/// Extract doc comments from attributes
fn extract_doc_comments(attrs: &[Attribute]) -> Option<String> {
    let mut doc_lines = Vec::new();

    for attr in attrs {
        if attr.path().is_ident("doc") {
            if let syn::Meta::NameValue(meta) = &attr.meta {
                if let syn::Expr::Lit(expr_lit) = &meta.value {
                    if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                        let line = lit_str.value();
                        // Clean up the doc comment (remove leading/trailing whitespace)
                        doc_lines.push(line.trim().to_string());
                    }
                }
            }
        }
    }

    if doc_lines.is_empty() {
        None
    } else {
        Some(doc_lines.join("\n"))
    }
}

/// Convert visibility to string
fn visibility_to_string(vis: &Visibility) -> String {
    match vis {
        Visibility::Public(_) => "pub".to_string(),
        Visibility::Restricted(restricted) => {
            let path = &restricted.path;
            format!("pub({})", quote::quote!(#path))
        }
        Visibility::Inherited => "private".to_string(),
    }
}

/// Generate markdown documentation from parsed docs
pub fn generate_markdown(docs: &ProjectDocumentation) -> String {
    let mut markdown = String::new();

    markdown.push_str(&format!("# {} Documentation\n\n", docs.project_name));
    markdown.push_str(&format!("**Project Path:** `{}`\n\n", docs.project_path.display()));

    // Table of contents
    markdown.push_str("## Table of Contents\n\n");

    if !docs.modules.is_empty() {
        markdown.push_str("### Modules\n\n");
        for module in &docs.modules {
            let module_name = format_path_with_name(&module.path, &module.name);
            markdown.push_str(&format!("- [`{}`](#module-{})\n", module_name, slugify(&module_name)));
        }
        markdown.push_str("\n");
    }

    if !docs.structs.is_empty() {
        markdown.push_str("### Structs\n\n");
        for struct_doc in &docs.structs {
            let struct_name = format_path_with_name(&struct_doc.path, &struct_doc.name);
            markdown.push_str(&format!("- [`{}`](#struct-{})\n", struct_name, slugify(&struct_name)));
        }
        markdown.push_str("\n");
    }

    if !docs.enums.is_empty() {
        markdown.push_str("### Enums\n\n");
        for enum_doc in &docs.enums {
            let enum_name = format_path_with_name(&enum_doc.path, &enum_doc.name);
            markdown.push_str(&format!("- [`{}`](#enum-{})\n", enum_name, slugify(&enum_name)));
        }
        markdown.push_str("\n");
    }

    if !docs.traits.is_empty() {
        markdown.push_str("### Traits\n\n");
        for trait_doc in &docs.traits {
            let trait_name = format_path_with_name(&trait_doc.path, &trait_doc.name);
            markdown.push_str(&format!("- [`{}`](#trait-{})\n", trait_name, slugify(&trait_name)));
        }
        markdown.push_str("\n");
    }

    if !docs.functions.is_empty() {
        markdown.push_str("### Functions\n\n");
        for fn_doc in &docs.functions {
            let fn_name = format_path_with_name(&fn_doc.path, &fn_doc.name);
            markdown.push_str(&format!("- [`{}`](#function-{})\n", fn_name, slugify(&fn_name)));
        }
        markdown.push_str("\n");
    }

    // Detailed documentation
    if !docs.modules.is_empty() {
        markdown.push_str("---\n\n## Modules\n\n");
        for module in &docs.modules {
            let module_name = format_path_with_name(&module.path, &module.name);
            markdown.push_str(&format!("### <a name=\"module-{}\"></a>`{}`\n\n", slugify(&module_name), module_name));
            markdown.push_str(&format!("**Visibility:** `{}`\n\n", module.visibility));
            if let Some(doc) = &module.doc_comment {
                markdown.push_str(doc);
                markdown.push_str("\n\n");
            }
        }
    }

    if !docs.structs.is_empty() {
        markdown.push_str("---\n\n## Structs\n\n");
        for struct_doc in &docs.structs {
            let struct_name = format_path_with_name(&struct_doc.path, &struct_doc.name);
            markdown.push_str(&format!("### <a name=\"struct-{}\"></a>`{}`\n\n", slugify(&struct_name), struct_name));
            markdown.push_str(&format!("**Visibility:** `{}`\n\n", struct_doc.visibility));

            if let Some(doc) = &struct_doc.doc_comment {
                markdown.push_str(doc);
                markdown.push_str("\n\n");
            }

            if !struct_doc.fields.is_empty() {
                markdown.push_str("**Fields:**\n\n");
                for field in &struct_doc.fields {
                    markdown.push_str(&format!("- `{}`: `{}` ({})\n", field.name, field.ty, field.visibility));
                    if let Some(field_doc) = &field.doc_comment {
                        markdown.push_str(&format!("  - {}\n", field_doc));
                    }
                }
                markdown.push_str("\n");
            }
        }
    }

    if !docs.enums.is_empty() {
        markdown.push_str("---\n\n## Enums\n\n");
        for enum_doc in &docs.enums {
            let enum_name = format_path_with_name(&enum_doc.path, &enum_doc.name);
            markdown.push_str(&format!("### <a name=\"enum-{}\"></a>`{}`\n\n", slugify(&enum_name), enum_name));
            markdown.push_str(&format!("**Visibility:** `{}`\n\n", enum_doc.visibility));

            if let Some(doc) = &enum_doc.doc_comment {
                markdown.push_str(doc);
                markdown.push_str("\n\n");
            }

            if !enum_doc.variants.is_empty() {
                markdown.push_str("**Variants:**\n\n");
                for variant in &enum_doc.variants {
                    markdown.push_str(&format!("- `{}`\n", variant.name));
                    if let Some(variant_doc) = &variant.doc_comment {
                        markdown.push_str(&format!("  - {}\n", variant_doc));
                    }
                }
                markdown.push_str("\n");
            }
        }
    }

    if !docs.traits.is_empty() {
        markdown.push_str("---\n\n## Traits\n\n");
        for trait_doc in &docs.traits {
            let trait_name = format_path_with_name(&trait_doc.path, &trait_doc.name);
            markdown.push_str(&format!("### <a name=\"trait-{}\"></a>`{}`\n\n", slugify(&trait_name), trait_name));
            markdown.push_str(&format!("**Visibility:** `{}`\n\n", trait_doc.visibility));

            if let Some(doc) = &trait_doc.doc_comment {
                markdown.push_str(doc);
                markdown.push_str("\n\n");
            }

            if !trait_doc.methods.is_empty() {
                markdown.push_str("**Methods:**\n\n");
                for method in &trait_doc.methods {
                    markdown.push_str(&format!("- `{}`\n", method.name));
                    if let Some(method_doc) = &method.doc_comment {
                        markdown.push_str(&format!("  - {}\n", method_doc));
                    }
                    markdown.push_str(&format!("  - Signature: `{}`\n", method.signature));
                }
                markdown.push_str("\n");
            }
        }
    }

    if !docs.functions.is_empty() {
        markdown.push_str("---\n\n## Functions\n\n");
        for fn_doc in &docs.functions {
            let fn_name = format_path_with_name(&fn_doc.path, &fn_doc.name);
            markdown.push_str(&format!("### <a name=\"function-{}\"></a>`{}`\n\n", slugify(&fn_name), fn_name));
            markdown.push_str(&format!("**Visibility:** `{}`\n\n", fn_doc.visibility));
            markdown.push_str(&format!("**Signature:** `{}`\n\n", fn_doc.signature));

            if let Some(doc) = &fn_doc.doc_comment {
                markdown.push_str(doc);
                markdown.push_str("\n\n");
            }
        }
    }

    markdown
}

fn format_path_with_name(path: &[String], name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{}::{}", path.join("::"), name)
    }
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .replace("::", "-")
        .replace(['<', '>', '(', ')', ' ', ','], "")
}
