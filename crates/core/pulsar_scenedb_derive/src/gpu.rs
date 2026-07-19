use proc_macro2::TokenStream;
use quote::quote;
use syn::{Ident, Type};

use crate::scene_store::{FieldInfo, MirrorModeAttr};

pub fn generate_gpu_column_set(
    name: &Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    gpu_fields: &[&FieldInfo],
) -> TokenStream {
    if gpu_fields.is_empty() {
        return quote! {
            impl #impl_generics ::pulsar_scenedb::gpu::scene_store::GpuColumnSet for #name #ty_generics #where_clause {
                fn gpu_columns() -> Vec<::pulsar_scenedb::gpu::scene_store::GpuColumnDesc> {
                    Vec::new()
                }
                fn write_gpu(
                    _store: &::pulsar_scenedb::gpu::scene_store::SceneGpuStore,
                    _id: ::pulsar_scenedb::gpu::scene_store::CellId,
                    _cell: &mut ::pulsar_scenedb::cell::CellStorage,
                    _handle: ::pulsar_scenedb::handle::Handle,
                    _data: &Self,
                    _phase: &impl ::pulsar_scenedb::gpu::phase::SimulateWitness,
                ) {
                }
            }
        };
    }

    let column_descs: Vec<_> = gpu_fields
        .iter()
        .map(|f| {
            let field_name = f.ident.to_string();
            let field_ident = &f.ident;
            let field_type = &f.ty;
            let mirror_mode = match f.mirror_mode {
                MirrorModeAttr::DirtyTracked => {
                    quote! { ::pulsar_scenedb::gpu::scene_store::MirrorMode::DirtyTracked }
                }
                MirrorModeAttr::Once => {
                    quote! { ::pulsar_scenedb::gpu::scene_store::MirrorMode::Once }
                }
            };
            quote! {
                ::pulsar_scenedb::gpu::scene_store::GpuColumnDesc {
                    field_token: ::pulsar_scenedb::token::TypeToken::of::<#field_type>(),
                    field_offset: ::std::mem::offset_of!(#name, #field_ident),
                    mode: #mirror_mode,
                    buffer_name: #field_name,
                }
            }
        })
        .collect();

    let write_arms: Vec<_> = gpu_fields
        .iter()
        .map(|f| {
            let field_name = f.ident.to_string();
            let field_ident = &f.ident;
            let field_type = &f.ty;
            quote! {
                #field_name => {
                    let row = cell.row_of(handle).unwrap_or_else(|| {
                        panic!("write_gpu: handle {:?} not found in cell", handle);
                    }) as usize;
                    if let Some(col) = cell.column_for_mut::<#field_type>() {
                        col[row] = data.#field_ident;
                    }
                    let comp_id = ::pulsar_scenedb::component::component_id::<#field_type>();
                    store.mark_column_dirty(id, comp_id, row as u32);
                }
            }
        })
        .collect();

    quote! {
        impl #impl_generics ::pulsar_scenedb::gpu::scene_store::GpuColumnSet for #name #ty_generics #where_clause {
            fn gpu_columns() -> Vec<::pulsar_scenedb::gpu::scene_store::GpuColumnDesc> {
                vec![
                    #(#column_descs),*
                ]
            }
            fn write_gpu(
                store: &::pulsar_scenedb::gpu::scene_store::SceneGpuStore,
                id: ::pulsar_scenedb::gpu::scene_store::CellId,
                cell: &mut ::pulsar_scenedb::cell::CellStorage,
                handle: ::pulsar_scenedb::handle::Handle,
                data: &Self,
                _phase: &impl ::pulsar_scenedb::gpu::phase::SimulateWitness,
            ) {
                let descs = Self::gpu_columns();
                for desc in &descs {
                    match desc.buffer_name {
                        #(#write_arms)*
                        _ => {}
                    }
                }
            }
        }
    }
}
