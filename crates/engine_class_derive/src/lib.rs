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
    Attribute, Data, DeriveInput, Field, Fields, FnArg, ImplItem, ItemImpl, Lit, Meta,
    MetaNameValue, Pat, PatType, ReturnType, Type, parse_macro_input,
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
    let (property_impls, property_fields): (Vec<_>, Vec<_>) = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields) => {
                let props: Vec<_> = fields
                    .named
                    .iter()
                    .filter_map(|field| {
                        if has_property_attr(field) {
                            Some((generate_property_metadata(field, name, &category), field))
                        } else {
                            None
                        }
                    })
                    .collect();
                props.into_iter().unzip()
            }
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

    // Generate auto-property methods (getters and setters)
    let property_method_items = generate_property_method_items(&property_fields, name);

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

            fn get_methods() -> Vec<pulsar_reflection::MethodMetadata> {
                let mut methods = Vec::new();

                // Auto-generated property getter/setter methods
                methods.extend(vec![#(#property_method_items),*]);

                // Manually registered methods from #[component_methods]
                for registration in pulsar_reflection::inventory::iter::<pulsar_reflection::ComponentMethodRegistration>() {
                    if registration.class_name == stringify!(#name) {
                        methods.extend((registration.methods)());
                    }
                }

                methods
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

        // Register property methods with inventory (for registry lookup)
        pulsar_reflection::inventory::submit! {
            pulsar_reflection::ComponentMethodRegistration {
                class_name: stringify!(#name),
                methods: || vec![#(#property_method_items),*],
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

#[proc_macro_attribute]
pub fn component_methods(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let impl_block = parse_macro_input!(item as ItemImpl);

    // Extract the type name from the impl block
    let type_name = match &*impl_block.self_ty {
        Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last() {
                segment.ident.clone()
            } else {
                return syn::Error::new_spanned(&impl_block.self_ty, "Expected type path")
                    .to_compile_error()
                    .into();
            }
        }
        _ => {
            return syn::Error::new_spanned(&impl_block.self_ty, "Expected type path")
                .to_compile_error()
                .into();
        }
    };

    let type_name_str = type_name.to_string();

    // Find all methods marked with #[method]
    let mut method_metadata_items = Vec::new();

    for item in &impl_block.items {
        if let ImplItem::Fn(method) = item {
            // Check if method has #[method] attribute
            let method_attr = method.attrs.iter().find(|attr| attr.path().is_ident("method"));

            if let Some(attr) = method_attr {
                // Parse the method
                let method_ident = &method.sig.ident;
                let method_name_str = method_ident.to_string();
                let display_name = capitalize_first(&method_name_str.replace('_', " "));

                // Extract method type and category from attribute
                let (method_type, category) = parse_method_attribute(attr);

                // Extract parameters (skip &self / &mut self)
                let mut params = Vec::new();
                for input in &method.sig.inputs {
                    if let FnArg::Typed(PatType { pat, ty, .. }) = input {
                        if let Pat::Ident(pat_ident) = &**pat {
                            let param_name = pat_ident.ident.to_string();
                            let param_type = infer_method_param_type(ty);
                            params.push((param_name, param_type));
                        }
                    }
                }

                // Extract return type
                let return_type = match &method.sig.output {
                    ReturnType::Default => None,
                    ReturnType::Type(_, ty) => {
                        let return_prop_type = infer_method_param_type(ty);
                        Some(return_prop_type)
                    }
                };

                // Generate param metadata
                let param_metadata: Vec<_> = params
                    .iter()
                    .map(|(name, ty)| {
                        quote! {
                            pulsar_reflection::MethodParameter {
                                name: #name,
                                param_type: #ty,
                            }
                        }
                    })
                    .collect();

                // Generate return type metadata
                let return_metadata = if let Some(ret_ty) = &return_type {
                    quote! {
                        Some(pulsar_reflection::MethodReturnType {
                            return_type: #ret_ty,
                        })
                    }
                } else {
                    quote! { None }
                };

                // Determine mutability (for downcasting)
                let is_mut = method
                    .sig
                    .inputs
                    .iter()
                    .any(|arg| matches!(arg, FnArg::Receiver(r) if r.mutability.is_some()));

                // Generate caller closure
                let param_reads: Vec<_> = params
                    .iter()
                    .enumerate()
                    .map(|(i, (_, ty))| {
                        let as_method = get_property_value_as_method(ty);
                        quote! {
                            args.get(#i).and_then(|v| #as_method(v)).expect("Invalid argument type")
                        }
                    })
                    .collect();

                let caller = if is_mut {
                    let result_conversion = if return_type.is_some() {
                        let to_prop_value = get_to_property_value(&return_type.as_ref().unwrap());
                        quote! { Some(#to_prop_value(result)) }
                    } else {
                        quote! { None }
                    };

                    quote! {
                        Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, args: Vec<pulsar_reflection::PropertyValue>| {
                            let concrete = obj.as_any_mut().downcast_mut::<#type_name>().expect("Downcast failed");
                            let result = concrete.#method_ident(#(#param_reads),*);
                            #result_conversion
                        })
                    }
                } else {
                    let result_conversion = if return_type.is_some() {
                        let to_prop_value = get_to_property_value(&return_type.as_ref().unwrap());
                        quote! { Some(#to_prop_value(result)) }
                    } else {
                        quote! { None }
                    };

                    quote! {
                        Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, args: Vec<pulsar_reflection::PropertyValue>| {
                            let concrete = obj.as_any().downcast_ref::<#type_name>().expect("Downcast failed");
                            let result = concrete.#method_ident(#(#param_reads),*);
                            #result_conversion
                        })
                    }
                };

                // Generate MethodMetadata
                let category_expr = if let Some(cat) = category {
                    quote! { Some(#cat) }
                } else {
                    quote! { None }
                };

                method_metadata_items.push(quote! {
                    pulsar_reflection::MethodMetadata {
                        name: #method_name_str,
                        display_name: #display_name.to_string(),
                        category: #category_expr,
                        params: vec![#(#param_metadata),*],
                        return_type: #return_metadata,
                        method_type: #method_type,
                        caller: #caller,
                    }
                });
            }
        }
    }

    // Generate inventory registration
    let registration = if !method_metadata_items.is_empty() {
        quote! {
            pulsar_reflection::inventory::submit! {
                pulsar_reflection::ComponentMethodRegistration {
                    class_name: #type_name_str,
                    methods: || vec![#(#method_metadata_items),*],
                }
            }
        }
    } else {
        quote! {}
    };

    // Output: original impl block + registration
    let output = quote! {
        #impl_block
        #registration
    };

    output.into()
}

/// Parse #[method(...)] attribute to extract type and category
fn parse_method_attribute(attr: &Attribute) -> (proc_macro2::TokenStream, Option<String>) {
    let mut method_type = quote! { pulsar_reflection::MethodType::Pure };
    let mut category = None;

    if let Meta::List(meta_list) = &attr.meta {
        let tokens_str = meta_list.tokens.to_string();

        // Parse type
        if tokens_str.contains("type") {
            if tokens_str.contains("MethodType :: Pure") || tokens_str.contains("Pure") {
                method_type = quote! { pulsar_reflection::MethodType::Pure };
            } else if tokens_str.contains("MethodType :: Fn") || tokens_str.contains("Fn") {
                method_type = quote! { pulsar_reflection::MethodType::Fn };
            } else if tokens_str.contains("MethodType :: ControlFlow") || tokens_str.contains("ControlFlow") {
                method_type = quote! { pulsar_reflection::MethodType::ControlFlow };
            }
        }

        // Parse category
        if let Some(start) = tokens_str.find("category") {
            if let Some(quote_start) = tokens_str[start..].find('"') {
                let rest = &tokens_str[start + quote_start + 1..];
                if let Some(quote_end) = rest.find('"') {
                    category = Some(rest[..quote_end].to_string());
                }
            }
        }
    }

    (method_type, category)
}

/// Infer PropertyType from method parameter/return type
fn infer_method_param_type(ty: &Type) -> proc_macro2::TokenStream {
    if let Type::Path(type_path) = ty {
        let type_str = quote!(#type_path).to_string();

        match type_str.as_str() {
            "f32" => quote! { pulsar_reflection::PropertyType::F32 { min: None, max: None, step: None } },
            "i32" => quote! { pulsar_reflection::PropertyType::I32 { min: None, max: None } },
            "bool" => quote! { pulsar_reflection::PropertyType::Bool },
            "String" => quote! { pulsar_reflection::PropertyType::String { max_length: None } },
            "[f32; 3]" | "[f32 ; 3]" => quote! { pulsar_reflection::PropertyType::Vec3 },
            "[f32; 4]" | "[f32 ; 4]" => quote! { pulsar_reflection::PropertyType::Color },
            _ => quote! { pulsar_reflection::PropertyType::String { max_length: None } },
        }
    } else {
        quote! { pulsar_reflection::PropertyType::String { max_length: None } }
    }
}

/// Get the PropertyValue::as_* method for a given PropertyType
fn get_property_value_as_method(prop_type: &proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let type_str = prop_type.to_string();

    if type_str.contains("F32") {
        quote! { pulsar_reflection::PropertyValue::as_f32 }
    } else if type_str.contains("I32") {
        quote! { pulsar_reflection::PropertyValue::as_i32 }
    } else if type_str.contains("Bool") {
        quote! { pulsar_reflection::PropertyValue::as_bool }
    } else if type_str.contains("String") {
        quote! { |v| v.as_string().map(|s| s.to_string()) }
    } else if type_str.contains("Vec3") {
        quote! { pulsar_reflection::PropertyValue::as_vec3 }
    } else if type_str.contains("Color") {
        quote! { pulsar_reflection::PropertyValue::as_color }
    } else {
        quote! { |v| v.as_string().map(|s| s.to_string()) }
    }
}

/// Get conversion from result to PropertyValue
fn get_to_property_value(prop_type: &proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let type_str = prop_type.to_string();

    if type_str.contains("F32") {
        quote! { pulsar_reflection::PropertyValue::F32 }
    } else if type_str.contains("I32") {
        quote! { pulsar_reflection::PropertyValue::I32 }
    } else if type_str.contains("Bool") {
        quote! { pulsar_reflection::PropertyValue::Bool }
    } else if type_str.contains("String") {
        quote! { pulsar_reflection::PropertyValue::String }
    } else if type_str.contains("Vec3") {
        quote! { pulsar_reflection::PropertyValue::Vec3 }
    } else if type_str.contains("Color") {
        quote! { pulsar_reflection::PropertyValue::Color }
    } else {
        quote! { |v| pulsar_reflection::PropertyValue::String(format!("{:?}", v)) }
    }
}

/// Generate getter and setter method metadata items for properties
fn generate_property_method_items(
    fields: &[&Field],
    struct_name: &syn::Ident,
) -> Vec<proc_macro2::TokenStream> {
    let mut method_items = Vec::new();

    for field in fields {
        // Check if this type is supported for method generation
        let property_value_getter = match get_property_value_as_method_for_field(&field.ty) {
            Some(getter) => getter,
            None => continue, // Skip unsupported types
        };

        let field_name = field.ident.as_ref().unwrap();
        let field_name_str = field_name.to_string();
        let getter_name = format!("get_{}", field_name_str);
        let setter_name = format!("set_{}", field_name_str);
        let getter_display = capitalize_first(&format!("Get {}", field_name_str));
        let setter_display = capitalize_first(&format!("Set {}", field_name_str));

        let property_type = infer_property_type(&field.ty, &field.attrs);
        let getter_return_type = property_type.clone();
        let setter_param_type = property_type.clone();

        // Generate getter method metadata
        let property_value_expr = infer_property_value(&field.ty, field_name);

        method_items.push(quote! {
            pulsar_reflection::MethodMetadata {
                name: #getter_name,
                display_name: #getter_display.to_string(),
                category: None,
                params: vec![],
                return_type: Some(pulsar_reflection::MethodReturnType {
                    return_type: #getter_return_type,
                }),
                method_type: pulsar_reflection::MethodType::Pure,
                caller: Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, _args: Vec<pulsar_reflection::PropertyValue>| {
                    let concrete = obj.as_any().downcast_ref::<#struct_name>().unwrap();
                    let property_value = #property_value_expr;
                    Some(property_value)
                }),
            }
        });

        // Generate setter method metadata
        method_items.push(quote! {
            pulsar_reflection::MethodMetadata {
                name: #setter_name,
                display_name: #setter_display.to_string(),
                category: None,
                params: vec![
                    pulsar_reflection::MethodParameter {
                        name: "value",
                        param_type: #setter_param_type,
                    }
                ],
                return_type: None,
                method_type: pulsar_reflection::MethodType::Fn,
                caller: Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, args: Vec<pulsar_reflection::PropertyValue>| {
                    let concrete = obj.as_any_mut().downcast_mut::<#struct_name>().unwrap();
                    if let Some(value) = args.get(0).and_then(#property_value_getter) {
                        concrete.#field_name = value;
                    }
                    None
                }),
            }
        });
    }

    method_items
}

/// Get the PropertyValue::as_* method for a given field type
/// Returns None if the type is not supported for method generation
fn get_property_value_as_method_for_field(ty: &Type) -> Option<proc_macro2::TokenStream> {
    if let Type::Path(type_path) = ty {
        let type_str = quote!(#type_path).to_string();

        match type_str.as_str() {
            "f32" => Some(quote! { pulsar_reflection::PropertyValue::as_f32 }),
            "i32" => Some(quote! { pulsar_reflection::PropertyValue::as_i32 }),
            "bool" => Some(quote! { pulsar_reflection::PropertyValue::as_bool }),
            "String" => Some(quote! { |v| v.as_string().map(|s| s.to_string()) }),
            "[f32; 3]" | "[f32 ; 3]" => Some(quote! { pulsar_reflection::PropertyValue::as_vec3 }),
            "[f32; 4]" | "[f32 ; 4]" => Some(quote! { pulsar_reflection::PropertyValue::as_color }),
            _ => None, // Unsupported type
        }
    } else {
        None
    }
}
