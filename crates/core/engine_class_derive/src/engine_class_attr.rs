//! `#[engine_class(...)]`: the attribute form that adds derives and then
//! delegates to the `EngineClass` derive. Entry point: [`expand`].

use proc_macro::TokenStream;
use quote::quote;
use syn::{Expr, ItemStruct, Lit, Meta, parse_macro_input, punctuated::Punctuated};

use crate::derive_engine_class::struct_has_gpu_vec_field;
use crate::field_attrs::has_derive;

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, syn::Token![,]>::parse_terminated);
    let item_struct = parse_macro_input!(item as ItemStruct);

    let mut category: Option<String> = None;
    let mut add_serialize = false;
    let mut add_deserialize = false;
    let mut add_default = false;
    let mut add_clone = false;
    let mut add_debug = false;
    let mut add_scene_store = false;
    let mut add_gpu_rows = false;
    let mut no_register = false;

    for arg in args {
        match arg {
            Meta::Path(path) if path.is_ident("serialize") => add_serialize = true,
            Meta::Path(path) if path.is_ident("deserialize") => add_deserialize = true,
            Meta::Path(path) if path.is_ident("default") => add_default = true,
            Meta::Path(path) if path.is_ident("clone") => add_clone = true,
            Meta::Path(path) if path.is_ident("debug") => add_debug = true,
            Meta::Path(path) if path.is_ident("no_register") => no_register = true,
            Meta::Path(path) if path.is_ident("scene_store") => add_scene_store = true,
            Meta::Path(path) if path.is_ident("gpu_rows") => add_gpu_rows = true,
            Meta::NameValue(name_value) if name_value.path.is_ident("category") => {
                if let Expr::Lit(expr_lit) = &name_value.value {
                    if let Lit::Str(lit_str) = &expr_lit.lit {
                        category = Some(lit_str.value());
                        continue;
                    }
                }
                return syn::Error::new_spanned(
                    &name_value,
                    "engine_class category must be a string literal",
                )
                .to_compile_error()
                .into();
            }
            other => {
                return syn::Error::new_spanned(other, "unsupported #[engine_class(...)] argument")
                    .to_compile_error()
                    .into();
            }
        }
    }

    let has_engine_class_derive = has_derive(&item_struct.attrs, "EngineClass");
    let has_serialize_derive = has_derive(&item_struct.attrs, "Serialize");
    let has_deserialize_derive = has_derive(&item_struct.attrs, "Deserialize");
    let has_default_derive = has_derive(&item_struct.attrs, "Default");
    let has_clone_derive = has_derive(&item_struct.attrs, "Clone");
    let has_debug_derive = has_derive(&item_struct.attrs, "Debug");
    let has_scene_store_derive = has_derive(&item_struct.attrs, "SceneStore");
    let has_copy_derive = has_derive(&item_struct.attrs, "Copy");
    let has_engine_class_category_attr = item_struct
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("engine_class_category"));

    let mut derive_additions = Vec::new();
    if !has_engine_class_derive {
        derive_additions.push(quote!(::engine_class_derive::EngineClass));
    }
    if add_serialize && !has_serialize_derive {
        derive_additions.push(quote!(::serde::Serialize));
    }
    if add_deserialize && !has_deserialize_derive {
        derive_additions.push(quote!(::serde::Deserialize));
    }
    if add_default && !has_default_derive {
        derive_additions.push(quote!(::core::default::Default));
    }
    if add_clone && !has_clone_derive {
        derive_additions.push(quote!(::core::clone::Clone));
    }
    if add_debug && !has_debug_derive {
        derive_additions.push(quote!(::core::fmt::Debug));
    }
    // Delegates to `pulsar_scenedb_derive`'s own derive (re-exported as
    // `pulsar_scenedb::SceneStore`) instead of hand-rolling equivalent
    // Pod/HasTypeToken/SceneColumnSet/GpuColumnSet codegen here -- see this
    // block's doc below for why. Requires the consuming crate to depend on
    // `pulsar_scenedb` (with the `gpu` feature on, for the GPU-column half);
    // `#[gpu(...)]` per-field attributes on the struct are consumed by
    // SceneStore's own attribute parsing (`attributes(gpu)`), not by this
    // macro -- they were already valid syntax here (the old codegen parsed
    // them by hand too), so no field-level change is needed to opt in.
    //
    // GOTCHA for whoever writes the first real `#[engine_class(scene_store,
    // ...)]` struct with a `#[gpu(...)]` field (Phase C+): SceneStore's
    // generated GPU-column methods are wrapped in `#[cfg(feature = "gpu")]`.
    // That `cfg`, once spliced into the crate where the struct is actually
    // DEFINED by macro expansion, checks THAT crate's own Cargo features --
    // not `pulsar_scenedb`'s. So the crate defining the struct (e.g.
    // `helio_component`) needs its own feature named exactly "gpu", same
    // as `engine_class_derive`'s own `[features] gpu` (Cargo.toml) exists
    // solely to make this crate's own `tests/scene_store_delegation.rs`
    // compile -- `helio-scenedb/Cargo.toml` documents the identical gotcha.
    //
    // `pulsar_scenedb::Pod` requires `Copy` (a GPU-mirrored/CellStorage-row
    // value is memcpy'd, never dropped in place) -- `scene_store` implies
    // `Copy` for the same reason `default`/`clone`/etc. each imply their
    // own derive, so a component author doesn't have to separately remember
    // it every time.
    if add_scene_store {
        if !has_scene_store_derive {
            derive_additions.push(quote!(::pulsar_scenedb::SceneStore));
        }
        // Skipped for a struct with a `#[gpu] Vec<T>` field -- see
        // `struct_has_gpu_vec_field`'s doc.
        if !has_copy_derive && !struct_has_gpu_vec_field(&item_struct.fields) {
            derive_additions.push(quote!(::core::marker::Copy));
        }
    }

    let derive_attr = if derive_additions.is_empty() {
        quote! {}
    } else {
        quote! { #[derive(#(#derive_additions),*)] }
    };

    let category_attr = match category.as_ref() {
        Some(cat) if !has_engine_class_category_attr => {
            quote! { #[engine_class_category(#cat)] }
        }
        _ => quote! {},
    };

    let no_register_attr = if no_register {
        quote! { #[engine_class_no_register] }
    } else {
        quote! {}
    };

    // Tell the `EngineClass` derive macro (below, via `derive_attr`) whether
    // `Serialize`/`Deserialize` end up on this struct at all -- whether we
    // just added them or the user already had them written by hand either
    // way. Has to be a marker attribute, not something the derive macro
    // infers from the `#[derive(...)]` list itself: a `#[proc_macro_derive]`
    // never sees the derive list that triggered it (only its OTHER
    // attributes), confirmed by instrumenting `derive_engine_class` directly
    // -- see that function's own comment on `has_serialize`/`has_deserialize`.
    let serialize_marker_attr = if add_serialize || has_serialize_derive {
        quote! { #[engine_class_serialize] }
    } else {
        quote! {}
    };
    let deserialize_marker_attr = if add_deserialize || has_deserialize_derive {
        quote! { #[engine_class_deserialize] }
    } else {
        quote! {}
    };
    // Same marker-attribute mechanism, for the GPU ownership boundary: a
    // `scene_store` struct's `#[gpu] Vec<T>`/`GpuHeavy<T>` fields are
    // SceneDB's own `#[derive(SceneStore)]` storage (var-len / handle-split
    // paths), NOT this derive's packed companions. The marker is how
    // `derive_engine_class` -- which can never see the derive list --
    // distinguishes "routed to SceneDB, skip" from "misuse, reject".
    let scene_store_marker_attr = if add_scene_store || has_scene_store_derive {
        quote! { #[engine_class_scene_store] }
    } else {
        quote! {}
    };
    // `gpu_rows`: the generated `#[gpu]` companion is uploaded as a SceneDB
    // GPU row (see `gpu_mirror_codegen`).
    let gpu_rows_marker_attr = if add_gpu_rows {
        quote! { #[engine_class_gpu_rows] }
    } else {
        quote! {}
    };

    let sub_props_marker_impl = if no_register {
        let name = &item_struct.ident;
        quote! { impl pulsar_reflection::EngineSubProps for #name {} }
    } else {
        quote! {}
    };

    let _name = &item_struct.ident;
    // SceneDB storage (Pod/HasTypeToken/SceneColumnSet/GpuColumnSet) is no
    // longer hand-generated here -- `add_scene_store` instead adds
    // `::pulsar_scenedb::SceneStore` to `derive_additions` above, which
    // `#derive_attr` below splices onto `#item_struct` alongside
    // EngineClass/Serialize/etc. This targets `pulsar_scenedb`'s current
    // `World`/`Entity`/`#[gpu(buffer = "...")]` storage model (the one
    // `World::insert`/`get_mut` and the GPU world-mirror actually use)
    // instead of the older `CellStorage`/`Handle`-mirrored model the
    // previous hand-rolled codegen targeted -- confirmed unexercised
    // anywhere in this workspace before this rewrite (nothing called
    // `write_gpu`/`.gpu_columns()`/`SceneColumnSet::cell_type()` outside
    // this file), so retargeting it is purely additive, not a break.

    quote! {
        #derive_attr
        #category_attr
        #no_register_attr
        #serialize_marker_attr
        #deserialize_marker_attr
        #scene_store_marker_attr
        #gpu_rows_marker_attr
        #item_struct
        #sub_props_marker_impl
    }
    .into()
}
