//! `#[register_scene_props_applier]`: registers a `ScenePropsProjector` impl.
//! Entry point: [`expand`].

use proc_macro::TokenStream;
use quote::quote;
use syn::{ItemImpl, parse_macro_input};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "#[register_scene_props_applier] does not accept arguments",
        )
        .to_compile_error()
        .into();
    }

    let impl_block = parse_macro_input!(item as ItemImpl);

    if !impl_block.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &impl_block.generics,
            "#[register_scene_props_applier] does not support generic impl blocks",
        )
        .to_compile_error()
        .into();
    }

    let Some((_, trait_path, _)) = &impl_block.trait_ else {
        return syn::Error::new_spanned(
            &impl_block.self_ty,
            "#[register_scene_props_applier] must be used on `impl ScenePropsProjector for Type`",
        )
        .to_compile_error()
        .into();
    };

    let Some(trait_ident) = trait_path.segments.last().map(|s| &s.ident) else {
        return syn::Error::new_spanned(
            trait_path,
            "invalid trait path for #[register_scene_props_applier]",
        )
        .to_compile_error()
        .into();
    };

    if trait_ident != "ScenePropsProjector" {
        return syn::Error::new_spanned(
            trait_path,
            "#[register_scene_props_applier] must target `ScenePropsProjector` impl",
        )
        .to_compile_error()
        .into();
    }

    let self_ty = &impl_block.self_ty;
    let output = quote! {
        #impl_block

        pulsar_reflection::inventory::submit! {
            pulsar_reflection::ScenePropsApplierRegistration {
                class_name: <#self_ty as pulsar_reflection::ScenePropsProjector>::CLASS_NAME,
                apply: <#self_ty as pulsar_reflection::ScenePropsProjector>::apply_scene_props,
            }
        }
    };

    output.into()
}
