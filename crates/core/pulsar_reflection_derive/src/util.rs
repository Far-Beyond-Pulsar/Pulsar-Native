use syn::{Attribute, Expr, LitStr, Path};

pub fn parse_path_expr(expr: &Expr, arg_name: &str) -> syn::Result<Path> {
    if let Expr::Path(path) = expr {
        return Ok(path.path.clone());
    }

    Err(syn::Error::new_spanned(
        expr,
        format!("{} must be a function path", arg_name),
    ))
}

/// Look for `#[reflect(color = "#RRGGBB")]` among the given attributes and
/// return the declared color literal, if any. This is the declarative path
/// for registering a type's display color as part of its reflection metadata
/// — the single source of truth consumers (e.g. node-graph pin renderers)
/// should read from, rather than deriving their own type-to-color mapping.
pub fn parse_reflect_color(attrs: &[Attribute]) -> syn::Result<Option<LitStr>> {
    for attr in attrs {
        if !attr.path().is_ident("reflect") {
            continue;
        }

        let mut found = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("color") {
                let value = meta.value()?;
                let lit: LitStr = value.parse()?;
                found = Some(lit);
                Ok(())
            } else {
                Err(meta.error("unsupported #[reflect(...)] argument"))
            }
        })?;

        if found.is_some() {
            return Ok(found);
        }
    }

    Ok(None)
}

pub fn parse_lit_str_expr(expr: &Expr, arg_name: &str) -> syn::Result<LitStr> {
    if let Expr::Lit(syn::ExprLit {
        lit: syn::Lit::Str(lit_str),
        ..
    }) = expr
    {
        return Ok(lit_str.clone());
    }

    Err(syn::Error::new_spanned(
        expr,
        format!("{} must be a string literal", arg_name),
    ))
}
