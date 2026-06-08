use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, parse_macro_input};

use crate::enum_impl;
use crate::struct_impl;
use crate::util;

// TODO: This doesnt look as generic as it could be. do we really care about this?
pub fn derive_reflectable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let color = match util::parse_reflect_color(&input.attrs) {
        Ok(color) => color,
        Err(err) => return err.to_compile_error().into(),
    };
    let color_expr = match &color {
        Some(lit) => quote! { Some(#lit) },
        None => quote! { None },
    };

    let expanded = match &input.data {
        Data::Struct(data_struct) => struct_impl::generate_struct_impl(
            name,
            &impl_generics,
            &ty_generics,
            &where_clause,
            data_struct,
            &color_expr,
        ),
        Data::Enum(data_enum) => enum_impl::generate_enum_impl(
            name,
            &impl_generics,
            &ty_generics,
            &where_clause,
            data_enum,
            &color_expr,
        ),
        Data::Union(_) => {
            return syn::Error::new_spanned(&input, "Reflectable cannot be derived for unions")
                .to_compile_error()
                .into();
        }
    };

    expanded.into()
}
