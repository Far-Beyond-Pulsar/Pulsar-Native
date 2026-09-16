use quote::quote;
use syn::{Field, Ident, punctuated::Punctuated};

pub fn generate_field_infos(
    fields: &Punctuated<Field, syn::token::Comma>,
    struct_name: &Ident,
) -> proc_macro2::TokenStream {
    let field_info_items: Vec<_> = fields
        .iter()
        .map(|field| {
            let field_name = field.ident.as_ref().unwrap();
            let field_name_str = field_name.to_string();
            let field_type = &field.ty;

            let offset_expr = quote! {
                std::mem::offset_of!(#struct_name, #field_name)
            };

            quote! {
                ::pulsar_reflection::FieldInfo {
                    name: #field_name_str,
                    type_info: <#field_type as ::pulsar_reflection::Reflectable>::type_info(),
                    offset: #offset_expr,
                }
            }
        })
        .collect();

    if field_info_items.is_empty() {
        quote! { [] }
    } else {
        quote! {
            [#(#field_info_items),*]
        }
    }
}
