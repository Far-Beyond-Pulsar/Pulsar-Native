//! `#[engine_class(...)]`: the attribute form that adds derives and then
//! delegates to the `EngineClass` derive. Entry point: [`expand`].

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Expr, ItemStruct, Lit, Meta, parse_macro_input, punctuated::Punctuated};

use crate::derive_engine_class::struct_has_gpu_vec_field;
use crate::field_attrs::has_derive;

/// The flags an `#[engine_class(...)]` invocation turned on.
#[derive(Default)]
struct ClassFlags {
    category: Option<String>,
    add_serialize: bool,
    add_deserialize: bool,
    add_default: bool,
    add_clone: bool,
    add_debug: bool,
    add_scene_store: bool,
    add_gpu_rows: bool,
    no_register: bool,
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, syn::Token![,]>::parse_terminated);
    let item_struct = parse_macro_input!(item as ItemStruct);

    let flags = match parse_flags(args) {
        Ok(flags) => flags,
        Err(error) => return error.to_compile_error().into(),
    };

    let derive_additions = derive_additions(&flags, &item_struct);
    let derive_attr = if derive_additions.is_empty() {
        quote! {}
    } else {
        quote! { #[derive(#(#derive_additions),*)] }
    };
    let marker_attrs = marker_attrs(&flags, &item_struct);

    let sub_props_marker_impl = if flags.no_register {
        let name = &item_struct.ident;
        quote! { impl pulsar_reflection::EngineSubProps for #name {} }
    } else {
        quote! {}
    };

    quote! {
        #derive_attr
        #marker_attrs
        #item_struct
        #sub_props_marker_impl
    }
    .into()
}

/// Reads the macro's arguments, rejecting anything it does not know.
fn parse_flags(args: Punctuated<Meta, syn::Token![,]>) -> syn::Result<ClassFlags> {
    let mut flags = ClassFlags::default();
    for arg in args {
        match arg {
            Meta::Path(path) if path.is_ident("serialize") => flags.add_serialize = true,
            Meta::Path(path) if path.is_ident("deserialize") => flags.add_deserialize = true,
            Meta::Path(path) if path.is_ident("default") => flags.add_default = true,
            Meta::Path(path) if path.is_ident("clone") => flags.add_clone = true,
            Meta::Path(path) if path.is_ident("debug") => flags.add_debug = true,
            Meta::Path(path) if path.is_ident("no_register") => flags.no_register = true,
            Meta::Path(path) if path.is_ident("scene_store") => flags.add_scene_store = true,
            Meta::Path(path) if path.is_ident("gpu_rows") => flags.add_gpu_rows = true,
            Meta::NameValue(name_value) if name_value.path.is_ident("category") => {
                if let Expr::Lit(expr_lit) = &name_value.value {
                    if let Lit::Str(lit_str) = &expr_lit.lit {
                        flags.category = Some(lit_str.value());
                        continue;
                    }
                }
                return Err(syn::Error::new_spanned(
                    &name_value,
                    "engine_class category must be a string literal",
                ));
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "unsupported #[engine_class(...)] argument",
                ));
            }
        }
    }
    Ok(flags)
}

/// The derives to add: `EngineClass` plus each one a flag asked for that the
/// struct does not already derive.
fn derive_additions(flags: &ClassFlags, item_struct: &ItemStruct) -> Vec<TokenStream2> {
    let derives = |name: &str| has_derive(&item_struct.attrs, name);
    let mut derive_additions = Vec::new();
    if !derives("EngineClass") {
        derive_additions.push(quote!(::engine_class_derive::EngineClass));
    }
    if flags.add_serialize && !derives("Serialize") {
        derive_additions.push(quote!(::serde::Serialize));
    }
    if flags.add_deserialize && !derives("Deserialize") {
        derive_additions.push(quote!(::serde::Deserialize));
    }
    if flags.add_default && !derives("Default") {
        derive_additions.push(quote!(::core::default::Default));
    }
    if flags.add_clone && !derives("Clone") {
        derive_additions.push(quote!(::core::clone::Clone));
    }
    if flags.add_debug && !derives("Debug") {
        derive_additions.push(quote!(::core::fmt::Debug));
    }
    // Delegates to `pulsar_scenedb_derive`'s own derive (re-exported as
    // `pulsar_scenedb::SceneStore`) instead of hand-rolling equivalent
    // Pod/HasTypeToken/SceneColumnSet/GpuColumnSet codegen here. This targets
    // `pulsar_scenedb`'s `World`/`Entity`/`#[gpu(buffer = "...")]` storage
    // model, the one `World::insert`/`get_mut` and the GPU world-mirror
    // actually use. Requires the consuming crate to depend on
    // `pulsar_scenedb` (with the `gpu` feature on, for the GPU-column half);
    // `#[gpu(...)]` per-field attributes on the struct are consumed by
    // SceneStore's own attribute parsing (`attributes(gpu)`), not by this
    // macro.
    //
    // GOTCHA for a `#[engine_class(scene_store, ...)]` struct with a
    // `#[gpu(...)]` field: SceneStore's generated GPU-column methods are
    // wrapped in `#[cfg(feature = "gpu")]`. That `cfg`, once spliced into the
    // crate where the struct is actually DEFINED by macro expansion, checks
    // THAT crate's own Cargo features -- not `pulsar_scenedb`'s. So the crate
    // defining the struct (e.g. `helio_component`) needs its own feature named
    // exactly "gpu", same as `engine_class_derive`'s own `[features] gpu`
    // (Cargo.toml) exists solely to make this crate's own
    // `tests/scene_store_delegation.rs` compile -- `helio-scenedb/Cargo.toml`
    // documents the identical gotcha.
    //
    // `pulsar_scenedb::Pod` requires `Copy` (a GPU-mirrored/CellStorage-row
    // value is memcpy'd, never dropped in place) -- `scene_store` implies
    // `Copy` for the same reason `default`/`clone`/etc. each imply their
    // own derive, so a component author doesn't have to separately remember
    // it every time.
    if flags.add_scene_store {
        if !derives("SceneStore") {
            derive_additions.push(quote!(::pulsar_scenedb::SceneStore));
        }
        // Skipped for a struct with a `#[gpu] Vec<T>` field -- see
        // `struct_has_gpu_vec_field`'s doc.
        if !derives("Copy") && !struct_has_gpu_vec_field(&item_struct.fields) {
            derive_additions.push(quote!(::core::marker::Copy));
        }
    }
    derive_additions
}

/// The marker attributes the `EngineClass` derive reads. A
/// `#[proc_macro_derive]` never sees the derive list that triggered it (only
/// its OTHER attributes), so everything it needs to know about the struct is
/// stamped here as a marker.
fn marker_attrs(flags: &ClassFlags, item_struct: &ItemStruct) -> TokenStream2 {
    let derives = |name: &str| has_derive(&item_struct.attrs, name);
    let has_engine_class_category_attr = item_struct
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("engine_class_category"));

    let category_attr = match flags.category.as_ref() {
        Some(cat) if !has_engine_class_category_attr => {
            quote! { #[engine_class_category(#cat)] }
        }
        _ => quote! {},
    };

    let no_register_attr = if flags.no_register {
        quote! { #[engine_class_no_register] }
    } else {
        quote! {}
    };

    // Whether `Serialize`/`Deserialize` end up on this struct at all, whether
    // just added or already written by hand (see `json_tokens` in
    // `derive_engine_class`, which reads these).
    let serialize_marker_attr = if flags.add_serialize || derives("Serialize") {
        quote! { #[engine_class_serialize] }
    } else {
        quote! {}
    };
    let deserialize_marker_attr = if flags.add_deserialize || derives("Deserialize") {
        quote! { #[engine_class_deserialize] }
    } else {
        quote! {}
    };
    // The GPU ownership boundary: a `scene_store` struct's
    // `#[gpu] Vec<T>`/`GpuHeavy<T>` fields are SceneDB's own
    // `#[derive(SceneStore)]` storage (var-len / handle-split paths), NOT this
    // derive's packed companions. The marker is how `derive_engine_class`
    // distinguishes "routed to SceneDB, skip" from "misuse, reject".
    let scene_store_marker_attr = if flags.add_scene_store || derives("SceneStore") {
        quote! { #[engine_class_scene_store] }
    } else {
        quote! {}
    };
    // `gpu_rows`: the generated `#[gpu]` companion is uploaded as a SceneDB
    // GPU row (see `gpu_mirror_codegen`).
    let gpu_rows_marker_attr = if flags.add_gpu_rows {
        quote! { #[engine_class_gpu_rows] }
    } else {
        quote! {}
    };

    quote! {
        #category_attr
        #no_register_attr
        #serialize_marker_attr
        #deserialize_marker_attr
        #scene_store_marker_attr
        #gpu_rows_marker_attr
    }
}
