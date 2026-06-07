use syn::{Expr, Ident, Path};

pub fn parse_ident_expr(expr: &Expr, arg_name: &str) -> syn::Result<Ident> {
    if let Expr::Path(path) = expr {
        if let Some(ident) = path.path.get_ident() {
            return Ok(ident.clone());
        }
    }

    Err(syn::Error::new_spanned(
        expr,
        format!("{} must be an identifier", arg_name),
    ))
}

pub fn parse_path_expr(expr: &Expr, arg_name: &str) -> syn::Result<Path> {
    if let Expr::Path(path) = expr {
        return Ok(path.path.clone());
    }

    Err(syn::Error::new_spanned(
        expr,
        format!("{} must be a function path", arg_name),
    ))
}
