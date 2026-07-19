use quote::quote;
use syn::{DataStruct, Fields, Ident, ImplGenerics, TypeGenerics, WhereClause, parse_quote};

use crate::field_info;

pub fn generate_struct_impl(
    name: &Ident,
    impl_generics: &ImplGenerics,
    ty_generics: &TypeGenerics,
    where_clause: &Option<&WhereClause>,
    data_struct: &DataStruct,
    color_expr: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    match &data_struct.fields {
        Fields::Named(fields) => generate_named_fields_impl(
            name,
            impl_generics,
            ty_generics,
            where_clause,
            &fields,
            color_expr,
        ),
        Fields::Unnamed(_) => syn::Error::new_spanned(
            name,
            "Reflectable only supports structs with named fields (tuple structs not supported yet)",
        )
        .to_compile_error(),
        Fields::Unit => {
            generate_unit_struct_impl(name, impl_generics, ty_generics, where_clause, color_expr)
        }
    }
}

fn generate_named_fields_impl(
    name: &Ident,
    impl_generics: &ImplGenerics,
    ty_generics: &TypeGenerics,
    where_clause: &Option<&WhereClause>,
    fields: &syn::FieldsNamed,
    color_expr: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let field_infos = field_info::generate_field_infos(&fields.named, name);

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

    let type_info_name = quote::format_ident!("{}_TYPE_INFO", name);

    quote! {
        static #type_info_name: ::pulsar_reflection::RuntimeTypeInfo = ::pulsar_reflection::RuntimeTypeInfo {
            type_id: std::any::TypeId::of::<#name #ty_generics>(),
            type_name: stringify!(#name),
            size: std::mem::size_of::<#name #ty_generics>(),
            align: std::mem::align_of::<#name #ty_generics>(),
            structure: ::pulsar_reflection::TypeStructure::Struct {
                fields: &#field_infos,
            },
            color: #color_expr,
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

        ::pulsar_reflection::inventory::submit! {
            ::pulsar_reflection::RuntimeTypeRegistration {
                type_info: &#type_info_name,
                serialize_json: |value: &dyn ::std::any::Any| {
                    let typed = value.downcast_ref::<#name #ty_generics>().ok_or_else(|| {
                        ::pulsar_reflection::ReflectError::TypeMismatch {
                            expected: stringify!(#name),
                            found: format!("{:?}", value.type_id()),
                        }
                    })?;
                    let mut serializer = ::pulsar_reflection::JsonSerializer::new();
                    <#name #ty_generics as ::pulsar_reflection::Reflectable>::serialize(typed, &mut serializer)?;
                    Ok(serializer.into_json())
                },
                deserialize_json: |value: ::serde_json::Value| {
                    let mut deserializer = ::pulsar_reflection::JsonDeserializer::new(value);
                    let typed = <#name #ty_generics as ::pulsar_reflection::Reflectable>::deserialize(&mut deserializer)?;
                    Ok(::std::boxed::Box::new(typed) as ::std::boxed::Box<dyn ::std::any::Any>)
                },
            }
        }
    }
}

fn generate_unit_struct_impl(
    name: &Ident,
    impl_generics: &ImplGenerics,
    ty_generics: &TypeGenerics,
    where_clause: &Option<&WhereClause>,
    color_expr: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let type_info_name = quote::format_ident!("{}_TYPE_INFO", name);

    quote! {
        static #type_info_name: ::pulsar_reflection::RuntimeTypeInfo = ::pulsar_reflection::RuntimeTypeInfo {
            type_id: std::any::TypeId::of::<#name #ty_generics>(),
            type_name: stringify!(#name),
            size: std::mem::size_of::<#name #ty_generics>(),
            align: std::mem::align_of::<#name #ty_generics>(),
            structure: ::pulsar_reflection::TypeStructure::Struct {
                fields: &[],
            },
            color: #color_expr,
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
                serialize_json: |value: &dyn ::std::any::Any| {
                    let typed = value.downcast_ref::<#name #ty_generics>().ok_or_else(|| {
                        ::pulsar_reflection::ReflectError::TypeMismatch {
                            expected: stringify!(#name),
                            found: format!("{:?}", value.type_id()),
                        }
                    })?;
                    let mut serializer = ::pulsar_reflection::JsonSerializer::new();
                    <#name #ty_generics as ::pulsar_reflection::Reflectable>::serialize(typed, &mut serializer)?;
                    Ok(serializer.into_json())
                },
                deserialize_json: |value: ::serde_json::Value| {
                    let mut deserializer = ::pulsar_reflection::JsonDeserializer::new(value);
                    let typed = <#name #ty_generics as ::pulsar_reflection::Reflectable>::deserialize(&mut deserializer)?;
                    Ok(::std::boxed::Box::new(typed) as ::std::boxed::Box<dyn ::std::any::Any>)
                },
            }
        }
    }
}
