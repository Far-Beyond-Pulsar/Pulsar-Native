/// Markdown generation module - Hierarchical structure
/// 
/// Generates a complete folder hierarchy with individual pages and JSON indices

use std::error::Error;
use std::fmt::Write;
use std::fs;
use std::path::Path;
use serde::Serialize;
use super::types::*;
use super::workspace::CrateInfo;

/// JSON index structures
#[derive(Serialize)]
struct CrateIndex {
    name: String,
    version: String,
    description: Option<String>,
    sections: Vec<Section>,
}

#[derive(Serialize)]
struct Section {
    name: String,
    path: String,
    count: usize,
    items: Vec<IndexItem>,
}

#[derive(Serialize)]
struct IndexItem {
    name: String,
    path: String,
    doc_summary: Option<String>,
}

/// Helper function to write doc comments to markdown
/// Ensures proper formatting of code blocks and preserves blank lines
fn write_doc_comment(output: &mut String, doc: &str) -> std::fmt::Result {
    // Write the doc comment as-is, but ensure it ends with proper spacing
    writeln!(output, "{}", doc)?;
    
    // Add a blank line after doc comments for proper markdown spacing
    // unless the doc already ends with multiple newlines
    if !doc.ends_with("\n\n") {
        writeln!(output)?;
    }
    
    Ok(())
}

/// Generate hierarchical documentation structure
pub fn generate_hierarchical_docs(
    docs: &CrateDocumentation,
    _crate_info: &CrateInfo,
    output_dir: &Path,
) -> Result<(), Box<dyn Error>> {
    let crate_dir = output_dir.join(&docs.name);
    fs::create_dir_all(&crate_dir)?;
    
    // Generate crate index page
    generate_crate_index(&crate_dir, docs)?;
    
    // Generate sections with individual pages
    generate_structs_pages(&crate_dir, &docs.structs)?;
    generate_enums_pages(&crate_dir, &docs.enums)?;
    generate_traits_pages(&crate_dir, &docs.traits)?;
    generate_functions_pages(&crate_dir, &docs.functions)?;
    generate_macros_pages(&crate_dir, &docs.macros)?;
    generate_constants_pages(&crate_dir, &docs.constants)?;
    generate_type_aliases_pages(&crate_dir, &docs.type_aliases)?;
    
    // Generate crate-level index.json
    generate_crate_json_index(&crate_dir, docs)?;
    
    Ok(())
}

/// Generate crate index page
fn generate_crate_index(crate_dir: &Path, docs: &CrateDocumentation) -> Result<(), Box<dyn Error>> {
    let mut md = String::new();
    
    writeln!(md, "# {}", docs.name)?;
    writeln!(md)?;
    writeln!(md, "**Version:** {}", docs.version)?;
    writeln!(md)?;
    
    if let Some(desc) = &docs.description {
        writeln!(md, "{}", desc)?;
        writeln!(md)?;
    }
    
    writeln!(md, "## Contents")?;
    writeln!(md)?;
    
    if !docs.structs.is_empty() {
        writeln!(md, "- [Structs](structs/) ({} items)", docs.structs.len())?;
    }
    if !docs.enums.is_empty() {
        writeln!(md, "- [Enums](enums/) ({} items)", docs.enums.len())?;
    }
    if !docs.traits.is_empty() {
        writeln!(md, "- [Traits](traits/) ({} items)", docs.traits.len())?;
    }
    if !docs.functions.is_empty() {
        writeln!(md, "- [Functions](functions/) ({} items)", docs.functions.len())?;
    }
    if !docs.macros.is_empty() {
        writeln!(md, "- [Macros](macros/) ({} items)", docs.macros.len())?;
    }
    if !docs.constants.is_empty() {
        writeln!(md, "- [Constants](constants/) ({} items)", docs.constants.len())?;
    }
    if !docs.type_aliases.is_empty() {
        writeln!(md, "- [Type Aliases](type_aliases/) ({} items)", docs.type_aliases.len())?;
    }
    
    fs::write(crate_dir.join("index.md"), md)?;
    Ok(())
}

/// Generate JSON index for sidebar navigation
fn generate_crate_json_index(crate_dir: &Path, docs: &CrateDocumentation) -> Result<(), Box<dyn Error>> {
    let mut sections = Vec::new();
    
    if !docs.structs.is_empty() {
        sections.push(Section {
            name: "Structs".to_string(),
            path: "structs".to_string(),
            count: docs.structs.iter().filter(|s| s.visibility == Visibility::Public).count(),
            items: docs.structs.iter()
                .filter(|s| s.visibility == Visibility::Public)
                .map(|s| IndexItem {
                    name: s.name.clone(),
                    path: format!("structs/{}.md", s.name),
                    doc_summary: s.doc_comment.as_ref().and_then(|d| d.lines().next().map(String::from)),
                })
                .collect(),
        });
    }
    
    if !docs.enums.is_empty() {
        sections.push(Section {
            name: "Enums".to_string(),
            path: "enums".to_string(),
            count: docs.enums.iter().filter(|e| e.visibility == Visibility::Public).count(),
            items: docs.enums.iter()
                .filter(|e| e.visibility == Visibility::Public)
                .map(|e| IndexItem {
                    name: e.name.clone(),
                    path: format!("enums/{}.md", e.name),
                    doc_summary: e.doc_comment.as_ref().and_then(|d| d.lines().next().map(String::from)),
                })
                .collect(),
        });
    }
    
    if !docs.traits.is_empty() {
        sections.push(Section {
            name: "Traits".to_string(),
            path: "traits".to_string(),
            count: docs.traits.iter().filter(|t| t.visibility == Visibility::Public).count(),
            items: docs.traits.iter()
                .filter(|t| t.visibility == Visibility::Public)
                .map(|t| IndexItem {
                    name: t.name.clone(),
                    path: format!("traits/{}.md", t.name),
                    doc_summary: t.doc_comment.as_ref().and_then(|d| d.lines().next().map(String::from)),
                })
                .collect(),
        });
    }
    
    if !docs.functions.is_empty() {
        sections.push(Section {
            name: "Functions".to_string(),
            path: "functions".to_string(),
            count: docs.functions.iter().filter(|f| f.visibility == Visibility::Public).count(),
            items: docs.functions.iter()
                .filter(|f| f.visibility == Visibility::Public)
                .map(|f| IndexItem {
                    name: f.name.clone(),
                    path: format!("functions/{}.md", f.name),
                    doc_summary: f.doc_comment.as_ref().and_then(|d| d.lines().next().map(String::from)),
                })
                .collect(),
        });
    }
    
    if !docs.macros.is_empty() {
        sections.push(Section {
            name: "Macros".to_string(),
            path: "macros".to_string(),
            count: docs.macros.len(),
            items: docs.macros.iter()
                .map(|m| IndexItem {
                    name: m.name.clone(),
                    path: format!("macros/{}.md", m.name),
                    doc_summary: m.doc_comment.as_ref().and_then(|d| d.lines().next().map(String::from)),
                })
                .collect(),
        });
    }
    
    if !docs.constants.is_empty() {
        sections.push(Section {
            name: "Constants".to_string(),
            path: "constants".to_string(),
            count: docs.constants.iter().filter(|c| c.visibility == Visibility::Public).count(),
            items: docs.constants.iter()
                .filter(|c| c.visibility == Visibility::Public)
                .map(|c| IndexItem {
                    name: c.name.clone(),
                    path: format!("constants/{}.md", c.name),
                    doc_summary: c.doc_comment.as_ref().and_then(|d| d.lines().next().map(String::from)),
                })
                .collect(),
        });
    }
    
    let index = CrateIndex {
        name: docs.name.clone(),
        version: docs.version.clone(),
        description: docs.description.clone(),
        sections,
    };
    
    let json = serde_json::to_string_pretty(&index)?;
    fs::write(crate_dir.join("index.json"), json)?;
    
    Ok(())
}

/// Generate struct pages
fn generate_structs_pages(crate_dir: &Path, structs: &[StructDoc]) -> Result<(), Box<dyn Error>> {
    if structs.is_empty() {
        return Ok(());
    }
    
    let structs_dir = crate_dir.join("structs");
    fs::create_dir_all(&structs_dir)?;
    
    for s in structs {
        if s.visibility != Visibility::Public {
            continue;
        }
        
        let mut md = String::new();
        
        writeln!(md, "# `{}`", s.name)?;
        writeln!(md)?;
        
        if let Some(doc) = &s.doc_comment {
            write_doc_comment(&mut md, doc)?;
        }
        
        // Signature
        write!(md, "```rust\n{} struct {}", visibility_str(&s.visibility), s.name)?;
        if !s.generics.is_empty() {
            write!(md, "<")?;
            for (i, g) in s.generics.iter().enumerate() {
                if i > 0 {
                    write!(md, ", ")?;
                }
                write!(md, "{}", g.name)?;
                if !g.bounds.is_empty() {
                    write!(md, ": {}", g.bounds.join(" + "))?;
                }
            }
            write!(md, ">")?;
        }
        writeln!(md, "\n```")?;
        writeln!(md)?;
        
        // Fields
        if !s.fields.is_empty() {
            writeln!(md, "## Fields")?;
            writeln!(md)?;
            for field in &s.fields {
                writeln!(md, "### `{}`", field.name)?;
                writeln!(md)?;
                writeln!(md, "**Type:** `{}`", field.type_)?;
                writeln!(md)?;
                if let Some(doc) = &field.doc_comment {
                    write_doc_comment(&mut md, doc)?;
                }
                writeln!(md, "---")?;
                writeln!(md)?;
            }
        }
        
        // Source code
        writeln!(md, "## Source Code")?;
        writeln!(md)?;
        writeln!(md, "```rust")?;
        writeln!(md, "{}", s.source_code)?;
        writeln!(md, "```")?;
        
        fs::write(structs_dir.join(format!("{}.md", s.name)), md)?;
    }
    
    Ok(())
}

/// Generate enum pages
fn generate_enums_pages(crate_dir: &Path, enums: &[EnumDoc]) -> Result<(), Box<dyn Error>> {
    if enums.is_empty() {
        return Ok(());
    }
    
    let enums_dir = crate_dir.join("enums");
    fs::create_dir_all(&enums_dir)?;
    
    for e in enums {
        if e.visibility != Visibility::Public {
            continue;
        }
        
        let mut md = String::new();
        
        writeln!(md, "# `{}`", e.name)?;
        writeln!(md)?;
        
        if let Some(doc) = &e.doc_comment {
            write_doc_comment(&mut md, doc)?;
        }
        
        // Signature
        write!(md, "```rust\n{} enum {}", visibility_str(&e.visibility), e.name)?;
        if !e.generics.is_empty() {
            write!(md, "<")?;
            for (i, g) in e.generics.iter().enumerate() {
                if i > 0 {
                    write!(md, ", ")?;
                }
                write!(md, "{}", g.name)?;
            }
            write!(md, ">")?;
        }
        writeln!(md, "\n```")?;
        writeln!(md)?;
        
        // Variants
        writeln!(md, "## Variants")?;
        writeln!(md)?;
        for variant in &e.variants {
            write!(md, "### `{}`", variant.name)?;
            
            match &variant.fields {
                VariantFields::Unit => {}
                VariantFields::Tuple(types) => {
                    write!(md, "({})", types.join(", "))?;
                }
                VariantFields::Struct(_) => {
                    write!(md, " {{ ... }}")?;
                }
            }
            writeln!(md)?;
            writeln!(md)?;
            
            if let Some(doc) = &variant.doc_comment {
                write_doc_comment(&mut md, doc)?;
            }
            
            writeln!(md, "---")?;
            writeln!(md)?;
        }
        
        // Source code
        writeln!(md, "## Source Code")?;
        writeln!(md)?;
        writeln!(md, "```rust")?;
        writeln!(md, "{}", e.source_code)?;
        writeln!(md, "```")?;
        
        fs::write(enums_dir.join(format!("{}.md", e.name)), md)?;
    }
    
    Ok(())
}

/// Generate trait pages
fn generate_traits_pages(crate_dir: &Path, traits: &[TraitDoc]) -> Result<(), Box<dyn Error>> {
    if traits.is_empty() {
        return Ok(());
    }
    
    let traits_dir = crate_dir.join("traits");
    fs::create_dir_all(&traits_dir)?;
    
    for t in traits {
        if t.visibility != Visibility::Public {
            continue;
        }
        
        let mut md = String::new();
        
        writeln!(md, "# `{}`", t.name)?;
        writeln!(md)?;
        
        if let Some(doc) = &t.doc_comment {
            write_doc_comment(&mut md, doc)?;
        }
        
        // Methods
        if !t.methods.is_empty() {
            writeln!(md, "## Methods")?;
            writeln!(md)?;
            for method in &t.methods {
                write!(md, "### `{}`", method.name)?;
                write!(md, "(")?;
                if method.self_param.is_some() {
                    write!(md, "&self")?;
                    if !method.parameters.is_empty() {
                        write!(md, ", ")?;
                    }
                }
                for (i, param) in method.parameters.iter().enumerate() {
                    if i > 0 {
                        write!(md, ", ")?;
                    }
                    write!(md, "{}: {}", param.name, param.type_)?;
                }
                write!(md, ")")?;
                
                if let Some(ret) = &method.return_type {
                    write!(md, " -> {}", ret)?;
                }
                writeln!(md)?;
                writeln!(md)?;
                
                if let Some(doc) = &method.doc_comment {
                    write_doc_comment(&mut md, doc)?;
                }
                
                writeln!(md, "```rust")?;
                writeln!(md, "{}", method.source_code)?;
                writeln!(md, "```")?;
                writeln!(md)?;
                writeln!(md, "---")?;
                writeln!(md)?;
            }
        }
        
        // Source code
        writeln!(md, "## Source Code")?;
        writeln!(md)?;
        writeln!(md, "```rust")?;
        writeln!(md, "{}", t.source_code)?;
        writeln!(md, "```")?;
        
        fs::write(traits_dir.join(format!("{}.md", t.name)), md)?;
    }
    
    Ok(())
}

/// Generate function pages
fn generate_functions_pages(crate_dir: &Path, functions: &[FunctionDoc]) -> Result<(), Box<dyn Error>> {
    if functions.is_empty() {
        return Ok(());
    }
    
    let functions_dir = crate_dir.join("functions");
    fs::create_dir_all(&functions_dir)?;
    
    for f in functions {
        if f.visibility != Visibility::Public {
            continue;
        }
        
        let mut md = String::new();
        
        // Signature as title
        write!(md, "# `")?;
        if f.is_const {
            write!(md, "const ")?;
        }
        if f.is_async {
            write!(md, "async ")?;
        }
        if f.is_unsafe {
            write!(md, "unsafe ")?;
        }
        write!(md, "fn {}", f.name)?;
        
        if !f.generics.is_empty() {
            write!(md, "<")?;
            for (i, g) in f.generics.iter().enumerate() {
                if i > 0 {
                    write!(md, ", ")?;
                }
                write!(md, "{}", g.name)?;
            }
            write!(md, ">")?;
        }
        
        write!(md, "(")?;
        for (i, param) in f.parameters.iter().enumerate() {
            if i > 0 {
                write!(md, ", ")?;
            }
            write!(md, "{}: {}", param.name, param.type_)?;
        }
        write!(md, ")")?;
        
        if let Some(ret) = &f.return_type {
            write!(md, " -> {}", ret)?;
        }
        
        writeln!(md, "`")?;
        writeln!(md)?;
        
        if let Some(doc) = &f.doc_comment {
            write_doc_comment(&mut md, doc)?;
        }
        
        // Source code
        writeln!(md, "## Source Code")?;
        writeln!(md)?;
        writeln!(md, "```rust")?;
        writeln!(md, "{}", f.source_code)?;
        writeln!(md, "```")?;
        
        fs::write(functions_dir.join(format!("{}.md", f.name)), md)?;
    }
    
    Ok(())
}

/// Generate macro pages
fn generate_macros_pages(crate_dir: &Path, macros: &[MacroDoc]) -> Result<(), Box<dyn Error>> {
    if macros.is_empty() {
        return Ok(());
    }
    
    let macros_dir = crate_dir.join("macros");
    fs::create_dir_all(&macros_dir)?;
    
    for m in macros {
        let mut md = String::new();
        
        writeln!(md, "# `{}!`", m.name)?;
        writeln!(md)?;
        
        if let Some(doc) = &m.doc_comment {
            write_doc_comment(&mut md, doc)?;
        }
        
        writeln!(md, "## Source Code")?;
        writeln!(md)?;
        writeln!(md, "```rust")?;
        writeln!(md, "{}", m.source_code)?;
        writeln!(md, "```")?;
        
        fs::write(macros_dir.join(format!("{}.md", m.name)), md)?;
    }
    
    Ok(())
}

/// Generate constant pages
fn generate_constants_pages(crate_dir: &Path, constants: &[ConstantDoc]) -> Result<(), Box<dyn Error>> {
    if constants.is_empty() {
        return Ok(());
    }
    
    let constants_dir = crate_dir.join("constants");
    fs::create_dir_all(&constants_dir)?;
    
    for c in constants {
        if c.visibility != Visibility::Public {
            continue;
        }
        
        let mut md = String::new();
        
        write!(md, "# `const {}: {}", c.name, c.type_)?;
        if let Some(value) = &c.value {
            write!(md, " = {}", value)?;
        }
        writeln!(md, "`")?;
        writeln!(md)?;
        
        if let Some(doc) = &c.doc_comment {
            write_doc_comment(&mut md, doc)?;
        }
        
        fs::write(constants_dir.join(format!("{}.md", c.name)), md)?;
    }
    
    Ok(())
}

/// Generate type alias pages
fn generate_type_aliases_pages(crate_dir: &Path, type_aliases: &[TypeAliasDoc]) -> Result<(), Box<dyn Error>> {
    if type_aliases.is_empty() {
        return Ok(());
    }
    
    let type_aliases_dir = crate_dir.join("type_aliases");
    fs::create_dir_all(&type_aliases_dir)?;
    
    for t in type_aliases {
        if t.visibility != Visibility::Public {
            continue;
        }
        
        let mut md = String::new();
        
        writeln!(md, "# `type {} = {}`", t.name, t.target_type)?;
        writeln!(md)?;
        
        if let Some(doc) = &t.doc_comment {
            writeln!(md, "{}", doc)?;
            writeln!(md)?;
        }
        
        fs::write(type_aliases_dir.join(format!("{}.md", t.name)), md)?;
    }
    
    Ok(())
}

/// Helper to convert visibility to string
fn visibility_str(vis: &Visibility) -> &'static str {
    match vis {
        Visibility::Public => "pub",
        Visibility::PublicCrate => "pub(crate)",
        Visibility::PublicSuper => "pub(super)",
        Visibility::PublicIn(_) => "pub(in ...)",
        Visibility::Private => "",
    }
}
