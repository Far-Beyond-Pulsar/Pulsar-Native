//! Which Rust types a `#[blueprint]` function may use to also become a script native.

/// Types a `#[blueprint]` function may use (by value) to also become a
/// script VM native. Anything else keeps the node Blueprint-only.
const SCRIPT_NATIVE_TYPES: &[&str] = &[
    "bool", "i8", "i16", "i32", "i64", "isize", "i128", "u8", "u16", "u32", "u64", "usize", "u128",
    "f32", "f64", "String",
];

/// Types usable as a map key in a script native.
const SCRIPT_KEY_TYPES: &[&str] = &[
    "bool", "i8", "i16", "i32", "i64", "isize", "i128", "u8", "u16", "u32", "u64", "usize", "u128",
    "String",
];

pub(crate) fn is_ident_in(ty: &syn::Type, names: &[&str]) -> bool {
    matches!(ty, syn::Type::Path(path) if path.path.get_ident().is_some_and(|i| names.contains(&i.to_string().as_str())))
}

/// The generic arguments of `Name<..>`, when `ty` is that path.
pub(crate) fn generic_args<'t>(ty: &'t syn::Type, name: &str) -> Option<Vec<&'t syn::Type>> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if path.qself.is_some() || segment.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    args.args
        .iter()
        .map(|arg| match arg {
            syn::GenericArgument::Type(ty) => Some(ty),
            _ => None,
        })
        .collect()
}

/// Whether a value of `ty` is a script value: a scalar, `String`, `()`, an `Option`, a
/// `Vec`, fixed array, non-empty tuple (up to six) or `HashMap`/`BTreeMap`
/// (keyed by a scalar or `String`) of script values.
pub(crate) fn is_script_native_type(ty: &syn::Type) -> bool {
    match ty {
        syn::Type::Path(_) if is_ident_in(ty, SCRIPT_NATIVE_TYPES) => true,
        syn::Type::Path(_) => {
            if let Some([element]) = generic_args(ty, "Vec").as_deref() {
                return is_script_native_type(element);
            }
            if let Some([item]) = generic_args(ty, "Option").as_deref() {
                return is_script_native_type(item);
            }
            for set in ["HashSet", "BTreeSet"] {
                if let Some([key]) = generic_args(ty, set).as_deref() {
                    return is_ident_in(key, SCRIPT_KEY_TYPES);
                }
            }
            for map in ["HashMap", "BTreeMap"] {
                if let Some([key, value]) = generic_args(ty, map).as_deref() {
                    return is_ident_in(key, SCRIPT_KEY_TYPES) && is_script_native_type(value);
                }
            }
            false
        }
        syn::Type::Array(array) => is_script_native_type(&array.elem),
        syn::Type::Tuple(tuple) => {
            tuple.elems.len() <= 6 && tuple.elems.iter().all(is_script_native_type)
        }
        syn::Type::Paren(paren) => is_script_native_type(&paren.elem),
        _ => false,
    }
}

/// How a parameter of `ty` is received: the owned script type the native
/// slot holds, and whether the function borrows it (`&str`, `&[T]`,
/// `&Vec<T>`, `&[T; N]`, `&HashMap<..>`). `None`: not representable.
pub(crate) fn native_slot(ty: &syn::Type) -> Option<(syn::Type, bool)> {
    if is_script_native_type(ty) {
        return Some((ty.clone(), false));
    }
    let syn::Type::Reference(reference) = ty else {
        return None;
    };
    if reference.mutability.is_some() {
        return None;
    }
    match &*reference.elem {
        syn::Type::Path(path) if path.path.is_ident("str") => {
            Some((syn::parse_quote!(::std::string::String), true))
        }
        syn::Type::Slice(slice) if is_script_native_type(&slice.elem) => {
            let element = &slice.elem;
            Some((syn::parse_quote!(::std::vec::Vec<#element>), true))
        }
        owned if is_script_native_type(owned) => Some((owned.clone(), true)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn ty(src: &str) -> syn::Type {
        syn::parse_str(src).unwrap()
    }

    fn ty_string(ty: &syn::Type) -> String {
        quote!(#ty).to_string().replace(' ', "")
    }

    #[test]
    fn script_native_types_accept_scalars_and_containers_of_them() {
        for ok in [
            "i32",
            "f64",
            "bool",
            "String",
            "()",
            "Vec<i32>",
            "Option<String>",
            "[u8; 4]",
            "(i32, f32)",
            "HashMap<String, Vec<f64>>",
            "BTreeMap<i64, bool>",
            "HashSet<u8>",
            "Vec<Option<Vec<bool>>>",
        ] {
            assert!(
                is_script_native_type(&ty(ok)),
                "{ok} should be a script type"
            );
        }
    }

    #[test]
    fn script_native_types_reject_unsupported_shapes() {
        for bad in [
            "Foo",
            "Vec<Foo>",
            "HashMap<f64, i32>",
            "HashMap<String, Foo>",
            "HashSet<f32>",
            "&str",
            "*const i32",
            "(i32, i32, i32, i32, i32, i32, i32)",
            "Box<i32>",
        ] {
            assert!(
                !is_script_native_type(&ty(bad)),
                "{bad} should not be a script type"
            );
        }
    }

    #[test]
    fn native_slot_owns_borrowed_parameters_and_flags_borrow() {
        let (slot, borrowed) = native_slot(&ty("&str")).unwrap();
        assert_eq!(ty_string(&slot), "::std::string::String");
        assert!(borrowed);

        let (slot, borrowed) = native_slot(&ty("&[i32]")).unwrap();
        assert_eq!(ty_string(&slot), "::std::vec::Vec<i32>");
        assert!(borrowed);

        let (slot, borrowed) = native_slot(&ty("&Vec<f32>")).unwrap();
        assert_eq!(ty_string(&slot), "Vec<f32>");
        assert!(borrowed);

        let (slot, borrowed) = native_slot(&ty("u8")).unwrap();
        assert_eq!(ty_string(&slot), "u8");
        assert!(!borrowed);
    }

    #[test]
    fn native_slot_rejects_mutable_refs_and_unknown_types() {
        assert!(native_slot(&ty("&mut String")).is_none());
        assert!(native_slot(&ty("&Foo")).is_none());
        assert!(native_slot(&ty("&[Foo]")).is_none());
        assert!(native_slot(&ty("Foo")).is_none());
    }
}
