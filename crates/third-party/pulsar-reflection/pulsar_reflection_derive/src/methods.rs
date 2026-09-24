//! `#[reflect_methods]`: register an impl block's `#[reflect_method]`
//! methods in `pulsar_reflection::methods`.
//!
//! For each marked method this generates an `invoke` shim (an associated
//! fn, so `Self` resolves naturally) and a `ReflectedMethod` entry; the
//! block's entries are one associated const, submitted to `inventory` as a
//! `TypeMethodRegistration`. Signature handling lives in
//! `pulsar_reflection_codegen`, shared with other crates' method macros.

use proc_macro2::{Span, TokenStream};
use pulsar_reflection_codegen::{reflected_registration, take_marked, MethodSpec};
use quote::quote;
use syn::{parse_macro_input, ItemImpl};

pub fn reflect_methods(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    if !attr.is_empty() {
        let err = syn::Error::new(Span::call_site(), "#[reflect_methods] takes no arguments");
        return err.to_compile_error().into();
    }
    let item = parse_macro_input!(item as ItemImpl);
    match expand(item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if let Some((_, path, _)) = &item.trait_ {
        return Err(syn::Error::new_spanned(
            path,
            "#[reflect_methods] goes on an inherent impl block, not a trait impl",
        ));
    }
    if !item.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item.generics,
            "#[reflect_methods] needs a concrete type: methods are keyed by TypeId",
        ));
    }

    let specs = take_marked(&mut item.items, "reflect_method")
        .into_iter()
        .map(|(func, marker)| MethodSpec::parse(func, &marker, 0))
        .collect::<syn::Result<Vec<_>>>()?;
    if specs.is_empty() {
        return Ok(quote!(#item));
    }

    let registration = reflected_registration(&quote!(::pulsar_reflection), &item.self_ty, &specs);
    Ok(quote! {
        #item
        #registration
    })
}
