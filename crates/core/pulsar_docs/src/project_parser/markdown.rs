//! Rendering parsed project documentation as markdown.

use super::{EnumDoc, FunctionDoc, ModuleDoc, ProjectDocumentation, StructDoc, TraitDoc};

/// Generate markdown documentation from parsed docs
pub fn generate_markdown(docs: &ProjectDocumentation) -> String {
    let mut markdown = String::new();

    markdown.push_str(&format!("# {} Documentation\n\n", docs.project_name));
    markdown.push_str(&format!(
        "**Project Path:** `{}`\n\n",
        docs.project_path.display()
    ));

    push_table_of_contents(&mut markdown, docs);
    push_modules(&mut markdown, &docs.modules);
    push_structs(&mut markdown, &docs.structs);
    push_enums(&mut markdown, &docs.enums);
    push_traits(&mut markdown, &docs.traits);
    push_functions(&mut markdown, &docs.functions);

    markdown
}

fn push_table_of_contents(markdown: &mut String, docs: &ProjectDocumentation) {
    markdown.push_str("## Table of Contents\n\n");
    let entry_names = |entries: Vec<(&[String], &str)>| {
        entries
            .into_iter()
            .map(|(path, name)| format_path_with_name(path, name))
            .collect::<Vec<_>>()
    };
    push_toc_group(
        markdown,
        "Modules",
        "module",
        &entry_names(
            docs.modules
                .iter()
                .map(|m| (&m.path[..], &m.name[..]))
                .collect(),
        ),
    );
    push_toc_group(
        markdown,
        "Structs",
        "struct",
        &entry_names(
            docs.structs
                .iter()
                .map(|s| (&s.path[..], &s.name[..]))
                .collect(),
        ),
    );
    push_toc_group(
        markdown,
        "Enums",
        "enum",
        &entry_names(
            docs.enums
                .iter()
                .map(|e| (&e.path[..], &e.name[..]))
                .collect(),
        ),
    );
    push_toc_group(
        markdown,
        "Traits",
        "trait",
        &entry_names(
            docs.traits
                .iter()
                .map(|t| (&t.path[..], &t.name[..]))
                .collect(),
        ),
    );
    push_toc_group(
        markdown,
        "Functions",
        "function",
        &entry_names(
            docs.functions
                .iter()
                .map(|f| (&f.path[..], &f.name[..]))
                .collect(),
        ),
    );
}

/// One `### Title` list of anchor links; nothing when `names` is empty.
fn push_toc_group(markdown: &mut String, title: &str, anchor: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    markdown.push_str(&format!("### {title}\n\n"));
    for name in names {
        markdown.push_str(&format!("- [`{}`](#{anchor}-{})\n", name, slugify(name)));
    }
    markdown.push('\n');
}

/// The `---` rule and `## Title` heading that open a detail section.
fn push_section_heading(markdown: &mut String, title: &str) {
    markdown.push_str(&format!("---\n\n## {title}\n\n"));
}

/// The anchored `### name` heading of one documented item.
fn push_item_heading(markdown: &mut String, anchor: &str, name: &str) {
    markdown.push_str(&format!(
        "### <a name=\"{anchor}-{}\"></a>`{}`\n\n",
        slugify(name),
        name
    ));
}

fn push_doc_comment(markdown: &mut String, doc: &Option<String>) {
    if let Some(doc) = doc {
        markdown.push_str(doc);
        markdown.push_str("\n\n");
    }
}

fn push_modules(markdown: &mut String, modules: &[ModuleDoc]) {
    if modules.is_empty() {
        return;
    }
    push_section_heading(markdown, "Modules");
    for module in modules {
        let module_name = format_path_with_name(&module.path, &module.name);
        push_item_heading(markdown, "module", &module_name);
        markdown.push_str(&format!("**Visibility:** `{}`\n\n", module.visibility));
        push_doc_comment(markdown, &module.doc_comment);
    }
}

fn push_structs(markdown: &mut String, structs: &[StructDoc]) {
    if structs.is_empty() {
        return;
    }
    push_section_heading(markdown, "Structs");
    for struct_doc in structs {
        let struct_name = format_path_with_name(&struct_doc.path, &struct_doc.name);
        push_item_heading(markdown, "struct", &struct_name);
        markdown.push_str(&format!("**Visibility:** `{}`\n\n", struct_doc.visibility));
        push_doc_comment(markdown, &struct_doc.doc_comment);

        if !struct_doc.fields.is_empty() {
            markdown.push_str("**Fields:**\n\n");
            for field in &struct_doc.fields {
                markdown.push_str(&format!(
                    "- `{}`: `{}` ({})\n",
                    field.name, field.ty, field.visibility
                ));
                if let Some(field_doc) = &field.doc_comment {
                    markdown.push_str(&format!("  - {}\n", field_doc));
                }
            }
            markdown.push('\n');
        }
    }
}

fn push_enums(markdown: &mut String, enums: &[EnumDoc]) {
    if enums.is_empty() {
        return;
    }
    push_section_heading(markdown, "Enums");
    for enum_doc in enums {
        let enum_name = format_path_with_name(&enum_doc.path, &enum_doc.name);
        push_item_heading(markdown, "enum", &enum_name);
        markdown.push_str(&format!("**Visibility:** `{}`\n\n", enum_doc.visibility));
        push_doc_comment(markdown, &enum_doc.doc_comment);

        if !enum_doc.variants.is_empty() {
            markdown.push_str("**Variants:**\n\n");
            for variant in &enum_doc.variants {
                markdown.push_str(&format!("- `{}`\n", variant.name));
                if let Some(variant_doc) = &variant.doc_comment {
                    markdown.push_str(&format!("  - {}\n", variant_doc));
                }
            }
            markdown.push('\n');
        }
    }
}

fn push_traits(markdown: &mut String, traits: &[TraitDoc]) {
    if traits.is_empty() {
        return;
    }
    push_section_heading(markdown, "Traits");
    for trait_doc in traits {
        let trait_name = format_path_with_name(&trait_doc.path, &trait_doc.name);
        push_item_heading(markdown, "trait", &trait_name);
        markdown.push_str(&format!("**Visibility:** `{}`\n\n", trait_doc.visibility));
        push_doc_comment(markdown, &trait_doc.doc_comment);

        if !trait_doc.methods.is_empty() {
            markdown.push_str("**Methods:**\n\n");
            for method in &trait_doc.methods {
                markdown.push_str(&format!("- `{}`\n", method.name));
                if let Some(method_doc) = &method.doc_comment {
                    markdown.push_str(&format!("  - {}\n", method_doc));
                }
                markdown.push_str(&format!("  - Signature: `{}`\n", method.signature));
            }
            markdown.push('\n');
        }
    }
}

fn push_functions(markdown: &mut String, functions: &[FunctionDoc]) {
    if functions.is_empty() {
        return;
    }
    push_section_heading(markdown, "Functions");
    for fn_doc in functions {
        let fn_name = format_path_with_name(&fn_doc.path, &fn_doc.name);
        push_item_heading(markdown, "function", &fn_name);
        markdown.push_str(&format!("**Visibility:** `{}`\n\n", fn_doc.visibility));
        markdown.push_str(&format!("**Signature:** `{}`\n\n", fn_doc.signature));
        push_doc_comment(markdown, &fn_doc.doc_comment);
    }
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
