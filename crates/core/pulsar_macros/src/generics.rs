//! Replacing a generic node's type parameters with `()` so wrapper sizes can be measured.

// ── Generic type-param substitution ──────────────────────────────────────────
//
// Replace every unbound type parameter (e.g. `T`) in a `syn::Type` with `()`.
// Used so the `#[blueprint]` macro can call `size_of::<SubstitutedType>()` in
// a const context to record the *wrapper* size at compile time:
//
//   Vec<T>  →  Vec<()>   →  size_of = 24  (wrapper size, T-independent)
//   T       →  ()        →  size_of = 0   (signal: resolve via graph traversal)
//
// No lookup table, no hardcoded constants — the Rust compiler does the work.

pub(crate) fn substitute_generics_with_unit(
    ty: &syn::Type,
    params: &std::collections::HashSet<String>,
) -> syn::Type {
    use syn::{GenericArgument, PathArguments, Type};
    match ty {
        Type::Path(type_path) => {
            // Bare generic parameter with no arguments (e.g. just `T`)?
            if type_path.qself.is_none() && type_path.path.segments.len() == 1 {
                let seg = &type_path.path.segments[0];
                if matches!(seg.arguments, PathArguments::None)
                    && params.contains(&seg.ident.to_string())
                {
                    return syn::parse_quote!(());
                }
            }
            // Recurse into angle-bracketed generic arguments (e.g. `Vec<T>`).
            let mut new_tp = type_path.clone();
            for seg in new_tp.path.segments.iter_mut() {
                if let PathArguments::AngleBracketed(ref mut ab) = seg.arguments {
                    let mut new_args = syn::punctuated::Punctuated::new();
                    for arg in ab.args.iter() {
                        let new_arg = if let GenericArgument::Type(inner) = arg {
                            GenericArgument::Type(substitute_generics_with_unit(inner, params))
                        } else {
                            arg.clone()
                        };
                        new_args.push(new_arg);
                    }
                    ab.args = new_args;
                }
            }
            Type::Path(new_tp)
        }
        Type::Tuple(tup) => {
            let mut new_tup = tup.clone();
            let mut new_elems = syn::punctuated::Punctuated::new();
            for elem in tup.elems.iter() {
                new_elems.push(substitute_generics_with_unit(elem, params));
            }
            new_tup.elems = new_elems;
            Type::Tuple(new_tup)
        }
        // References and other compound types: recurse where possible, else clone.
        Type::Reference(r) => {
            let mut new_r = r.clone();
            new_r.elem = Box::new(substitute_generics_with_unit(&r.elem, params));
            Type::Reference(new_r)
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use std::collections::HashSet;

    fn ty(src: &str) -> syn::Type {
        syn::parse_str(src).unwrap()
    }

    fn ty_string(ty: &syn::Type) -> String {
        quote!(#ty).to_string().replace(' ', "")
    }

    #[test]
    fn generic_params_are_replaced_by_unit_recursively() {
        let params: HashSet<String> = ["T".to_string()].into();
        for (input, expected) in [
            ("T", "()"),
            ("Vec<T>", "Vec<()>"),
            ("(T, i32)", "((),i32)"),
            ("&T", "&()"),
            ("HashMap<String, Vec<T>>", "HashMap<String,Vec<()>>"),
            ("U", "U"),
            ("[T; 2]", "[T;2]"),
        ] {
            assert_eq!(
                ty_string(&substitute_generics_with_unit(&ty(input), &params)),
                expected,
                "{input}"
            );
        }
    }
}
