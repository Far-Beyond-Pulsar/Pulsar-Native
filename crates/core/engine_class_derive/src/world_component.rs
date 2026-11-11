//! `#[register_world_component]` / `#[world_component]`: wiring a component
//! into the `World` storage bridge. Entry points: [`expand`] (both macros;
//! `submit` picks whether the registration is submitted here).

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemImpl};

/// `#[register_world_component]`'s optional arguments:
///
/// - `decode = path` -- `fn(&serde_json::Value) -> Result<Self, String>`.
///   The JSON boundary decoder for this class, replacing the generated
///   `serde_json::from_value`. For a class whose file shape has legacy
///   variants (`LightComponent`'s flat intensity) or that loads asset data
///   its serialized form only references (`StaticMeshComponent`'s mesh
///   file). Decoding is the only class-specific step of hydration: the
///   decoded value is inserted through SceneDB's erased insert, with every
///   normal write hook.
/// - `remove = path` -- `fn(&mut pulsar_scenedb::World,
///   pulsar_scenedb::Entity)`, replacing the generated
///   `world.remove::<Self>(entity)`.
/// - `property_written = path` -- `fn(&mut Self, Option<&str>)`, run under
///   the same write guard right after a reflected property setter
///   (`Some(name)`) or method (`None`), so the class can re-establish data it
///   derives from its own fields. Generated as a no-op. Never a GPU refresh:
///   GPU rows, including the generated `#[gpu]` companion, follow every write
///   through SceneDB's own mirror dispatch.
///
/// Each `path` is used directly as a function pointer (or wrapped by a
/// one-line generated adapter), so a signature mismatch is an ordinary
/// compile error at the registration.
pub(crate) struct RegisterWorldComponentArgs {
    decode: Option<syn::Path>,
    custom_remove: Option<syn::Path>,
    property_written: Option<syn::Path>,
}

impl syn::parse::Parse for RegisterWorldComponentArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut args = RegisterWorldComponentArgs {
            decode: None,
            custom_remove: None,
            property_written: None,
        };
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            let _: syn::Token![=] = input.parse()?;
            let slot = match key.to_string().as_str() {
                "decode" => &mut args.decode,
                "remove" => &mut args.custom_remove,
                "property_written" => &mut args.property_written,
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!(
                            "unknown #[register_world_component] option `{other}` (expected `decode`, `remove`, or `property_written`)"
                        ),
                    ));
                }
            };
            *slot = Some(input.parse()?);
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(args)
    }
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream, submit: bool) -> TokenStream {
    let args = if attr.is_empty() {
        RegisterWorldComponentArgs {
            decode: None,
            custom_remove: None,
            property_written: None,
        }
    } else {
        match syn::parse::<RegisterWorldComponentArgs>(attr) {
            Ok(args) => args,
            Err(error) => return error.to_compile_error().into(),
        }
    };

    let impl_block = parse_macro_input!(item as ItemImpl);

    if !impl_block.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &impl_block.generics,
            "#[register_world_component] does not support generic impl blocks",
        )
        .to_compile_error()
        .into();
    }

    if impl_block.trait_.is_some() {
        return syn::Error::new_spanned(
            &impl_block.self_ty,
            "#[register_world_component] goes on an inherent `impl Type {}` block",
        )
        .to_compile_error()
        .into();
    }

    let self_ty = &impl_block.self_ty;
    let Some(self_ty_ident) = (match &**self_ty {
        syn::Type::Path(type_path) => type_path.path.segments.last().map(|s| &s.ident),
        _ => None,
    }) else {
        return syn::Error::new_spanned(
            self_ty,
            "#[register_world_component] requires a simple named type (no generics, no qualified paths)",
        )
        .to_compile_error()
        .into();
    };
    let class_name = syn::LitStr::new(&self_ty_ident.to_string(), self_ty_ident.span());
    let default_fn_name = quote::format_ident!("__pulsar_world_default_{}", self_ty_ident);
    let decode_fn_name = quote::format_ident!("__pulsar_world_decode_{}", self_ty_ident);
    let clone_fn_name = quote::format_ident!("__pulsar_world_clone_{}", self_ty_ident);
    let remove_fn_name = quote::format_ident!("__pulsar_world_remove_{}", self_ty_ident);
    let get_fn_name = quote::format_ident!("__pulsar_world_get_engine_class_{}", self_ty_ident);
    let get_mut_fn_name =
        quote::format_ident!("__pulsar_world_get_engine_class_mut_{}", self_ty_ident);
    let property_written_fn_name =
        quote::format_ident!("__pulsar_world_property_written_{}", self_ty_ident);

    let decode_expr = match &args.decode {
        Some(custom) => quote! { #custom(data)? },
        None => quote! {
            ::pulsar_world_registry::decode_json::<#self_ty>(data)?
        },
    };

    let (remove_fn_def, remove_fn_ref) = match &args.custom_remove {
        None => (
            quote! {
                #[doc(hidden)]
                #[allow(non_snake_case)]
                fn #remove_fn_name(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
                    let _ = world.remove::<#self_ty>(entity);
                }
            },
            quote! { #remove_fn_name },
        ),
        Some(custom) => (quote! {}, quote! { #custom }),
    };

    let property_written_body = match &args.property_written {
        Some(custom) => quote! {
            if let Some(value) = value.as_any_mut().downcast_mut::<#self_ty>() {
                #custom(value, property);
            }
        },
        None => quote! {
            let _ = (value, property);
        },
    };

    let submission = if submit {
        quote! {
            // The engine's own class: its origin is the engine's
            // (`pulsar_plugins::engine_origin!()`), always registered.
            pulsar_world_registry::inventory::submit! {
                pulsar_world_registry::WorldComponentRegistration {
                    origin: ::core::concat!("engine:", ::core::env!("CARGO_PKG_NAME")),
                    ..<#self_ty>::PULSAR_WORLD_COMPONENT
                }
            }
        }
    } else {
        quote! {}
    };

    let output = quote! {
        #remove_fn_def

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #default_fn_name() -> ::std::boxed::Box<dyn ::std::any::Any + Send + Sync> {
            ::std::boxed::Box::new(<#self_ty as ::core::default::Default>::default())
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #decode_fn_name(
            data: &::serde_json::Value,
        ) -> ::std::result::Result<::std::boxed::Box<dyn ::std::any::Any + Send + Sync>, ::std::string::String> {
            let value: #self_ty = #decode_expr;
            Ok(::std::boxed::Box::new(value))
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #clone_fn_name(
            value: &dyn ::std::any::Any,
        ) -> ::std::option::Option<::std::boxed::Box<dyn ::std::any::Any + Send + Sync>> {
            value
                .downcast_ref::<#self_ty>()
                .map(|value| ::std::boxed::Box::new(::core::clone::Clone::clone(value))
                    as ::std::boxed::Box<dyn ::std::any::Any + Send + Sync>)
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #property_written_fn_name(
            value: &mut dyn pulsar_reflection::EngineClass,
            property: ::std::option::Option<&str>,
        ) {
            #property_written_body
        }

        // Direct live access to the real `World`-resident value as `&(mut)
        // dyn EngineClass` -- the properties panel's read and edit paths
        // (Pulsar-Native#561). No JSON, no throwaway instance.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #get_fn_name(
            world: &pulsar_scenedb::World,
            entity: pulsar_scenedb::Entity,
        ) -> Option<&dyn pulsar_reflection::EngineClass> {
            world
                .get::<#self_ty>(entity)
                .map(|component| component as &dyn pulsar_reflection::EngineClass)
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #get_mut_fn_name(
            world: &mut pulsar_scenedb::World,
            entity: pulsar_scenedb::Entity,
        ) -> Option<pulsar_world_registry::EngineClassMut<'_>> {
            // A guard, not a bare `&mut`: SceneDB's write hooks (GPU mirror,
            // change tracker, journals, object subscriptions) fire when it drops,
            // after the edit, and only if it was written through (#841).
            pulsar_world_registry::EngineClassMut::of::<#self_ty>(world, entity)
        }

        impl #self_ty {
            /// This class's `World` registration (`#[world_component]`):
            /// what the plugin's root adds through its builder
            /// (`p.components().class::<Self>(spec)`, which sets the
            /// origin, requirements and presentation).
            #[doc(hidden)]
            pub const PULSAR_WORLD_COMPONENT: pulsar_world_registry::WorldComponentRegistration =
            pulsar_world_registry::WorldComponentRegistration {
                origin: ::core::env!("CARGO_PKG_NAME"),
                class_name: #class_name,
                component_type: pulsar_scenedb::component_id::<#self_ty>,
                default_value: #default_fn_name,
                decode: #decode_fn_name,
                clone_value: #clone_fn_name,
                value_as_engine_class: pulsar_world_registry::values::erased::as_engine_class::<#self_ty>,
                value_as_engine_class_mut: pulsar_world_registry::values::erased::as_engine_class_mut::<#self_ty>,
                register_erased: pulsar_scenedb::register_component::<#self_ty>,
                remove: #remove_fn_ref,
                get_as_engine_class: #get_fn_name,
                get_as_engine_class_mut: #get_mut_fn_name,
                property_written: #property_written_fn_name,
                requires: &[],
                presentation: pulsar_world_registry::Presentation::Component,
            };
        }

        impl pulsar_world_registry::WorldComponentClass for #self_ty {
            const REGISTRATION: pulsar_world_registry::WorldComponentRegistration =
                <#self_ty>::PULSAR_WORLD_COMPONENT;
        }

        #submission

        #impl_block
    };

    output.into()
}
