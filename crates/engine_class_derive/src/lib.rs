//! Proc macro for deriving `EngineClass` trait
//!
//! This crate provides the `#[derive(EngineClass)]` macro that automatically
//! implements the reflection trait for components and other engine types.
//!
//! # Example
//!
//! ```ignore
//! use engine_class_derive::EngineClass;
//!
//! #[derive(EngineClass, Default)]
//! pub struct PhysicsComponent {
//!     #[property(min = 0.0, max = 1000.0)]
//!     pub mass: f32,
//!
//!     #[property]
//!     pub friction: f32,
//! }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Field, Fields, Lit, Meta, MetaNameValue, Type, parse_macro_input,
};

#[proc_macro_derive(EngineClass, attributes(property, category))]
pub fn derive_engine_class(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Extract category from struct attributes
    let category = extract_category(&input.attrs);

    // Convert category to TokenStream for registration
    let category_token = if let Some(cat) = &category {
        quote! { Some(#cat) }
    } else {
        quote! { None }
    };

    // Extract fields marked with #[property]
    let property_impls = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields) => fields
                .named
                .iter()
                .filter_map(|field| {
                    if has_property_attr(field) {
                        Some(generate_property_metadata(field, name, &category))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>(),
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "EngineClass can only be derived for structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(&input, "EngineClass can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    // Generate the trait implementation
    let generated = quote! {
        impl #impl_generics pulsar_reflection::EngineClass for #name #ty_generics #where_clause {
            fn class_name() -> &'static str {
                stringify!(#name)
            }

            fn get_properties(&self) -> Vec<pulsar_reflection::PropertyMetadata> {
                vec![
                    #(#property_impls),*
                ]
            }

            fn create_default() -> Box<dyn pulsar_reflection::EngineClass> {
                Box::new(Self::default())
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }

            fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
                self
            }

            fn clone_boxed(&self) -> Box<dyn pulsar_reflection::EngineClass> {
                Box::new(self.clone())
            }
        }

        // Auto-register with global registry
        pulsar_reflection::inventory::submit! {
            pulsar_reflection::EngineClassRegistration {
                name: stringify!(#name),
                category: #category_token,
                constructor: || Box::new(#name::default()),
            }
        }
    };

    generated.into()
}

#[proc_macro_derive(RegisterRuntimeBehavior)]
pub fn derive_register_runtime_behavior(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let generated = quote! {
        pulsar_reflection::inventory::submit! {
            pulsar_reflection::RuntimeBehaviorRegistration {
                class_name: <#name as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                sync: <#name as pulsar_reflection::ComponentRuntimeBehavior>::sync_component,
            }
        }
    };

    generated.into()
}

/// Check if a field has the #[property] attribute
fn has_property_attr(field: &Field) -> bool {
    field
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("property"))
}

/// Extract category from struct-level attributes
fn extract_category(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("category") {
            if let Ok(Meta::NameValue(MetaNameValue {
                value: syn::Expr::Lit(expr_lit),
                ..
            })) = attr.parse_args()
            {
                if let Lit::Str(lit_str) = &expr_lit.lit {
                    return Some(lit_str.value());
                }
            }
        }
    }
    None
}

/// Generate PropertyMetadata for a single field
///
/// NOW USES RUNTIME TYPE REFLECTION - NO MORE ENUM INFERENCE!
fn generate_property_metadata(
    field: &Field,
    struct_name: &syn::Ident,
    category: &Option<String>,
) -> proc_macro2::TokenStream {
    let field_name = field.ident.as_ref().unwrap();
    let field_name_str = field_name.to_string();
    let display_name = capitalize_first(&field_name_str);
    let field_type = &field.ty;

    // Generate category option
    let category_expr = if let Some(cat) = category {
        quote! { Some(#cat) }
    } else {
        quote! { None }
    };

    // Use Reflectable::type_info() to get runtime type information
    // This eliminates the need for PropertyType enum inference!
    let type_info_expr = quote! {
        <#field_type as pulsar_reflection::Reflectable>::type_info()
    };

    // Generate getter closure that returns Box<dyn Any>
    let getter = quote! {
        Box::new(|obj: &dyn pulsar_reflection::EngineClass| -> Box<dyn std::any::Any> {
            let concrete = obj.as_any().downcast_ref::<#struct_name>().unwrap();
            Box::new(concrete.#field_name.clone())
        })
    };

    // Generate setter closure that accepts Box<dyn Any>
    let setter = quote! {
        Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, value: Box<dyn std::any::Any>| {
            let concrete = obj.as_any_mut().downcast_mut::<#struct_name>().unwrap();
            if let Some(typed_value) = value.downcast_ref::<#field_type>() {
                concrete.#field_name = typed_value.clone();
            } else {
                tracing::warn!(
                    "Type mismatch in property setter for {}.{}: expected {}, got {:?}",
                    stringify!(#struct_name),
                    #field_name_str,
                    stringify!(#field_type),
                    value.type_id()
                );
            }
        })
    };

    quote! {
        pulsar_reflection::PropertyMetadata {
            name: #field_name_str,
            display_name: #display_name.to_string(),
            category: #category_expr,
            type_info: #type_info_expr,
            getter: #getter,
            setter: #setter,
        }
    }
}





/// Capitalize first letter of a string
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
