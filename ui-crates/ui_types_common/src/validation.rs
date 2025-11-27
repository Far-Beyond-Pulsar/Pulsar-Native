use regex::Regex;
use std::collections::{HashMap, HashSet};
use crate::{
    TypeRef, TypeAstNode, TypeIndex, TypeKind, AliasAsset,
    StructAsset, EnumAsset, TraitAsset, TypeSystemError, Result, PRIMITIVES, CONSTRUCTORS,
};

lazy_static::lazy_static! {
    static ref NAME_REGEX: Regex = Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap();
}

/// Validates a type name
pub fn validate_name(name: &str) -> Result<()> {
    if !NAME_REGEX.is_match(name) {
        return Err(TypeSystemError::InvalidName(name.to_string()));
    }
    Ok(())
}

/// Validates that a name doesn't collide with existing types
pub fn validate_no_collision(index: &TypeIndex, name: &str, kind: Option<TypeKind>) -> Result<()> {
    if let Some(existing_kind) = index.has_collision(name) {
        // Allow if it's the same type kind (updating existing type)
        if let Some(k) = kind {
            if k == existing_kind {
                return Ok(());
            }
        }
        return Err(TypeSystemError::NameCollision(
            name.to_string(),
            existing_kind.plural().to_string(),
        ));
    }
    Ok(())
}

/// Validates a type reference
pub fn validate_type_ref(type_ref: &TypeRef, index: &TypeIndex) -> Result<()> {
    match type_ref {
        TypeRef::Primitive { name } => {
            if !PRIMITIVES.contains(&name.as_str()) {
                return Err(TypeSystemError::InvalidTypeReference(format!(
                    "{} is not a valid primitive type",
                    name
                )));
            }
        }
        TypeRef::Path { .. } => {
            // Path types are assumed valid - they reference external types
        }
        TypeRef::AliasRef { alias } => {
            if !index.exists(TypeKind::Alias, alias) {
                return Err(TypeSystemError::AliasNotFound(alias.clone()));
            }
        }
    }
    Ok(())
}

/// Validates generic arity for constructors
pub fn validate_constructor_arity(name: &str, params: &[TypeAstNode]) -> Result<()> {
    let expected = match name {
        "Box" | "Arc" | "Rc" | "Option" | "Vec" | "Pin" | "Cell" | "RefCell" | "Mutex" | "RwLock" => 1,
        "Result" | "Cow" => 2,
        "HashMap" | "HashSet" => {
            if params.len() == 1 || params.len() == 2 {
                return Ok(());
            }
            2
        }
        "PhantomData" => 1,
        _ => return Ok(()), // Unknown constructors allowed for extensibility
    };

    if params.len() != expected {
        return Err(TypeSystemError::InvalidArity {
            constructor: name.to_string(),
            expected,
            actual: params.len(),
        });
    }

    Ok(())
}

/// Validates a type AST node
pub fn validate_ast_node(node: &TypeAstNode, index: &TypeIndex) -> Result<()> {
    match node {
        TypeAstNode::Primitive { name } => {
            if !PRIMITIVES.contains(&name.as_str()) {
                return Err(TypeSystemError::InvalidTypeReference(format!(
                    "{} is not a valid primitive type",
                    name
                )));
            }
        }
        TypeAstNode::Path { .. } => {
            // Path types are assumed valid
        }
        TypeAstNode::AliasRef { alias } => {
            if !index.exists(TypeKind::Alias, alias) {
                return Err(TypeSystemError::AliasNotFound(alias.clone()));
            }
        }
        TypeAstNode::Constructor { name, params, .. } => {
            validate_constructor_arity(name, params)?;
            for param in params {
                validate_ast_node(param, index)?;
            }
        }
        TypeAstNode::Tuple { elements } => {
            for elem in elements {
                validate_ast_node(elem, index)?;
            }
        }
        TypeAstNode::FnPointer { params, return_type } => {
            for param in params {
                validate_ast_node(param, index)?;
            }
            validate_ast_node(return_type, index)?;
        }
    }
    Ok(())
}

/// Detects circular dependencies in aliases
pub fn detect_alias_cycles(alias: &AliasAsset, index: &TypeIndex) -> Result<()> {
    let mut visited = HashSet::new();
    let mut stack = HashSet::new();

    fn visit(
        alias_name: &str,
        node: &TypeAstNode,
        index: &TypeIndex,
        visited: &mut HashSet<String>,
        stack: &mut HashSet<String>,
    ) -> Result<()> {
        if stack.contains(alias_name) {
            return Err(TypeSystemError::CircularDependency(format!(
                "Circular dependency involving alias '{}'",
                alias_name
            )));
        }

        if visited.contains(alias_name) {
            return Ok(());
        }

        stack.insert(alias_name.to_string());

        match node {
            TypeAstNode::AliasRef { alias: ref_alias } => {
                // Check if referenced alias exists and visit it
                if let Some(_entry) = index.get(TypeKind::Alias, ref_alias) {
                    // For full cycle detection, we'd need to load the referenced alias
                    // For now, we just mark it as visited
                    visit(ref_alias, node, index, visited, stack)?;
                }
            }
            TypeAstNode::Constructor { params, .. } => {
                for param in params {
                    visit(alias_name, param, index, visited, stack)?;
                }
            }
            TypeAstNode::Tuple { elements } => {
                for elem in elements {
                    visit(alias_name, elem, index, visited, stack)?;
                }
            }
            TypeAstNode::FnPointer { params, return_type } => {
                for param in params {
                    visit(alias_name, param, index, visited, stack)?;
                }
                visit(alias_name, return_type, index, visited, stack)?;
            }
            _ => {}
        }

        stack.remove(alias_name);
        visited.insert(alias_name.to_string());

        Ok(())
    }

    visit(&alias.name, &alias.ast, index, &mut visited, &mut stack)
}

/// Validates a struct asset
pub fn validate_struct(asset: &StructAsset, index: &TypeIndex) -> Result<()> {
    validate_name(&asset.name)?;
    validate_no_collision(index, &asset.name, Some(TypeKind::Struct))?;

    for field in &asset.fields {
        validate_name(&field.name)?;
        validate_type_ref(&field.type_ref, index)?;
    }

    Ok(())
}

/// Validates an enum asset
pub fn validate_enum(asset: &EnumAsset, index: &TypeIndex) -> Result<()> {
    validate_name(&asset.name)?;
    validate_no_collision(index, &asset.name, Some(TypeKind::Enum))?;

    for variant in &asset.variants {
        validate_name(&variant.name)?;
        if let Some(payload) = &variant.payload {
            validate_type_ref(payload, index)?;
        }
    }

    Ok(())
}

/// Validates a trait asset
pub fn validate_trait(asset: &TraitAsset, index: &TypeIndex) -> Result<()> {
    validate_name(&asset.name)?;
    validate_no_collision(index, &asset.name, Some(TypeKind::Trait))?;

    for method in &asset.methods {
        validate_name(&method.name)?;

        for param in &method.signature.params {
            validate_name(&param.name)?;
            validate_type_ref(&param.type_ref, index)?;
        }

        validate_type_ref(&method.signature.return_type, index)?;
    }

    Ok(())
}

/// Validates an alias asset
pub fn validate_alias(asset: &AliasAsset, index: &TypeIndex) -> Result<()> {
    validate_name(&asset.name)?;
    validate_no_collision(index, &asset.name, Some(TypeKind::Alias))?;
    validate_ast_node(&asset.ast, index)?;
    detect_alias_cycles(asset, index)?;
    Ok(())
}
