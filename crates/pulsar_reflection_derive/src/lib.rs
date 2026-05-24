//! Proc macro for deriving `Reflectable` trait
//!
//! This crate provides the `#[derive(Reflectable)]` macro that automatically
//! implements runtime type reflection for structs and enums.
//!
//! # Example
//!
//! ```ignore
//! use pulsar_reflection_derive::Reflectable;
//!
//! #[derive(Reflectable, Clone)]
//! pub struct Transform {
//!     pub position: Vec3,
//!     pub rotation: Quat,
//!     pub scale: Vec3,
//! }
//! ```

use proc_macro::TokenStream;
use quote::{quote, format_ident};
use syn::{
    parse_macro_input, Data, DeriveInput, Fields, Field, Type, Ident,
};

/// Derive macro for Reflectable trait
///
/// Automatically generates:
/// - RuntimeTypeInfo with compile-time metadata
/// - Reflectable trait implementation
/// - Serialization/deserialization methods
/// - Inventory registration for automatic type discovery
#[proc_macro_derive(Reflectable, attributes(reflect))]
pub fn derive_reflectable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate implementation based on data type
    let expanded = match &input.data {
        Data::Struct(data_struct) => {
            generate_struct_impl(name, &impl_generics, &ty_generics, &where_clause, data_struct)
        }
        Data::Enum(data_enum) => {
            generate_enum_impl(name, &impl_generics, &ty_generics, &where_clause, data_enum)
        }
        Data::Union(_) => {
            return syn::Error::new_spanned(&input, "Reflectable cannot be derived for unions")
                .to_compile_error()
                .into();
        }
    };

    expanded.into()
}

/// Generate implementation for struct types
fn generate_struct_impl(
    name: &Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: &Option<&syn::WhereClause>,
    data_struct: &syn::DataStruct,
) -> proc_macro2::TokenStream {
    match &data_struct.fields {
        Fields::Named(fields) => {
            let field_infos = generate_field_infos(&fields.named, name);

            let serialize_fields = fields.named.iter().map(|field| {
                let field_name = field.ident.as_ref().unwrap();
                let field_name_str = field_name.to_string();
                quote! {
                    (#field_name_str, &self.#field_name as &dyn std::any::Any)
                }
            });

            let deserialize_fields = fields.named.iter().map(|field| {
                let field_name = field.ident.as_ref().unwrap();
                let field_name_str = field_name.to_string();
                let field_type = &field.ty;

                quote! {
                    #field_name: {
                        let value = fields.get(#field_name_str)
                            .ok_or_else(|| ::pulsar_reflection::ReflectError::MissingField {
                                struct_name: stringify!(#name),
                                field_name: #field_name_str,
                            })?;
                        *value.downcast_ref::<#field_type>()
                            .ok_or_else(|| ::pulsar_reflection::ReflectError::TypeMismatch {
                                expected: stringify!(#field_type),
                                found: format!("{:?}", value.type_id()),
                            })?
                    }
                }
            });

            let type_info_name = format_ident!("{}_TYPE_INFO", name);

            quote! {
                // Static type info
                static #type_info_name: ::pulsar_reflection::RuntimeTypeInfo = ::pulsar_reflection::RuntimeTypeInfo {
                    type_id: std::any::TypeId::of::<#name #ty_generics>(),
                    type_name: stringify!(#name),
                    size: std::mem::size_of::<#name #ty_generics>(),
                    align: std::mem::align_of::<#name #ty_generics>(),
                    structure: ::pulsar_reflection::TypeStructure::Struct {
                        fields: &#field_infos,
                    },
                };

                impl #impl_generics ::pulsar_reflection::Reflectable for #name #ty_generics #where_clause {
                    fn type_info() -> &'static ::pulsar_reflection::RuntimeTypeInfo {
                        &#type_info_name
                    }

                    fn serialize(&self, serializer: &mut dyn ::pulsar_reflection::TypeSerializer) -> ::pulsar_reflection::ReflectResult<()> {
                        serializer.serialize_struct(&[
                            #(#serialize_fields),*
                        ])
                    }

                    fn deserialize(deserializer: &mut dyn ::pulsar_reflection::TypeDeserializer) -> ::pulsar_reflection::ReflectResult<Self> {
                        let type_info = Self::type_info();
                        let fields_info = type_info.fields().ok_or_else(|| {
                            ::pulsar_reflection::ReflectError::DeserializationFailed(
                                format!("{} is not a struct", stringify!(#name))
                            )
                        })?;

                        let fields = deserializer.deserialize_struct(fields_info)?;

                        Ok(Self {
                            #(#deserialize_fields),*
                        })
                    }

                    fn clone_any(&self) -> Box<dyn std::any::Any> {
                        Box::new(self.clone())
                    }
                }

                // Auto-register with inventory
                ::pulsar_reflection::inventory::submit! {
                    ::pulsar_reflection::RuntimeTypeRegistration {
                        type_info: &#type_info_name,
                    }
                }
            }
        }
        Fields::Unnamed(_) => {
            syn::Error::new_spanned(
                name,
                "Reflectable only supports structs with named fields (tuple structs not supported yet)",
            )
            .to_compile_error()
        }
        Fields::Unit => {
            // Unit struct - no fields
            let type_info_name = format_ident!("{}_TYPE_INFO", name);

            quote! {
                static #type_info_name: ::pulsar_reflection::RuntimeTypeInfo = ::pulsar_reflection::RuntimeTypeInfo {
                    type_id: std::any::TypeId::of::<#name #ty_generics>(),
                    type_name: stringify!(#name),
                    size: std::mem::size_of::<#name #ty_generics>(),
                    align: std::mem::align_of::<#name #ty_generics>(),
                    structure: ::pulsar_reflection::TypeStructure::Struct {
                        fields: &[],
                    },
                };

                impl #impl_generics ::pulsar_reflection::Reflectable for #name #ty_generics #where_clause {
                    fn type_info() -> &'static ::pulsar_reflection::RuntimeTypeInfo {
                        &#type_info_name
                    }

                    fn serialize(&self, serializer: &mut dyn ::pulsar_reflection::TypeSerializer) -> ::pulsar_reflection::ReflectResult<()> {
                        serializer.serialize_struct(&[])
                    }

                    fn deserialize(_deserializer: &mut dyn ::pulsar_reflection::TypeDeserializer) -> ::pulsar_reflection::ReflectResult<Self> {
                        Ok(Self)
                    }

                    fn clone_any(&self) -> Box<dyn std::any::Any> {
                        Box::new(self.clone())
                    }
                }

                ::pulsar_reflection::inventory::submit! {
                    ::pulsar_reflection::RuntimeTypeRegistration {
                        type_info: &#type_info_name,
                    }
                }
            }
        }
    }
}

/// Generate implementation for enum types
fn generate_enum_impl(
    name: &Ident,
    impl_generics: &syn::ImplGenerics,
    ty_generics: &syn::TypeGenerics,
    where_clause: &Option<&syn::WhereClause>,
    data_enum: &syn::DataEnum,
) -> proc_macro2::TokenStream {
    // Extract variant names
    let variant_names: Vec<String> = data_enum
        .variants
        .iter()
        .map(|v| v.ident.to_string())
        .collect();

    let variant_name_literals: Vec<_> = variant_names.iter().map(|s| quote! { #s }).collect();

    let serialize_match_arms = data_enum.variants.iter().enumerate().map(|(idx, variant)| {
        let variant_ident = &variant.ident;
        let variant_name = variant_ident.to_string();

        // Handle different variant types
        match &variant.fields {
            Fields::Unit => {
                quote! {
                    Self::#variant_ident => {
                        serializer.serialize_enum(#variant_name, #idx)?;
                    }
                }
            }
            _ => {
                // For now, only support unit variants
                quote! {
                    Self::#variant_ident { .. } => {
                        return Err(::pulsar_reflection::ReflectError::SerializationFailed(
                            format!("Enum variants with fields not yet supported")
                        ));
                    }
                }
            }
        }
    });

    let deserialize_match_arms = data_enum.variants.iter().enumerate().map(|(idx, variant)| {
        let variant_ident = &variant.ident;

        match &variant.fields {
            Fields::Unit => {
                quote! {
                    #idx => Ok(Self::#variant_ident),
                }
            }
            _ => {
                quote! {
                    #idx => Err(::pulsar_reflection::ReflectError::DeserializationFailed(
                        format!("Enum variants with fields not yet supported")
                    )),
                }
            }
        }
    });

    let type_info_name = format_ident!("{}_TYPE_INFO", name);

    quote! {
        static #type_info_name: ::pulsar_reflection::RuntimeTypeInfo = ::pulsar_reflection::RuntimeTypeInfo {
            type_id: std::any::TypeId::of::<#name #ty_generics>(),
            type_name: stringify!(#name),
            size: std::mem::size_of::<#name #ty_generics>(),
            align: std::mem::align_of::<#name #ty_generics>(),
            structure: ::pulsar_reflection::TypeStructure::Enum {
                variants: &[#(#variant_name_literals),*],
            },
        };

        impl #impl_generics ::pulsar_reflection::Reflectable for #name #ty_generics #where_clause {
            fn type_info() -> &'static ::pulsar_reflection::RuntimeTypeInfo {
                &#type_info_name
            }

            fn serialize(&self, serializer: &mut dyn ::pulsar_reflection::TypeSerializer) -> ::pulsar_reflection::ReflectResult<()> {
                match self {
                    #(#serialize_match_arms)*
                }
                Ok(())
            }

            fn deserialize(deserializer: &mut dyn ::pulsar_reflection::TypeDeserializer) -> ::pulsar_reflection::ReflectResult<Self> {
                let type_info = Self::type_info();
                let variants = type_info.enum_variants().ok_or_else(|| {
                    ::pulsar_reflection::ReflectError::DeserializationFailed(
                        format!("{} is not an enum", stringify!(#name))
                    )
                })?;

                let variant_index = deserializer.deserialize_enum(variants)?;

                match variant_index {
                    #(#deserialize_match_arms)*
                    _ => Err(::pulsar_reflection::ReflectError::InvalidVariant {
                        enum_name: stringify!(#name),
                        variant: format!("index {}", variant_index),
                    })
                }
            }

            fn clone_any(&self) -> Box<dyn std::any::Any> {
                Box::new(self.clone())
            }
        }

        ::pulsar_reflection::inventory::submit! {
            ::pulsar_reflection::RuntimeTypeRegistration {
                type_info: &#type_info_name,
            }
        }
    }
}

/// Generate field info array for struct fields
fn generate_field_infos(
    fields: &syn::punctuated::Punctuated<Field, syn::token::Comma>,
    struct_name: &Ident,
) -> proc_macro2::TokenStream {
    let field_info_items: Vec<_> = fields
        .iter()
        .map(|field| {
            let field_name = field.ident.as_ref().unwrap();
            let field_name_str = field_name.to_string();
            let field_type = &field.ty;

            // Calculate offset using offset_of! (available in recent Rust versions)
            // For now, we'll use a placeholder since offset_of! requires the type
            // In practice, this should use std::mem::offset_of! when stable
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

/// Helper to check if a type is a primitive
fn _is_primitive_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        let type_str = quote!(#type_path).to_string();
        matches!(
            type_str.as_str(),
            "f32" | "i32" | "u64" | "bool" | "String"
        )
    } else {
        false
    }
}
