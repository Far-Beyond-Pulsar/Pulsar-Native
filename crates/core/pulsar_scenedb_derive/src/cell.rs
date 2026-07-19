use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Type};

use crate::scene_store::FieldInfo;

pub fn generate_scene_column_set(
    name: &Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    field_infos: &[FieldInfo],
) -> TokenStream {
    let cell_type_name = name.to_string();

    let cell_entries: Vec<_> = field_infos
        .iter()
        .map(|f| {
            let ty = &f.ty;
            quote! {
                .with(::pulsar_scenedb::token::TypeToken::of::<#ty>())
            }
        })
        .collect();

    quote! {
        impl #impl_generics ::pulsar_scenedb::cell_type::SceneColumnSet for #name #ty_generics #where_clause {
            fn cell_type() -> ::pulsar_scenedb::cell_type::RegisteredCellType {
                ::pulsar_scenedb::cell_type::CellType::new(#cell_type_name)
                    #(#cell_entries)*
                    .build()
                    .expect("SceneColumnSet cell_type: CellType::build failed (duplicate columns or stride overflow)")
            }
        }
    }
}
