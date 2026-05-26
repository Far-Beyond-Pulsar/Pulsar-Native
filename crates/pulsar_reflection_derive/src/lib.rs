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
    parse_macro_input, Data, DeriveInput, Expr, Field, Fields, Ident, Item, ItemType,
    Meta, Path, Type,
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

enum PrimitiveCloneMode {
    Copy,
    Clone,
    Ref,
}

struct PrimitiveAliasConfig {
    serialize_method: Ident,
    deserialize_method: Ident,
    structure: Ident,
    clone_mode: PrimitiveCloneMode,
}

// ── editor fn path (optional) ─────────────────────────────────────────────────

/// Attribute macro for runtime type registration.
///
/// Primitive alias mode:
///
/// ```ignore
/// #[pulsar_type(primitive)]
/// type RegisteredF32 = f32;
/// ```
#[proc_macro_attribute]
pub fn pulsar_type(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated);
    let input = parse_macro_input!(item as Item);

    let item_type = match input {
        Item::Type(item_type) => item_type,
        other => {
            return syn::Error::new_spanned(
                other,
                "#[pulsar_type] currently supports type aliases for primitive registration",
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

fn expand_primitive_alias(args: Vec<&Meta>, item_type: &ItemType) -> syn::Result<proc_macro2::TokenStream> {
    if !item_type.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &item_type.generics,
            "#[pulsar_type] primitive aliases do not support generics",
        ));
    }

    let mut primitive_mode = args.is_empty();
    let mut override_serialize: Option<Ident> = None;
    let mut override_deserialize: Option<Ident> = None;
    let mut override_structure: Option<Ident> = None;
    let mut override_clone: Option<PrimitiveCloneMode> = None;
    let mut override_serialize_json_with: Option<Path> = None;
    let mut override_deserialize_json_with: Option<Path> = None;
    let mut override_editor: Option<Path> = None;

    for meta in args {
        match meta {
            Meta::Path(path) if path.is_ident("primitive") => {
                primitive_mode = true;
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("serialize_method") => {
                override_serialize = Some(parse_ident_expr(&name_value.value, "serialize_method")?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("deserialize_method") => {
                override_deserialize = Some(parse_ident_expr(&name_value.value, "deserialize_method")?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("structure") => {
                override_structure = Some(parse_ident_expr(&name_value.value, "structure")?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("clone") => {
                let clone_ident = parse_ident_expr(&name_value.value, "clone")?;
                let clone_mode = match clone_ident.to_string().as_str() {
                    "copy" => PrimitiveCloneMode::Copy,
                    "clone" => PrimitiveCloneMode::Clone,
                    "ref" => PrimitiveCloneMode::Ref,
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &name_value.value,
                            "clone must be one of: copy, clone, ref",
                        ));
                    }
                };
                override_clone = Some(clone_mode);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("serialize_json_with") => {
                override_serialize_json_with = Some(parse_path_expr(
                    &name_value.value,
                    "serialize_json_with",
                )?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("deserialize_json_with") => {
                override_deserialize_json_with = Some(parse_path_expr(
                    &name_value.value,
                    "deserialize_json_with",
                )?);
            }
            Meta::NameValue(name_value) if name_value.path.is_ident("editor") => {
                override_editor = Some(parse_path_expr(&name_value.value, "editor")?);
            }
            _ => {
                return Err(syn::Error::new_spanned(
                    meta,
                    "unsupported #[pulsar_type(...)] argument",
                ));
            }
        }
    }

    if !primitive_mode {
        return Err(syn::Error::new_spanned(
            item_type,
            "#[pulsar_type] requires primitive mode for type aliases (use #[pulsar_type(primitive)])",
        ));
    }

    // Only try to infer if we don't have custom serialize/deserialize functions
    let needs_inference = override_serialize_json_with.is_none() || override_deserialize_json_with.is_none();

    let (serialize_method, deserialize_method, structure, _clone_mode) = if needs_inference {
        let inferred = infer_primitive_alias_config(&item_type.ty)?;
        (
            override_serialize.unwrap_or(inferred.serialize_method),
            override_deserialize.unwrap_or(inferred.deserialize_method),
            override_structure.unwrap_or(inferred.structure),
            override_clone.unwrap_or(inferred.clone_mode),
        )
    } else {
        // When custom functions are provided, use defaults
        (
            override_serialize.unwrap_or_else(|| format_ident!("serialize_generic")),
            override_deserialize.unwrap_or_else(|| format_ident!("deserialize_generic")),
            override_structure.unwrap_or_else(|| format_ident!("Primitive")),
            override_clone.unwrap_or(PrimitiveCloneMode::Clone),
        )
    };

    let alias_ident = &item_type.ident;
    let target_ty = &item_type.ty;
    let type_info_name = format_ident!("{}_TYPE_INFO", alias_ident.to_string().to_uppercase());

    let json_serialize_value = if let Some(path) = override_serialize_json_with {
        quote! { #path(typed) }
    } else {
        let value_expr = primitive_serialize_value_expr(&serialize_method, quote! { typed })?;
        quote! { Ok(#value_expr) }
    };
    let json_deserialize_value = if let Some(path) = override_deserialize_json_with {
        quote! { #path(value)? }
    } else {
        primitive_deserialize_value_expr(&deserialize_method, quote! { value })?
    };

    // Optional UI property-editor hint — only emitted when `editor = fn` is provided.
    let editor_submit = if let Some(editor_fn) = override_editor {
        quote! {
            ::pulsar_reflection::inventory::submit! {
                ::pulsar_reflection::UiPropertyEditorHint {
                    type_id: ::std::any::TypeId::of::<#target_ty>(),
                    // Erase the concrete fn-pointer type to `fn()` via the
                    // pulsar_reflection helper so this submit can live in any
                    // crate without a GPUI dependency.  The `ui_common` registry
                    // transmutes it back under the documented safety contract:
                    // the function must have signature
                    //   `fn(&PropertyEditorArgs<'_>, &gpui::App) -> gpui::AnyElement`.
                    // SAFETY: the `editor = fn` author guarantees the signature.
                    fn_ptr: unsafe { ::pulsar_reflection::erase_property_editor_fn_ptr(#editor_fn) },
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
            structure: ::pulsar_reflection::TypeStructure::#structure,
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
                    let typed: #target_ty = #json_deserialize_value;
                    Ok(::std::boxed::Box::new(typed) as ::std::boxed::Box<dyn ::std::any::Any>)
                },
            }
        }

        #editor_submit
    })
}

fn parse_ident_expr(expr: &Expr, arg_name: &str) -> syn::Result<Ident> {
    if let Expr::Path(path) = expr {
        if let Some(ident) = path.path.get_ident() {
            return Ok(ident.clone());
        }
    }

    Err(syn::Error::new_spanned(
        expr,
        format!("{} must be an identifier", arg_name),
    ))
}

fn parse_path_expr(expr: &Expr, arg_name: &str) -> syn::Result<Path> {
    if let Expr::Path(path) = expr {
        return Ok(path.path.clone());
    }

    Err(syn::Error::new_spanned(
        expr,
        format!("{} must be a function path", arg_name),
    ))
}

fn infer_primitive_alias_config(ty: &Type) -> syn::Result<PrimitiveAliasConfig> {
    if is_path_ident(ty, "f32") {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_f32"),
            deserialize_method: format_ident!("deserialize_f32"),
            structure: format_ident!("Primitive"),
            clone_mode: PrimitiveCloneMode::Copy,
        });
    }

    if is_path_ident(ty, "i32") {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_i32"),
            deserialize_method: format_ident!("deserialize_i32"),
            structure: format_ident!("Primitive"),
            clone_mode: PrimitiveCloneMode::Copy,
        });
    }

    if is_path_ident(ty, "u64") {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_u64"),
            deserialize_method: format_ident!("deserialize_u64"),
            structure: format_ident!("Primitive"),
            clone_mode: PrimitiveCloneMode::Copy,
        });
    }

    if is_path_ident(ty, "bool") {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_bool"),
            deserialize_method: format_ident!("deserialize_bool"),
            structure: format_ident!("Primitive"),
            clone_mode: PrimitiveCloneMode::Copy,
        });
    }

    if is_string_type(ty) {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_string"),
            deserialize_method: format_ident!("deserialize_string"),
            structure: format_ident!("String"),
            clone_mode: PrimitiveCloneMode::Ref,
        });
    }

    if is_serde_json_value_type(ty) {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_json_value"),
            deserialize_method: format_ident!("deserialize_json_value"),
            structure: format_ident!("Primitive"),
            clone_mode: PrimitiveCloneMode::Ref,
        });
    }

    if is_f32_array_of_len(ty, 3) {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_vec3"),
            deserialize_method: format_ident!("deserialize_vec3"),
            structure: format_ident!("Primitive"),
            clone_mode: PrimitiveCloneMode::Copy,
        });
    }

    if is_f32_array_of_len(ty, 4) {
        return Ok(PrimitiveAliasConfig {
            serialize_method: format_ident!("serialize_color"),
            deserialize_method: format_ident!("deserialize_color"),
            structure: format_ident!("Primitive"),
            clone_mode: PrimitiveCloneMode::Copy,
        });
    }

    Err(syn::Error::new_spanned(
        ty,
        "unsupported primitive alias type for #[pulsar_type(primitive)]",
    ))
}

fn is_path_ident(ty: &Type, ident: &str) -> bool {
    if let Type::Path(type_path) = ty {
        return type_path.path.is_ident(ident);
    }
    false
}

fn is_string_type(ty: &Type) -> bool {
    if let Type::Path(type_path) = ty {
        if let Some(segment) = type_path.path.segments.last() {
            return segment.ident == "String";
        }
    }
    false
}

fn is_serde_json_value_type(ty: &Type) -> bool {
    let Type::Path(type_path) = ty else {
        return false;
    };

    let segments: Vec<String> = type_path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect();

    matches!(segments.as_slice(), [single] if single == "Value")
        || matches!(segments.as_slice(), [serde_json, value] if serde_json == "serde_json" && value == "Value")
}

fn is_f32_array_of_len(ty: &Type, expected_len: usize) -> bool {
    let Type::Array(array) = ty else {
        return false;
    };

    if !is_path_ident(&array.elem, "f32") {
        return false;
    }

    let Expr::Lit(expr_lit) = &array.len else {
        return false;
    };

    let syn::Lit::Int(len_lit) = &expr_lit.lit else {
        return false;
    };

    match len_lit.base10_parse::<usize>() {
        Ok(value) => value == expected_len,
        Err(_) => false,
    }
}

fn primitive_serialize_value_expr(
    method: &Ident,
    typed_expr: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let method_name = method.to_string();
    match method_name.as_str() {
        "serialize_f32" | "serialize_f64" | "serialize_i32" | "serialize_i64"
        | "serialize_u32" | "serialize_u64" | "serialize_bool" => {
            Ok(quote! { ::serde_json::json!(*#typed_expr) })
        }
        "serialize_string" => Ok(quote! { ::serde_json::json!(#typed_expr) }),
        "serialize_json_value" => Ok(quote! { #typed_expr.clone() }),
        "serialize_vec3" => {
            Ok(quote! { ::serde_json::json!([#typed_expr[0], #typed_expr[1], #typed_expr[2]]) })
        }
        "serialize_color" => {
            Ok(quote! { ::serde_json::json!([#typed_expr[0], #typed_expr[1], #typed_expr[2], #typed_expr[3]]) })
        }
        _ => Err(syn::Error::new_spanned(
            method,
            "unsupported serialize_method for primitive JSON registration",
        )),
    }
}

fn primitive_deserialize_value_expr(
    method: &Ident,
    value_expr: proc_macro2::TokenStream,
) -> syn::Result<proc_macro2::TokenStream> {
    let method_name = method.to_string();
    match method_name.as_str() {
        "deserialize_f32" => Ok(quote! {
            #value_expr.as_f64().map(|v| v as f32).ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "f32",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_f64" => Ok(quote! {
            #value_expr.as_f64().ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "f64",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_i32" => Ok(quote! {
            #value_expr.as_i64().map(|v| v as i32).ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "i32",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_i64" => Ok(quote! {
            #value_expr.as_i64().ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "i64",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_u32" => Ok(quote! {
            #value_expr.as_u64().map(|v| v as u32).ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "u32",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_u64" => Ok(quote! {
            #value_expr.as_u64().ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "u64",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_bool" => Ok(quote! {
            #value_expr.as_bool().ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "bool",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_string" => Ok(quote! {
            #value_expr.as_str().map(|v| v.to_string()).ok_or_else(|| {
                ::pulsar_reflection::ReflectError::TypeMismatch {
                    expected: "String",
                    found: format!("{:?}", #value_expr),
                }
            })?
        }),
        "deserialize_json_value" => Ok(quote! { #value_expr }),
        "deserialize_vec3" => Ok(quote! {
            {
                let arr = #value_expr.as_array().ok_or_else(|| {
                    ::pulsar_reflection::ReflectError::TypeMismatch {
                        expected: "[f32; 3]",
                        found: format!("{:?}", #value_expr),
                    }
                })?;
                if arr.len() != 3 {
                    return Err(::pulsar_reflection::ReflectError::TypeMismatch {
                        expected: "[f32; 3]",
                        found: format!("array of length {}", arr.len()),
                    });
                }
                [
                    arr[0].as_f64().unwrap_or(0.0) as f32,
                    arr[1].as_f64().unwrap_or(0.0) as f32,
                    arr[2].as_f64().unwrap_or(0.0) as f32,
                ]
            }
        }),
        "deserialize_color" => Ok(quote! {
            {
                let arr = #value_expr.as_array().ok_or_else(|| {
                    ::pulsar_reflection::ReflectError::TypeMismatch {
                        expected: "[f32; 4]",
                        found: format!("{:?}", #value_expr),
                    }
                })?;
                if arr.len() != 4 {
                    return Err(::pulsar_reflection::ReflectError::TypeMismatch {
                        expected: "[f32; 4]",
                        found: format!("array of length {}", arr.len()),
                    });
                }
                [
                    arr[0].as_f64().unwrap_or(0.0) as f32,
                    arr[1].as_f64().unwrap_or(0.0) as f32,
                    arr[2].as_f64().unwrap_or(0.0) as f32,
                    arr[3].as_f64().unwrap_or(0.0) as f32,
                ]
            }
        }),
        _ => Err(syn::Error::new_spanned(
            method,
            "unsupported deserialize_method for primitive JSON registration",
        )),
    }
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
