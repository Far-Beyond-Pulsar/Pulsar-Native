use quote::quote;
use syn::{DataEnum, Fields, Ident, ImplGenerics, TypeGenerics, WhereClause};

pub fn generate_enum_impl(
    name: &Ident,
    impl_generics: &ImplGenerics,
    ty_generics: &TypeGenerics,
    where_clause: &Option<&WhereClause>,
    data_enum: &DataEnum,
    color_expr: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let variant_names: Vec<String> = data_enum
        .variants
        .iter()
        .map(|v| v.ident.to_string())
        .collect();

    let variant_name_literals: Vec<_> = variant_names.iter().map(|s| quote! { #s }).collect();

    let serialize_match_arms = data_enum.variants.iter().enumerate().map(|(idx, variant)| {
        let variant_ident = &variant.ident;
        let variant_name = variant_ident.to_string();

        match &variant.fields {
            Fields::Unit => {
                quote! {
                    Self::#variant_ident => {
                        serializer.serialize_enum(#variant_name, #idx)?;
                    }
                }
            }
            _ => {
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

    let type_info_name = quote::format_ident!("{}_TYPE_INFO", name);

    quote! {
        static #type_info_name: ::pulsar_reflection::RuntimeTypeInfo = ::pulsar_reflection::RuntimeTypeInfo {
            type_id: std::any::TypeId::of::<#name #ty_generics>(),
            type_name: stringify!(#name),
            size: std::mem::size_of::<#name #ty_generics>(),
            align: std::mem::align_of::<#name #ty_generics>(),
            structure: ::pulsar_reflection::TypeStructure::Enum {
                variants: &[#(#variant_name_literals),*],
            },
            color: #color_expr,
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
