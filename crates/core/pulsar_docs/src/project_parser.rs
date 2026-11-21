use serde::{Deserialize, Serialize};
/// Runtime project documentation parser
///
/// Parses Rust source files from a project directory at runtime to generate
/// documentation from doc comments. Uses full AST parsing via syn for accuracy.
use std::error::Error;
use std::path::{Path, PathBuf};
use syn::{Attribute, Item, Visibility};

mod markdown;

pub use markdown::generate_markdown;

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
    if !engine_fs::virtual_fs::exists(&src_dir).unwrap_or(false) {
        return Err("Project src directory not found".into());
    }

    let mut modules = Vec::new();
    let mut structs = Vec::new();
    let mut enums = Vec::new();
    let mut traits = Vec::new();
    let mut functions = Vec::new();
    let mut constants = Vec::new();

    // Walk through all .rs files in src/
    for relative in engine_fs::virtual_fs::find_by_extension(&src_dir, "rs") {
        let file_path_buf = src_dir.join(relative);
        let file_path = file_path_buf.as_path();

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
    let content = engine_fs::virtual_fs::read_to_string(file_path)?;
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

            let fields = item_struct
                .fields
                .iter()
                .map(|field| FieldDoc {
                    name: field
                        .ident
                        .as_ref()
                        .map(|i| i.to_string())
                        .unwrap_or_else(|| "unnamed".to_string()),
                    ty: quote::quote!(#field.ty).to_string(),
                    doc_comment: extract_doc_comments(&field.attrs),
                    visibility: visibility_to_string(&field.vis),
                })
                .collect();

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

            let variants = item_enum
                .variants
                .iter()
                .map(|variant| VariantDoc {
                    name: variant.ident.to_string(),
                    doc_comment: extract_doc_comments(&variant.attrs),
                })
                .collect();

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

            let methods = item_trait
                .items
                .iter()
                .filter_map(|trait_item| {
                    if let syn::TraitItem::Fn(method) = trait_item {
                        Some(MethodDoc {
                            name: method.sig.ident.to_string(),
                            signature: quote::quote!(#method.sig).to_string(),
                            doc_comment: extract_doc_comments(&method.attrs),
                        })
                    } else {
                        None
                    }
                })
                .collect();

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
