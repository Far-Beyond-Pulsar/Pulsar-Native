use crate::{
    TypeRef, TypeAstNode, StructAsset, EnumAsset, TraitAsset, AliasAsset,
    Visibility, TypeSystemError, Result,
};

/// Renders a type reference to Rust code
pub fn render_type_ref(type_ref: &TypeRef) -> String {
    match type_ref {
        TypeRef::Primitive { name } => name.clone(),
        TypeRef::Path { path } => path.clone(),
        TypeRef::AliasRef { alias } => {
            // Convert snake_case to PascalCase for type name
            to_pascal_case(alias)
        }
    }
}

/// Renders a type AST node to Rust code
pub fn render_ast_node(node: &TypeAstNode) -> String {
    match node {
        TypeAstNode::Primitive { name } => name.clone(),
        TypeAstNode::Path { path } => path.clone(),
        TypeAstNode::AliasRef { alias } => to_pascal_case(alias),
        TypeAstNode::Constructor { name, params, lifetimes, const_generics } => {
            let mut result = name.clone();

            // Add lifetimes and const generics
            let mut generic_params = Vec::new();

            for lifetime in lifetimes {
                generic_params.push(format!("'{}", lifetime));
            }

            for param in params {
                generic_params.push(render_ast_node(param));
            }

            for const_gen in const_generics {
                generic_params.push(const_gen.clone());
            }

            if !generic_params.is_empty() {
                result.push('<');
                result.push_str(&generic_params.join(", "));
                result.push('>');
            }

            result
        }
        TypeAstNode::Tuple { elements } => {
            if elements.is_empty() {
                "()".to_string()
            } else {
                let parts: Vec<_> = elements.iter().map(render_ast_node).collect();
                format!("({})", parts.join(", "))
            }
        }
        TypeAstNode::FnPointer { params, return_type } => {
            let param_str = if params.is_empty() {
                "()".to_string()
            } else {
                let parts: Vec<_> = params.iter().map(render_ast_node).collect();
                format!("({})", parts.join(", "))
            };
            let return_str = render_ast_node(return_type);
            format!("fn{} -> {}", param_str, return_str)
        }
    }
}

/// Renders visibility modifier
pub fn render_visibility(vis: Visibility) -> &'static str {
    match vis {
        Visibility::Public => "pub ",
        Visibility::Private => "",
        Visibility::Crate => "pub(crate) ",
        Visibility::Super => "pub(super) ",
    }
}

/// Converts snake_case to PascalCase
pub fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    first.to_uppercase().collect::<String>() + chars.as_str()
                }
            }
        })
        .collect()
}

/// Generates Rust code for a struct
pub fn generate_struct(asset: &StructAsset) -> Result<String> {
    let mut code = String::new();

    // Header
    code.push_str("// AUTO-GENERATED. DO NOT EDIT MANUALLY.\n");
    code.push_str("#![allow(dead_code)]\n\n");

    // Documentation
    if let Some(desc) = &asset.description {
        code.push_str(&format!("/// {}\n", desc));
    }

    // Struct definition
    code.push_str(&format!(
        "{}struct {} {{\n",
        render_visibility(asset.visibility),
        to_pascal_case(&asset.name)
    ));

    // Fields
    for field in &asset.fields {
        if let Some(doc) = &field.doc {
            code.push_str(&format!("    /// {}\n", doc));
        }
        code.push_str(&format!(
            "    {}{}: {},\n",
            render_visibility(field.visibility),
            field.name,
            render_type_ref(&field.type_ref)
        ));
    }

    code.push_str("}\n");

    Ok(code)
}

/// Generates Rust code for an enum
pub fn generate_enum(asset: &EnumAsset) -> Result<String> {
    let mut code = String::new();

    // Header
    code.push_str("// AUTO-GENERATED. DO NOT EDIT MANUALLY.\n");
    code.push_str("#![allow(dead_code)]\n\n");

    // Documentation
    if let Some(desc) = &asset.description {
        code.push_str(&format!("/// {}\n", desc));
    }

    // Enum definition
    code.push_str(&format!(
        "{}enum {} {{\n",
        render_visibility(asset.visibility),
        to_pascal_case(&asset.name)
    ));

    // Variants
    for variant in &asset.variants {
        if let Some(doc) = &variant.doc {
            code.push_str(&format!("    /// {}\n", doc));
        }

        match &variant.payload {
            None => {
                code.push_str(&format!("    {},\n", variant.name));
            }
            Some(payload) => {
                code.push_str(&format!(
                    "    {}({}),\n",
                    variant.name,
                    render_type_ref(payload)
                ));
            }
        }
    }

    code.push_str("}\n");

    Ok(code)
}

/// Generates Rust code for a trait
pub fn generate_trait(asset: &TraitAsset) -> Result<String> {
    let mut code = String::new();

    // Header
    code.push_str("// AUTO-GENERATED. DO NOT EDIT MANUALLY.\n");
    code.push_str("#![allow(dead_code)]\n\n");

    // Documentation
    if let Some(desc) = &asset.description {
        code.push_str(&format!("/// {}\n", desc));
    }

    // Trait definition
    code.push_str(&format!("pub trait {} {{\n", to_pascal_case(&asset.name)));

    // Methods
    for method in &asset.methods {
        if let Some(doc) = &method.doc {
            code.push_str(&format!("    /// {}\n", doc));
        }

        // Method signature
        code.push_str("    fn ");
        code.push_str(&method.name);
        code.push('(');

        // Parameters
        let params: Vec<String> = method
            .signature
            .params
            .iter()
            .map(|p| format!("{}: {}", p.name, render_type_ref(&p.type_ref)))
            .collect();
        code.push_str(&params.join(", "));

        code.push(')');

        // Return type
        let return_type = render_type_ref(&method.signature.return_type);
        if return_type != "()" {
            code.push_str(" -> ");
            code.push_str(&return_type);
        }

        // Body
        if let Some(body) = &method.default_body {
            code.push_str(" {\n");
            // Indent body
            for line in body.lines() {
                code.push_str("        ");
                code.push_str(line);
                code.push('\n');
            }
            code.push_str("    }\n");
        } else {
            code.push_str(";\n");
        }

        code.push('\n');
    }

    code.push_str("}\n");

    Ok(code)
}

/// Generates Rust code for a type alias
pub fn generate_alias(asset: &AliasAsset) -> Result<String> {
    let mut code = String::new();

    // Header
    code.push_str("// AUTO-GENERATED. DO NOT EDIT MANUALLY.\n");
    code.push_str("#![allow(dead_code)]\n\n");

    // Documentation
    if let Some(desc) = &asset.description {
        code.push_str(&format!("/// {}\n", desc));
    }

    // Type alias
    code.push_str(&format!(
        "pub type {} = {};\n",
        to_pascal_case(&asset.name),
        render_ast_node(&asset.ast)
    ));

    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("hello_world"), "HelloWorld");
        assert_eq!(to_pascal_case("foo"), "Foo");
        assert_eq!(to_pascal_case("boxed_item"), "BoxedItem");
    }

    #[test]
    fn test_render_type_ref() {
        assert_eq!(render_type_ref(&TypeRef::primitive("i32")), "i32");
        assert_eq!(
            render_type_ref(&TypeRef::path("std::time::Duration")),
            "std::time::Duration"
        );
        assert_eq!(render_type_ref(&TypeRef::alias("boxed_item")), "BoxedItem");
    }
}
