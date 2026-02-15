//! UI Generation Macros - Compile-time type introspection for automatic UI generation
//!
//! This crate provides procedural macros that analyze Rust types at compile time
//! and generate field metadata that can be used to create data-driven UIs.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, Data, Fields, Type};

/// Generate field metadata for enum variants
///
/// This macro analyzes an enum's fields at compile time and generates a method
/// that returns field metadata (name, type) for each variant.
///
/// # Example
/// ```rust
/// #[generate_field_metadata]
/// pub enum Component {
///     Material {
///         color: [f32; 4],
///         metallic: f32,
///         roughness: f32,
///     },
///     RigidBody {
///         mass: f32,
///         kinematic: bool,
///     }
/// }
/// ```
///
/// Generates:
/// ```rust
/// impl Component {
///     pub fn field_metadata(&self) -> Vec<(&'static str, FieldTypeInfo)> { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn generate_field_metadata(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as DeriveInput);
    let enum_name = &input.ident;
    
    let variants = match &input.data {
        Data::Enum(data_enum) => &data_enum.variants,
        _ => panic!("generate_field_metadata can only be used on enums"),
    };
    
    let mut variant_match_arms = vec![];
    
    for variant in variants {
        let variant_name = &variant.ident;
        
        let fields = match &variant.fields {
            Fields::Named(fields) => &fields.named,
            _ => {
                // For unit or tuple variants, return empty vec
                variant_match_arms.push(quote! {
                    Self::#variant_name { .. } | Self::#variant_name => vec![],
                });
                continue;
            }
        };
        
        let mut field_info = vec![];
        
        for field in fields {
            let field_name = field.ident.as_ref().unwrap();
            let field_name_str = field_name.to_string();
            let field_ty = &field.ty;
            
            let type_info = rust_type_to_type_info(field_ty);
            
            field_info.push(quote! {
                (#field_name_str, #type_info)
            });
        }
        
        variant_match_arms.push(quote! {
            Self::#variant_name { .. } => vec![
                #(#field_info),*
            ],
        });
    }
    
    // Collect variant names for the variant_name method
    let variant_names_for_method: Vec<_> = variants.iter().map(|v| &v.ident).collect();
    
    // Generate the original enum + new method
    let expanded = quote! {
        #input
        
        impl #enum_name {
            /// Get field metadata for this component variant
            /// Returns Vec<(field_name, field_type_info)>
            pub fn field_metadata(&self) -> Vec<(&'static str, FieldTypeInfo)> {
                match self {
                    #(#variant_match_arms)*
                }
            }
            
            /// Get variant name as string
            pub fn variant_name(&self) -> &'static str {
                match self {
                    #(Self::#variant_names_for_method { .. } => stringify!(#variant_names_for_method),)*
                }
            }
        }
    };
    
    TokenStream::from(expanded)
}

/// Convert Rust type to FieldTypeInfo enum
fn rust_type_to_type_info(ty: &Type) -> proc_macro2::TokenStream {
    match ty {
        Type::Path(type_path) => {
            let type_name = &type_path.path.segments.last().unwrap().ident.to_string();
            
            match type_name.as_str() {
                "f32" => quote! { FieldTypeInfo::F32 },
                "f64" => quote! { FieldTypeInfo::F64 },
                "i32" => quote! { FieldTypeInfo::I32 },
                "i64" => quote! { FieldTypeInfo::I64 },
                "u32" => quote! { FieldTypeInfo::U32 },
                "u64" => quote! { FieldTypeInfo::U64 },
                "bool" => quote! { FieldTypeInfo::Bool },
                "String" => quote! { FieldTypeInfo::String },
                _ => quote! { FieldTypeInfo::Other(stringify!(#type_name)) },
            }
        },
        Type::Array(type_array) => {
            // Check if it's [f32; 3] or [f32; 4]
            if let Type::Path(elem_path) = &*type_array.elem {
                let elem_name = elem_path.path.segments.last().unwrap().ident.to_string();
                if elem_name == "f32" {
                    if let syn::Expr::Lit(lit) = &type_array.len {
                        if let syn::Lit::Int(int_lit) = &lit.lit {
                            if let Ok(len) = int_lit.base10_parse::<usize>() {
                                return quote! { FieldTypeInfo::F32Array(#len) };
                            }
                        }
                    }
                }
            }
            quote! { FieldTypeInfo::Other("Array") }
        },
        _ => quote! { FieldTypeInfo::Other("Unknown") },
    }
}



