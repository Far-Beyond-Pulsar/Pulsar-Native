use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    parse::Parse, Attribute, Data, DeriveInput, Fields, Ident, Type,
};

use crate::cell::generate_scene_column_set;
use crate::gpu::generate_gpu_column_set;

// ── #[gpu] attribute parsing ──────────────────────────────────────────────

pub struct GpuAttr {
    pub mirror_mode: Option<MirrorModeAttr>,
}

pub enum MirrorModeAttr {
    DirtyTracked,
    Once,
}

impl Parse for GpuAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(GpuAttr { mirror_mode: None });
        }
        let _: Ident = input.parse()?;
        let _: syn::Token![=] = input.parse()?;
        let mode: Ident = input.parse()?;
        let mode = match mode.to_string().as_str() {
            "DirtyTracked" => MirrorModeAttr::DirtyTracked,
            "Once" => MirrorModeAttr::Once,
            _ => {
                return Err(syn::Error::new(
                    mode.span(),
                    "expected DirtyTracked or Once",
                ))
            }
        };
        Ok(GpuAttr {
            mirror_mode: Some(mode),
        })
    }
}

// ── Per-field metadata ────────────────────────────────────────────────────

pub struct FieldInfo {
    pub ident: Ident,
    pub ty: Type,
    pub is_gpu: bool,
    pub mirror_mode: MirrorModeAttr,
}

// ── Entry point ───────────────────────────────────────────────────────────

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(ds) => match &ds.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "SceneStore requires named fields",
                ))
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "SceneStore only supports structs",
            ))
        }
    };

    let mut field_infos: Vec<FieldInfo> = Vec::new();
    for field in fields {
        let ident = field.ident.as_ref().unwrap().clone();
        let ty = field.ty.clone();
        let mut is_gpu = false;
        let mut mirror_mode = MirrorModeAttr::DirtyTracked;

        for attr in &field.attrs {
            if attr.path().is_ident("gpu") {
                is_gpu = true;
                if let Ok(gpu_attr) = attr.parse_args::<GpuAttr>() {
                    if let Some(mode) = gpu_attr.mirror_mode {
                        mirror_mode = mode;
                    }
                }
            }
        }

        field_infos.push(FieldInfo {
            ident,
            ty,
            is_gpu,
            mirror_mode,
        });
    }

    if field_infos.is_empty() {
        return Err(syn::Error::new_spanned(
            name,
            "SceneStore requires at least one field",
        ));
    }

    let field_types: Vec<&Type> = field_infos.iter().map(|f| &f.ty).collect();
    let gpu_fields: Vec<&FieldInfo> = field_infos.iter().filter(|f| f.is_gpu).collect();

    let pod_impl = generate_pod_impl(name, &impl_generics, &ty_generics, where_clause, &field_types);
    let has_type_token =
        generate_has_type_token(name, &impl_generics, &ty_generics, where_clause);
    let scene_column_set =
        generate_scene_column_set(name, &impl_generics, &ty_generics, where_clause, &field_infos);
    let gpu_column_set =
        generate_gpu_column_set(name, &impl_generics, &ty_generics, where_clause, &gpu_fields);

    Ok(quote! {
        #pod_impl
        #has_type_token
        #scene_column_set
        #gpu_column_set
    })
}

// ── Pod impl ──────────────────────────────────────────────────────────────

fn generate_pod_impl(
    name: &Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
    field_types: &[&Type],
) -> TokenStream {
    let pod_bounds: Vec<_> = field_types
        .iter()
        .map(|ty| {
            quote! { #ty: ::pulsar_scenedb::page::Pod }
        })
        .collect();

    let mut wc: syn::WhereClause = where_clause
        .cloned()
        .unwrap_or_else(|| syn::WhereClause {
            where_token: Default::default(),
            predicates: syn::punctuated::Punctuated::new(),
        });

    for bound in &pod_bounds {
        let pred: syn::WherePredicate = syn::parse_quote! { #bound };
        wc.predicates.push(pred);
    }

    quote! {
        unsafe impl #impl_generics ::pulsar_scenedb::page::Pod for #name #ty_generics #wc {}
    }
}

// ── HasTypeToken impl ─────────────────────────────────────────────────────

fn generate_has_type_token(
    name: &Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream {
    quote! {
        impl #impl_generics ::pulsar_scenedb::token::HasTypeToken for #name #ty_generics #where_clause {
            fn type_token() -> ::pulsar_scenedb::token::TypeToken {
                ::pulsar_scenedb::token::TypeToken::of::<Self>()
            }
        }
    }
}
