use proc_macro::TokenStream;
use quote::quote;
use syn::{Item, ItemType, Meta, Path, parse_macro_input, punctuated::Punctuated};

use crate::util;

pub fn pulsar_type(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, syn::Token![,]>::parse_terminated);
    let input = parse_macro_input!(item as Item);

    let item_type = match input {
        Item::Type(item_type) => item_type,
        other => {
            return syn::Error::new_spanned(
                other,
                "#[pulsar_type] currently ONLYsupports type aliases for primitive registration",
            )
            .to_compile_error()
            .into();
        }
    };

    match expand_primitive_alias(args.iter().collect(), &item_type) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

pub fn expand_primitive_alias(
    args: Vec<&Meta>,
    item_type: &ItemType,
) -> syn::Result<proc_macro2::TokenStream> {
    if !item_type.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_type.generics,
            "#[pulsar_type] primitive aliases do not support generics",
        ));
    }

    let mut override_serialize_json_with: Option<Path> = None;
    let mut override_deserialize_json_with: Option<Path> = None;
    let mut override_editor: Option<Path> = None;
    let mut color: Option<syn::LitStr> = None;

    let mut has_deprecated_primitive: Option<proc_macro2::Span> = None;
    let mut has_deprecated_structure: Option<proc_macro2::Span> = None;

    for meta in args {
        match meta {
            Meta::Path(path) if path.is_ident("primitive") => {
                has_deprecated_primitive = Some(path.get_ident().unwrap().span());
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("structure") => {
                has_deprecated_structure = Some(name_value.path.get_ident().unwrap().span());
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("serialize_json_with") => {
                override_serialize_json_with = Some(util::parse_path_expr(
                    &name_value.value,
                    "serialize_json_with",
                )?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("deserialize_json_with") => {
                override_deserialize_json_with = Some(util::parse_path_expr(
                    &name_value.value,
                    "deserialize_json_with",
                )?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("editor") => {
                override_editor = Some(util::parse_path_expr(&name_value.value, "editor")?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("color") => {
                color = Some(util::parse_lit_str_expr(&name_value.value, "color")?);
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    meta,
                    "unsupported #[pulsar_type(...)] argument",
                ));
            }
        }
    }

    if has_deprecated_primitive.is_some() || has_deprecated_structure.is_some() {
        crate::deprecation::emit_deprecation_warning(
            has_deprecated_primitive,
            has_deprecated_structure,
        );
    }

    let color_expr = match &color {
        Some(lit) => quote! { Some(#lit) },
        None => quote! { None },
    };

    let alias_ident = &item_type.ident;
    let target_ty = &item_type.ty;
    let type_info_name =
        quote::format_ident!("{}_TYPE_INFO", alias_ident.to_string().to_uppercase());

    if override_serialize_json_with.is_none() || override_deserialize_json_with.is_none() {
        return Err(syn::Error::new_spanned(
            &item_type.ident,
            "#[pulsar_type] requires serialize_json_with and deserialize_json_with to be specified",
        ));
    }

    let serialize_json_with = override_serialize_json_with.clone().unwrap();
    let deserialize_json_with = override_deserialize_json_with.clone().unwrap();

    let json_serialize_value = quote! {
        (#serialize_json_with as fn(&#target_ty) -> ::pulsar_reflection::ReflectResult<::serde_json::Value>)(typed)
    };
    let json_deserialize_value = quote! {
        (#deserialize_json_with as fn(::serde_json::Value) -> ::pulsar_reflection::ReflectResult<#target_ty>)(value)
    };
    let clone_impl = quote! { typed.clone() };

    let editor_submit = if let Some(editor_fn) = override_editor {
        quote! {
            ::pulsar_reflection::inventory::submit! {
                ::pulsar_reflection::UiPropertyEditorHint {
                    type_id: ::std::any::TypeId::of::<#target_ty>(),
                    fn_ptr: ::pulsar_reflection::erase_property_editor_fn_ptr(
                        #editor_fn as fn(
                            &::pulsar_reflection::PropertyEditorArgs<'_>,
                            &::gpui::App
                        ) -> ::gpui::AnyElement
                    ),
                }
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #item_type

        #[allow(non_upper_case_globals)]
        static #type_info_name: ::pulsar_reflection::RuntimeTypeInfo = ::pulsar_reflection::RuntimeTypeInfo {
            type_id: ::std::any::TypeId::of::<#target_ty>(),
            type_name: stringify!(#target_ty),
            size: ::std::mem::size_of::<#target_ty>(),
            align: ::std::mem::align_of::<#target_ty>(),
            structure: ::pulsar_reflection::TypeStructure::Primitive,
            color: #color_expr,
        };

        impl ::pulsar_reflection::Reflectable for #target_ty {
            fn type_info() -> &'static ::pulsar_reflection::RuntimeTypeInfo {
                &#type_info_name
            }

            fn serialize(&self, serializer: &mut dyn ::pulsar_reflection::TypeSerializer) -> ::pulsar_reflection::ReflectResult<()> {
                serializer.serialize_registered(self as &dyn ::std::any::Any)
            }

            fn deserialize(deserializer: &mut dyn ::pulsar_reflection::TypeDeserializer) -> ::pulsar_reflection::ReflectResult<Self> {
                let boxed = deserializer.deserialize_registered(Self::type_info())?;
                let found = format!("{:?}", (&*boxed).type_id());
                boxed
                    .downcast::<#target_ty>()
                    .map(|value| *value)
                    .map_err(|_| ::pulsar_reflection::ReflectError::TypeMismatch {
                        expected: stringify!(#target_ty),
                        found,
                    })
            }

            fn clone_any(&self) -> ::std::boxed::Box<dyn ::std::any::Any> {
                ::std::boxed::Box::new(self.clone())
            }
        }

        ::pulsar_reflection::inventory::submit! {
            ::pulsar_reflection::RuntimeTypeRegistration {
                type_info: &#type_info_name,
                serialize_json: |value: &dyn ::std::any::Any| {
                    let typed = value.downcast_ref::<#target_ty>().ok_or_else(|| {
                        ::pulsar_reflection::ReflectError::TypeMismatch {
                            expected: stringify!(#target_ty),
                            found: format!("{:?}", value.type_id()),
                        }
                    })?;
                    #json_serialize_value
                },
                deserialize_json: |value: ::serde_json::Value| {
                    let typed: #target_ty = #json_deserialize_value?;
                    Ok(::std::boxed::Box::new(#clone_impl) as ::std::boxed::Box<dyn ::std::any::Any>)
                },
            }
        }

        #editor_submit
    })
}
