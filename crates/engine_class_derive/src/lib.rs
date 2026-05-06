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
use syn::{parse_macro_input, DeriveInput, Data, Fields, Field, Attribute, Lit, Meta, MetaNameValue, Type};

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
            Fields::Named(fields) => {
                fields.named.iter().filter_map(|field| {
                    if has_property_attr(field) {
                        Some(generate_property_metadata(field, name, &category))
                    } else {
                        None
                    }
                }).collect::<Vec<_>>()
            }
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "EngineClass can only be derived for structs with named fields"
                ).to_compile_error().into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input,
                "EngineClass can only be derived for structs"
            ).to_compile_error().into();
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

/// Check if a field has the #[property] attribute
fn has_property_attr(field: &Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("property"))
}

/// Extract category from struct-level attributes
fn extract_category(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("category") {
            if let Ok(Meta::NameValue(MetaNameValue { value: syn::Expr::Lit(expr_lit), .. })) = attr.parse_args() {
                if let Lit::Str(lit_str) = &expr_lit.lit {
                    return Some(lit_str.value());
                }
            }
        }
    }
    None
}

/// Generate PropertyMetadata for a single field
fn generate_property_metadata(field: &Field, struct_name: &syn::Ident, category: &Option<String>) -> proc_macro2::TokenStream {
    let field_name = field.ident.as_ref().unwrap();
    let field_name_str = field_name.to_string();
    let display_name = capitalize_first(&field_name_str);

    // Extract property attributes (min, max, step)
    let property_type = infer_property_type(&field.ty, &field.attrs);
    let property_value_getter = infer_property_value(&field.ty, field_name);
    let property_value_setter = infer_property_setter(&field.ty, field_name);

    // Generate category option
    let category_expr = if let Some(cat) = category {
        quote! { Some(#cat) }
    } else {
        quote! { None }
    };

    // Generate getter closure
    let getter = quote! {
        Box::new(|obj: &dyn pulsar_reflection::EngineClass| {
            let concrete = obj.as_any().downcast_ref::<#struct_name>().unwrap();
            #property_value_getter
        })
    };

    // Generate setter closure
    let setter = quote! {
        Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, value: pulsar_reflection::PropertyValue| {
            let concrete = obj.as_any_mut().downcast_mut::<#struct_name>().unwrap();
            #property_value_setter
        })
    };

    quote! {
        pulsar_reflection::PropertyMetadata {
            name: #field_name_str,
            display_name: #display_name.to_string(),
            category: #category_expr,
            property_type: #property_type,
            getter: #getter,
            setter: #setter,
        }
    }
}

/// Infer PropertyValue getter from field type
fn infer_property_value(ty: &Type, field_name: &syn::Ident) -> proc_macro2::TokenStream {
    if let Type::Path(type_path) = ty {
        let type_str = quote!(#type_path).to_string();

        match type_str.as_str() {
            "f32" => {
                quote! {
                    pulsar_reflection::PropertyValue::F32(concrete.#field_name)
                }
            }
            "i32" => {
                quote! {
                    pulsar_reflection::PropertyValue::I32(concrete.#field_name)
                }
            }
            "bool" => {
                quote! {
                    pulsar_reflection::PropertyValue::Bool(concrete.#field_name)
                }
            }
            "String" => {
                quote! {
                    pulsar_reflection::PropertyValue::String(concrete.#field_name.clone())
                }
            }
            "[f32; 3]" | "[f32 ; 3]" => {
                quote! {
                    pulsar_reflection::PropertyValue::Vec3(concrete.#field_name)
                }
            }
            "[f32; 4]" | "[f32 ; 4]" => {
                quote! {
                    pulsar_reflection::PropertyValue::Color(concrete.#field_name)
                }
            }
            _ if type_str.starts_with("Vec <") || type_str.starts_with("Vec<") => {
                // For Vec<T>, serialize each item to PropertyValue
                quote! {
                    pulsar_reflection::PropertyValue::Vec(
                        concrete.#field_name.iter().map(|item| {
                            pulsar_reflection::PropertyValue::String(format!("{:?}", item))
                        }).collect()
                    )
                }
            }
            _ => {
                // Default to String for unknown types (serialize as debug)
                quote! {
                    pulsar_reflection::PropertyValue::String(format!("{:?}", concrete.#field_name))
                }
            }
        }
    } else {
        // Fallback
        quote! {
            pulsar_reflection::PropertyValue::String(String::from("unsupported"))
        }
    }
}

/// Infer PropertyValue setter from field type
fn infer_property_setter(ty: &Type, field_name: &syn::Ident) -> proc_macro2::TokenStream {
    if let Type::Path(type_path) = ty {
        let type_str = quote!(#type_path).to_string();

        match type_str.as_str() {
            "f32" => {
                quote! {
                    if let Some(new_value) = value.as_f32() {
                        concrete.#field_name = new_value;
                    }
                }
            }
            "i32" => {
                quote! {
                    if let Some(new_value) = value.as_i32() {
                        concrete.#field_name = new_value;
                    }
                }
            }
            "bool" => {
                quote! {
                    if let Some(new_value) = value.as_bool() {
                        concrete.#field_name = new_value;
                    }
                }
            }
            "String" => {
                quote! {
                    if let Some(new_value) = value.as_string() {
                        concrete.#field_name = new_value.to_string();
                    }
                }
            }
            "[f32; 3]" | "[f32 ; 3]" => {
                quote! {
                    if let Some(new_value) = value.as_vec3() {
                        concrete.#field_name = new_value;
                    }
                }
            }
            "[f32; 4]" | "[f32 ; 4]" => {
                quote! {
                    if let Some(new_value) = value.as_color() {
                        concrete.#field_name = new_value;
                    }
                }
            }
            _ if type_str.starts_with("Vec <") || type_str.starts_with("Vec<") => {
                // For Vec<T>, we can't easily deserialize without knowing T
                // For now, just log that it's not supported in setter
                quote! {
                    // Vec<T> setters not yet fully supported - use direct mutation
                    tracing::warn!("Vec<T> property setter called but not fully implemented");
                }
            }
            _ => {
                // Unknown types can't be set
                quote! {
                    tracing::warn!("Attempted to set unsupported property type");
                }
            }
        }
    } else {
        // Fallback
        quote! {
            tracing::warn!("Attempted to set property with unsupported type structure");
        }
    }
}

/// Infer PropertyType from field type and attributes
fn infer_property_type(ty: &Type, attrs: &[Attribute]) -> proc_macro2::TokenStream {
    // Extract min/max/step from attributes
    let (min, max, step) = extract_numeric_constraints(attrs);

    // Match on type
    if let Type::Path(type_path) = ty {
        let type_str = quote!(#type_path).to_string();

        match type_str.as_str() {
            "f32" => {
                quote! {
                    pulsar_reflection::PropertyType::F32 {
                        min: #min,
                        max: #max,
                        step: #step,
                    }
                }
            }
            "i32" => {
                quote! {
                    pulsar_reflection::PropertyType::I32 {
                        min: #min,
                        max: #max,
                    }
                }
            }
            "bool" => {
                quote! { pulsar_reflection::PropertyType::Bool }
            }
            "String" => {
                quote! {
                    pulsar_reflection::PropertyType::String {
                        max_length: None,
                    }
                }
            }
            "[f32; 3]" | "[f32 ; 3]" => {
                quote! { pulsar_reflection::PropertyType::Vec3 }
            }
            "[f32; 4]" | "[f32 ; 4]" => {
                quote! { pulsar_reflection::PropertyType::Color }
            }
            _ => {
                // Default to String for unknown types
                quote! {
                    pulsar_reflection::PropertyType::String {
                        max_length: None,
                    }
                }
            }
        }
    } else {
        // Fallback
        quote! {
            pulsar_reflection::PropertyType::String {
                max_length: None,
            }
        }
    }
}

/// Extract numeric constraints from #[property(min = ..., max = ..., step = ...)]
fn extract_numeric_constraints(attrs: &[Attribute]) -> (proc_macro2::TokenStream, proc_macro2::TokenStream, proc_macro2::TokenStream) {
    let mut min = quote! { None };
    let mut max = quote! { None };
    let mut step = quote! { None };

    for attr in attrs {
        if !attr.path().is_ident("property") {
            continue;
        }

        // Parse the attribute's contents
        if let Meta::List(meta_list) = &attr.meta {
            // TODO: Properly parse nested meta items
            // For now, this is a simplified version
            let tokens = &meta_list.tokens;
            let tokens_str = tokens.to_string();

            // Very basic parsing (not robust, but works for simple cases)
            if tokens_str.contains("min") {
                // Extract min value (simplified)
                if let Some(start) = tokens_str.find("min = ") {
                    let rest = &tokens_str[start + 6..];
                    if let Some(end) = rest.find(|c: char| c == ',' || c == ')') {
                        let value_str = &rest[..end].trim();
                        if let Ok(value) = value_str.parse::<f32>() {
                            min = quote! { Some(#value) };
                        }
                    }
                }
            }

            if tokens_str.contains("max") {
                if let Some(start) = tokens_str.find("max = ") {
                    let rest = &tokens_str[start + 6..];
                    if let Some(end) = rest.find(|c: char| c == ',' || c == ')') {
                        let value_str = &rest[..end].trim();
                        if let Ok(value) = value_str.parse::<f32>() {
                            max = quote! { Some(#value) };
                        }
                    }
                }
            }

            if tokens_str.contains("step") {
                if let Some(start) = tokens_str.find("step = ") {
                    let rest = &tokens_str[start + 7..];
                    if let Some(end) = rest.find(|c: char| c == ',' || c == ')') {
                        let value_str = &rest[..end].trim();
                        if let Ok(value) = value_str.parse::<f32>() {
                            step = quote! { Some(#value) };
                        }
                    }
                }
            }
        }
    }

    (min, max, step)
}

/// Capitalize first letter of a string
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}
