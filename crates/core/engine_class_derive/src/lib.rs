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
    Attribute, Data, DeriveInput, Expr, Field, Fields, FnArg, ImplItem, ItemImpl, ItemStruct, Lit,
    Meta, MetaNameValue, Pat, PatType, ReturnType, Type,
    parse::{Parse, ParseStream},
    parse_macro_input,
    punctuated::Punctuated,
};

#[proc_macro_derive(
    EngineClass,
    attributes(
        property,
        category,
        engine_class_category,
        sub_props,
        engine_class_no_register
    )
)]
pub fn derive_engine_class(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Extract class category (menu grouping) and property category declarations.
    let class_category = extract_class_category(&input.attrs);
    let property_categories = match extract_property_categories(&input.attrs) {
        Ok(v) => v,
        Err(err) => return err.to_compile_error().into(),
    };

    // Convert category to TokenStream for registration
    let category_token = if let Some(cat) = &class_category {
        quote! { Some(#cat) }
    } else {
        quote! { None }
    };

    // Extract direct #[property] fields and optional #[sub_props] flattening fields.
    let (property_impls, property_fields, sub_props_fields): (Vec<_>, Vec<_>, Vec<_>) = match &input
        .data
    {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields) => {
                let mut props = Vec::new();
                let mut sub_props = Vec::new();
                for field in &fields.named {
                    let has_sub_props = has_sub_props_attr(field);
                    let property_attr = parse_property_attr(field);

                    if property_attr.is_property && has_sub_props {
                        return syn::Error::new_spanned(
                            field,
                            "field cannot use both #[property] and #[sub_props]",
                        )
                        .to_compile_error()
                        .into();
                    }

                    if property_attr.is_property {
                        let category_decl = if let Some(cat) = property_attr.category.as_ref() {
                            let Some(decl) = property_categories.iter().find(|d| d.name == *cat)
                            else {
                                return syn::Error::new_spanned(
                                    field,
                                    format!(
                                        "property category '{}' is not declared; add #[category(\"{}\", ...)] on the struct",
                                        cat, cat
                                    ),
                                )
                                .to_compile_error()
                                .into();
                            };
                            Some(decl)
                        } else {
                            None
                        };

                        props.push((
                            generate_property_metadata(field, name, &property_attr, category_decl),
                            field,
                        ));
                    }

                    if has_sub_props {
                        sub_props.push(field);
                    }
                }
                let (impls, fields): (Vec<_>, Vec<_>) = props.into_iter().unzip();
                (impls, fields, sub_props)
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
    let category_order_arms: Vec<_> = property_categories
        .iter()
        .map(|decl| {
            let cat_name = &decl.name;
            let order = decl.order;
            quote! { Some(#cat_name) => Some(#order), }
        })
        .collect();
    let sub_props_extenders: Vec<_> = sub_props_fields
        .iter()
        .map(|field| {
            let field_name = field.ident.as_ref().unwrap();
            quote! {
                for nested_prop in self.#field_name.get_properties() {
                    let pulsar_reflection::PropertyMetadata {
                        name: nested_name,
                        display_name,
                        category,
                        category_color,
                        category_default_collapsed,
                        category_order,
                        type_info,
                        getter: nested_getter,
                        setter: nested_setter,
                    } = nested_prop;

                    let remapped_category_order = match category {
                        #(#category_order_arms)*
                        _ => category_order,
                    };

                    let getter = Box::new(move |obj: &dyn pulsar_reflection::EngineClass| -> Box<dyn std::any::Any> {
                        let concrete = obj.as_any().downcast_ref::<#name>().unwrap();
                        nested_getter(&concrete.#field_name as &dyn pulsar_reflection::EngineClass)
                    });

                    let setter = Box::new(move |obj: &mut dyn pulsar_reflection::EngineClass, value: Box<dyn std::any::Any>| {
                        let concrete = obj.as_any_mut().downcast_mut::<#name>().unwrap();
                        nested_setter(&mut concrete.#field_name as &mut dyn pulsar_reflection::EngineClass, value);
                    });

                    props.push(pulsar_reflection::PropertyMetadata {
                        name: nested_name,
                        display_name,
                        category,
                        category_color,
                        category_default_collapsed,
                        category_order: remapped_category_order,
                        type_info,
                        getter,
                        setter,
                    });
                }
            }
        })
        .collect();

    // Compile-time assertions that every #[sub_props] field implements EngineSubProps.
    let sub_props_assertions: Vec<_> = sub_props_fields
        .iter()
        .map(|field| {
            let field_ty = &field.ty;
            quote! {
                const _: fn() = || {
                    fn _assert_engine_sub_props<T: pulsar_reflection::EngineSubProps>() {}
                    _assert_engine_sub_props::<#field_ty>();
                };
            }
        })
        .collect();

    let skip_registration = input
        .attrs
        .iter()
        .any(|a| a.path().is_ident("engine_class_no_register"));

    // Generate the trait implementation
    let generated = quote! {
        impl #impl_generics pulsar_reflection::EngineClass for #name #ty_generics #where_clause {
            fn class_name() -> &'static str {
                stringify!(#name)
            }

            fn get_properties(&self) -> Vec<pulsar_reflection::PropertyMetadata> {
                let mut props = vec![
                    #(#property_impls),*
                ];
                #(#sub_props_extenders)*
                props
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

    };

    let registration = if skip_registration {
        quote! {}
    } else {
        quote! {
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
        }
    };

    quote! {
        #generated
        #registration
        #(#sub_props_assertions)*
    }
    .into()
}

#[proc_macro_attribute]
pub fn engine_class(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, syn::Token![,]>::parse_terminated);
    let item_struct = parse_macro_input!(item as ItemStruct);

    let mut category: Option<String> = None;
    let mut add_serialize = false;
    let mut add_deserialize = false;
    let mut add_default = false;
    let mut add_clone = false;
    let mut add_debug = false;
    let mut register_runtime = false;
    let mut register_scene_props = false;
    let mut add_scene_store = false;
    let mut no_register = false;

    for arg in args {
        match arg {
            Meta::Path(path) if path.is_ident("serialize") => add_serialize = true,
            Meta::Path(path) if path.is_ident("deserialize") => add_deserialize = true,
            Meta::Path(path) if path.is_ident("default") => add_default = true,
            Meta::Path(path) if path.is_ident("clone") => add_clone = true,
            Meta::Path(path) if path.is_ident("debug") => add_debug = true,
            Meta::Path(path) if path.is_ident("runtime_behavior") => register_runtime = true,
            Meta::Path(path) if path.is_ident("no_register") => no_register = true,
            Meta::Path(path) if path.is_ident("scene_props_applier") => register_scene_props = true,
            Meta::Path(path) if path.is_ident("scene_store") => add_scene_store = true,
            Meta::NameValue(name_value) if name_value.path.is_ident("category") => {
                if let Expr::Lit(expr_lit) = &name_value.value {
                    if let Lit::Str(lit_str) = &expr_lit.lit {
                        category = Some(lit_str.value());
                        continue;
                    }
                }
                return syn::Error::new_spanned(
                    &name_value,
                    "engine_class category must be a string literal",
                )
                .to_compile_error()
                .into();
            }
            other => {
                return syn::Error::new_spanned(other, "unsupported #[engine_class(...)] argument")
                    .to_compile_error()
                    .into();
            }
        }
    }

    let has_engine_class_derive = has_derive(&item_struct.attrs, "EngineClass");
    let has_serialize_derive = has_derive(&item_struct.attrs, "Serialize");
    let has_deserialize_derive = has_derive(&item_struct.attrs, "Deserialize");
    let has_default_derive = has_derive(&item_struct.attrs, "Default");
    let has_clone_derive = has_derive(&item_struct.attrs, "Clone");
    let has_debug_derive = has_derive(&item_struct.attrs, "Debug");
    let has_engine_class_category_attr = item_struct
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("engine_class_category"));

    let mut derive_additions = Vec::new();
    if !has_engine_class_derive {
        derive_additions.push(quote!(::engine_class_derive::EngineClass));
    }
    if add_serialize && !has_serialize_derive {
        derive_additions.push(quote!(::serde::Serialize));
    }
    if add_deserialize && !has_deserialize_derive {
        derive_additions.push(quote!(::serde::Deserialize));
    }
    if add_default && !has_default_derive {
        derive_additions.push(quote!(::core::default::Default));
    }
    if add_clone && !has_clone_derive {
        derive_additions.push(quote!(::core::clone::Clone));
    }
    if add_debug && !has_debug_derive {
        derive_additions.push(quote!(::core::fmt::Debug));
    }

    let derive_attr = if derive_additions.is_empty() {
        quote! {}
    } else {
        quote! { #[derive(#(#derive_additions),*)] }
    };

    let category_attr = if category.is_some() && !has_engine_class_category_attr {
        let cat = category.unwrap();
        quote! { #[engine_class_category(#cat)] }
    } else {
        quote! {}
    };

    let no_register_attr = if no_register {
        quote! { #[engine_class_no_register] }
    } else {
        quote! {}
    };

    let sub_props_marker_impl = if no_register {
        let name = &item_struct.ident;
        quote! { impl pulsar_reflection::EngineSubProps for #name {} }
    } else {
        quote! {}
    };

    let name = &item_struct.ident;
    let runtime_registration = if register_runtime {
        quote! {
            pulsar_reflection::inventory::submit! {
                pulsar_reflection::RuntimeBehaviorRegistration {
                    class_name: <#name as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                    sync: <#name as pulsar_reflection::ComponentRuntimeBehavior>::sync_component,
                }
            }
        }
    } else {
        quote! {}
    };

    let scene_props_registration = if register_scene_props {
        quote! {
            pulsar_reflection::inventory::submit! {
                pulsar_reflection::ScenePropsApplierRegistration {
                    class_name: <#name as pulsar_reflection::ScenePropsProjector>::CLASS_NAME,
                    apply: <#name as pulsar_reflection::ScenePropsProjector>::apply_scene_props,
                }
            }
        }
    } else {
        quote! {}
    };

    // ── SceneStore impl generation (auto-derived for every engine_class) ──

    // Collect field info for Pod + SceneColumnSet + GpuColumnSet generation.
    struct NamedField {
        ident: syn::Ident,
        ty: syn::Type,
        is_gpu: bool,
    }

    let named_fields: Vec<NamedField> = match &item_struct.fields {
        Fields::Named(named) => named
            .named
            .iter()
            .map(|f| {
                let ident = f.ident.clone().unwrap();
                let ty = f.ty.clone();
                let is_gpu = f.attrs.iter().any(|a| a.path().is_ident("gpu"));
                NamedField { ident, ty, is_gpu }
            })
            .collect(),
        _ => Vec::new(),
    };

    let scenedb_impls = if !add_scene_store || named_fields.is_empty() {
        quote! {}
    } else {
        // ── Pod ──
        let pod_bounds: Vec<_> = named_fields
            .iter()
            .map(|f| { let ty = &f.ty; quote! { #ty: ::pulsar_scenedb::page::Pod } })
            .collect();
        let pod_impl = if pod_bounds.is_empty() {
            quote! { unsafe impl ::pulsar_scenedb::page::Pod for #name {} }
        } else {
            quote! { unsafe impl ::pulsar_scenedb::page::Pod for #name where #(#pod_bounds),* {} }
        };

        // ── HasTypeToken ──
        let has_type_token = quote! {
            impl ::pulsar_scenedb::token::HasTypeToken for #name {
                fn type_token() -> ::pulsar_scenedb::token::TypeToken {
                    ::pulsar_scenedb::token::TypeToken::of::<Self>()
                }
            }
        };

        // ── SceneColumnSet ──
        let cell_entries: Vec<_> = named_fields
            .iter()
            .map(|f| { let ty = &f.ty; quote! { .with(::pulsar_scenedb::token::TypeToken::of::<#ty>()) } })
            .collect();
        let name_str = name.to_string();
        let scene_column_set = quote! {
            impl ::pulsar_scenedb::cell_type::SceneColumnSet for #name {
                fn cell_type() -> ::pulsar_scenedb::cell_type::RegisteredCellType {
                    ::pulsar_scenedb::cell_type::CellType::new(#name_str)
                        #(#cell_entries)*
                        .build()
                        .expect("SceneColumnSet cell_type: CellType::build failed")
                }
            }
        };

        // ── GpuColumnSet (gated behind the `gpu` feature) ──
        let gpu_fields: Vec<&NamedField> = named_fields.iter().filter(|f| f.is_gpu).collect();
        let gpu_column_set = if gpu_fields.is_empty() {
            quote! {
                #[cfg(feature = "gpu")]
                impl ::pulsar_scenedb::gpu::scene_store::GpuColumnSet for #name {
                    fn gpu_columns() -> Vec<::pulsar_scenedb::gpu::scene_store::GpuColumnDesc> {
                        Vec::new()
                    }
                    fn write_gpu(
                        _store: &::pulsar_scenedb::gpu::scene_store::SceneGpuStore,
                        _id: ::pulsar_scenedb::gpu::scene_store::CellId,
                        _cell: &mut ::pulsar_scenedb::cell::CellStorage,
                        _handle: ::pulsar_scenedb::handle::Handle,
                        _data: &Self,
                        _phase: &impl ::pulsar_scenedb::gpu::phase::SimulateWitness,
                    ) {}
                }
            }
        } else {
            let descs: Vec<_> = gpu_fields
                .iter()
                .map(|f| {
                    let field_ident = &f.ident;
                    let field_name = field_ident.to_string();
                    let field_ty = &f.ty;
                    quote! {
                        ::pulsar_scenedb::gpu::scene_store::GpuColumnDesc {
                            field_token: ::pulsar_scenedb::token::TypeToken::of::<#field_ty>(),
                            field_offset: ::std::mem::offset_of!(#name, #field_ident),
                            mode: ::pulsar_scenedb::gpu::scene_store::MirrorMode::DirtyTracked,
                            buffer_name: #field_name,
                        }
                    }
                })
                .collect();
            let arms: Vec<_> = gpu_fields
                .iter()
                .map(|f| {
                    let field_ident = &f.ident;
                    let field_name = field_ident.to_string();
                    let field_ty = &f.ty;
                    quote! {
                        #field_name => {
                            let row = cell.row_of(handle).unwrap_or_else(|| {
                                panic!("write_gpu: handle {:?} not found in cell", handle);
                            }) as usize;
                            if let Some(col) = cell.column_for_mut::<#field_ty>() {
                                col[row] = data.#field_ident;
                            }
                            let comp_id = ::pulsar_scenedb::component::component_id::<#field_ty>();
                            store.mark_column_dirty(id, comp_id, row as u32);
                        }
                    }
                })
                .collect();
            quote! {
                #[cfg(feature = "gpu")]
                impl ::pulsar_scenedb::gpu::scene_store::GpuColumnSet for #name {
                    fn gpu_columns() -> Vec<::pulsar_scenedb::gpu::scene_store::GpuColumnDesc> {
                        vec![ #(#descs),* ]
                    }
                    fn write_gpu(
                        store: &::pulsar_scenedb::gpu::scene_store::SceneGpuStore,
                        id: ::pulsar_scenedb::gpu::scene_store::CellId,
                        cell: &mut ::pulsar_scenedb::cell::CellStorage,
                        handle: ::pulsar_scenedb::handle::Handle,
                        data: &Self,
                        _phase: &impl ::pulsar_scenedb::gpu::phase::SimulateWitness,
                    ) {
                        let descs = Self::gpu_columns();
                        for desc in &descs {
                            match desc.buffer_name {
                                #(#arms)*
                                _ => {}
                            }
                        }
                    }
                }
            }
        };

        quote! {
            #pod_impl
            #has_type_token
            #scene_column_set
            #gpu_column_set
        }
    };

    quote! {
        #derive_attr
        #category_attr
        #no_register_attr
        #item_struct
        #sub_props_marker_impl
        #scenedb_impls
        #runtime_registration
        #scene_props_registration
    }
    .into()
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

#[proc_macro_attribute]
pub fn register_runtime_behavior(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "#[register_runtime_behavior] does not accept arguments",
        )
        .to_compile_error()
        .into();
    }

    let impl_block = parse_macro_input!(item as ItemImpl);

    if !impl_block.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &impl_block.generics,
            "#[register_runtime_behavior] does not support generic impl blocks",
        )
        .to_compile_error()
        .into();
    }

    let Some((_, trait_path, _)) = &impl_block.trait_ else {
        return syn::Error::new_spanned(
            &impl_block.self_ty,
            "#[register_runtime_behavior] must be used on `impl ComponentRuntimeBehavior for Type`",
        )
        .to_compile_error()
        .into();
    };

    let Some(trait_ident) = trait_path.segments.last().map(|s| &s.ident) else {
        return syn::Error::new_spanned(
            trait_path,
            "invalid trait path for #[register_runtime_behavior]",
        )
        .to_compile_error()
        .into();
    };

    if trait_ident != "ComponentRuntimeBehavior" {
        return syn::Error::new_spanned(
            trait_path,
            "#[register_runtime_behavior] must target `ComponentRuntimeBehavior` impl",
        )
        .to_compile_error()
        .into();
    }

    let self_ty = &impl_block.self_ty;
    let output = quote! {
        #impl_block

        pulsar_reflection::inventory::submit! {
            pulsar_reflection::RuntimeBehaviorRegistration {
                class_name: <#self_ty as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                sync: <#self_ty as pulsar_reflection::ComponentRuntimeBehavior>::sync_component,
            }
        }
    };

    output.into()
}

#[proc_macro_attribute]
pub fn register_scene_props_applier(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "#[register_scene_props_applier] does not accept arguments",
        )
        .to_compile_error()
        .into();
    }

    let impl_block = parse_macro_input!(item as ItemImpl);

    if !impl_block.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &impl_block.generics,
            "#[register_scene_props_applier] does not support generic impl blocks",
        )
        .to_compile_error()
        .into();
    }

    let Some((_, trait_path, _)) = &impl_block.trait_ else {
        return syn::Error::new_spanned(
            &impl_block.self_ty,
            "#[register_scene_props_applier] must be used on `impl ScenePropsProjector for Type`",
        )
        .to_compile_error()
        .into();
    };

    let Some(trait_ident) = trait_path.segments.last().map(|s| &s.ident) else {
        return syn::Error::new_spanned(
            trait_path,
            "invalid trait path for #[register_scene_props_applier]",
        )
        .to_compile_error()
        .into();
    };

    if trait_ident != "ScenePropsProjector" {
        return syn::Error::new_spanned(
            trait_path,
            "#[register_scene_props_applier] must target `ScenePropsProjector` impl",
        )
        .to_compile_error()
        .into();
    }

    let self_ty = &impl_block.self_ty;
    let output = quote! {
        #impl_block

        pulsar_reflection::inventory::submit! {
            pulsar_reflection::ScenePropsApplierRegistration {
                class_name: <#self_ty as pulsar_reflection::ScenePropsProjector>::CLASS_NAME,
                apply: <#self_ty as pulsar_reflection::ScenePropsProjector>::apply_scene_props,
            }
        }
    };

    output.into()
}

/// Check whether a type already derives a specific trait by final segment ident.
fn has_derive(attrs: &[Attribute], trait_ident: &str) -> bool {
    attrs.iter().any(|attr| {
        if !attr.path().is_ident("derive") {
            return false;
        }

        attr.parse_args_with(Punctuated::<syn::Path, syn::Token![,]>::parse_terminated)
            .map(|paths| {
                paths.iter().any(|p| {
                    p.segments
                        .last()
                        .map(|s| s.ident == trait_ident)
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    })
}

fn has_sub_props_attr(field: &Field) -> bool {
    field
        .attrs
        .iter()
        .any(|attr| attr.path().is_ident("sub_props"))
}

#[derive(Default)]
struct PropertyAttrOptions {
    is_property: bool,
    category: Option<String>,
    category_color: Option<String>,
}

#[derive(Clone, Debug)]
struct PropertyCategoryDefinition {
    name: String,
    category_color: Option<String>,
    default_collapsed: bool,
    order: usize,
}

struct CategoryAttrArgs {
    name: syn::LitStr,
    options: Punctuated<MetaNameValue, syn::Token![,]>,
}

impl Parse for CategoryAttrArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let name: syn::LitStr = input.parse()?;
        let mut options = Punctuated::new();
        if input.is_empty() {
            return Ok(Self { name, options });
        }

        let _comma: syn::Token![,] = input.parse()?;
        while !input.is_empty() {
            options.push_value(input.parse::<MetaNameValue>()?);
            if input.is_empty() {
                break;
            }
            let punct: syn::Token![,] = input.parse()?;
            options.push_punct(punct);
        }

        Ok(Self { name, options })
    }
}

/// Parse `#[property(...)]` options.
fn parse_property_attr(field: &Field) -> PropertyAttrOptions {
    let mut out = PropertyAttrOptions::default();

    for attr in &field.attrs {
        if !attr.path().is_ident("property") {
            continue;
        }
        out.is_property = true;

        let Ok(args) = attr.parse_args_with(Punctuated::<Meta, syn::Token![,]>::parse_terminated)
        else {
            continue;
        };
        for arg in args {
            if let Meta::NameValue(name_value) = arg {
                if name_value.path.is_ident("category")
                    && let Expr::Lit(expr_lit) = &name_value.value
                    && let Lit::Str(lit_str) = &expr_lit.lit
                {
                    out.category = Some(lit_str.value());
                }
                if name_value.path.is_ident("category_color")
                    && let Expr::Lit(expr_lit) = &name_value.value
                    && let Lit::Str(lit_str) = &expr_lit.lit
                {
                    out.category_color = Some(lit_str.value());
                }
            }
        }
    }

    out
}

/// Extract engine-class category (registry grouping) from struct-level attributes.
fn extract_class_category(attrs: &[Attribute]) -> Option<String> {
    for attr in attrs {
        if attr.path().is_ident("engine_class_category")
            && let Ok(lit_str) = attr.parse_args::<syn::LitStr>()
        {
            return Some(lit_str.value());
        }
    }

    // Backwards-compatible fallback for legacy `#[category("Physics")]` style.
    // This only matches the single-string form (category declarations with extra
    // options are intentionally excluded from class-category extraction).
    for attr in attrs {
        if attr.path().is_ident("category") {
            if let Ok(lit_str) = attr.parse_args::<syn::LitStr>() {
                return Some(lit_str.value());
            }
        }
    }
    None
}

/// Extract `#[category("Name", ...)]` declarations used by property grouping.
fn extract_property_categories(
    attrs: &[Attribute],
) -> syn::Result<Vec<PropertyCategoryDefinition>> {
    let mut out = Vec::new();

    for attr in attrs {
        if !attr.path().is_ident("category") {
            continue;
        }

        let parsed: CategoryAttrArgs = attr.parse_args()?;
        let mut category_color: Option<String> = None;
        let mut default_collapsed = false;

        for nv in parsed.options {
            if nv.path.is_ident("category_color") {
                if let Expr::Lit(expr_lit) = &nv.value
                    && let Lit::Str(lit) = &expr_lit.lit
                {
                    category_color = Some(lit.value());
                    continue;
                }
                return Err(syn::Error::new_spanned(
                    nv,
                    "category_color must be a string literal",
                ));
            }
            if nv.path.is_ident("default_collapsed") {
                if let Expr::Lit(expr_lit) = &nv.value
                    && let Lit::Bool(lit) = &expr_lit.lit
                {
                    default_collapsed = lit.value();
                    continue;
                }
                return Err(syn::Error::new_spanned(
                    nv,
                    "default_collapsed must be a bool literal",
                ));
            }
            return Err(syn::Error::new_spanned(
                nv,
                "unsupported #[category(...)] option",
            ));
        }

        if out
            .iter()
            .any(|existing: &PropertyCategoryDefinition| existing.name == parsed.name.value())
        {
            return Err(syn::Error::new_spanned(
                attr,
                format!(
                    "duplicate #[category(\"{}\")] declaration",
                    parsed.name.value()
                ),
            ));
        }

        out.push(PropertyCategoryDefinition {
            name: parsed.name.value(),
            category_color,
            default_collapsed,
            order: out.len(),
        });
    }

    Ok(out)
}

/// Generate PropertyMetadata for a single field
///
/// NOW USES RUNTIME TYPE REFLECTION - NO MORE ENUM INFERENCE!
fn generate_property_metadata(
    field: &Field,
    struct_name: &syn::Ident,
    property_attr: &PropertyAttrOptions,
    category_decl: Option<&PropertyCategoryDefinition>,
) -> proc_macro2::TokenStream {
    let field_name = field.ident.as_ref().unwrap();
    let field_name_str = field_name.to_string();
    let display_name = capitalize_first(&field_name_str);
    let field_type = &field.ty;

    // Generate category option
    let resolved_category = property_attr.category.clone();
    let category_expr = if let Some(cat) = resolved_category {
        quote! { Some(#cat) }
    } else {
        quote! { None }
    };
    let resolved_category_color = property_attr
        .category_color
        .clone()
        .or_else(|| category_decl.and_then(|decl| decl.category_color.clone()));
    let category_color_expr = if let Some(color) = resolved_category_color {
        quote! { Some(#color) }
    } else {
        quote! { None }
    };
    let category_default_collapsed_expr = if category_decl
        .map(|decl| decl.default_collapsed)
        .unwrap_or(false)
    {
        quote! { true }
    } else {
        quote! { false }
    };
    let category_order_expr = if let Some(order) = category_decl.map(|decl| decl.order) {
        quote! { Some(#order) }
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
            category_color: #category_color_expr,
            category_default_collapsed: #category_default_collapsed_expr,
            category_order: #category_order_expr,
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
            let method_attr = method
                .attrs
                .iter()
                .find(|attr| attr.path().is_ident("method"));

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
                            let param_type = ty.clone();
                            params.push((param_name, param_type));
                        }
                    }
                }

                // Extract return type
                let return_type = match &method.sig.output {
                    ReturnType::Default => None,
                    ReturnType::Type(_, ty) => Some(ty.clone()),
                };

                // Generate param metadata
                let param_metadata: Vec<_> = params
                    .iter()
                    .map(|(name, ty)| {
                        quote! {
                            pulsar_reflection::MethodParameter {
                                name: #name,
                                type_info: <#ty as pulsar_reflection::Reflectable>::type_info(),
                            }
                        }
                    })
                    .collect();

                // Generate return type metadata
                let return_metadata = if let Some(ret_ty) = &return_type {
                    quote! {
                        Some(pulsar_reflection::MethodReturnType {
                            type_info: <#ret_ty as pulsar_reflection::Reflectable>::type_info(),
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
                        quote! {
                            {
                                let boxed = __pulsar_args
                                    .next()
                                    .expect(concat!("Missing argument at index ", stringify!(#i)));
                                match boxed.downcast::<#ty>() {
                                    Ok(value) => *value,
                                    Err(_) => panic!(concat!("Invalid argument type at index ", stringify!(#i))),
                                }
                            }
                        }
                    })
                    .collect();

                let caller = if is_mut {
                    let result_conversion = if return_type.is_some() {
                        quote! { Some(Box::new(result) as Box<dyn std::any::Any>) }
                    } else {
                        quote! { None }
                    };

                    quote! {
                        Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, args: pulsar_reflection::MethodArgs| {
                            let concrete = obj.as_any_mut().downcast_mut::<#type_name>().expect("Downcast failed");
                            let mut __pulsar_args = args.into_iter();
                            let result = concrete.#method_ident(#(#param_reads),*);
                            #result_conversion
                        })
                    }
                } else {
                    let result_conversion = if return_type.is_some() {
                        quote! { Some(Box::new(result) as Box<dyn std::any::Any>) }
                    } else {
                        quote! { None }
                    };

                    quote! {
                        Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, args: pulsar_reflection::MethodArgs| {
                            let concrete = obj.as_any().downcast_ref::<#type_name>().expect("Downcast failed");
                            let mut __pulsar_args = args.into_iter();
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
            } else if tokens_str.contains("MethodType :: ControlFlow")
                || tokens_str.contains("ControlFlow")
            {
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

/// Generate getter and setter method metadata items for properties
fn generate_property_method_items(
    fields: &[&Field],
    struct_name: &syn::Ident,
) -> Vec<proc_macro2::TokenStream> {
    let mut method_items = Vec::new();

    for field in fields {
        let field_name = field.ident.as_ref().unwrap();
        let field_name_str = field_name.to_string();
        let getter_name = format!("get_{}", field_name_str);
        let setter_name = format!("set_{}", field_name_str);
        let getter_display = capitalize_first(&format!("Get {}", field_name_str));
        let setter_display = capitalize_first(&format!("Set {}", field_name_str));
        let field_type = &field.ty;

        method_items.push(quote! {
            pulsar_reflection::MethodMetadata {
                name: #getter_name,
                display_name: #getter_display.to_string(),
                category: None,
                params: vec![],
                return_type: Some(pulsar_reflection::MethodReturnType {
                    type_info: <#field_type as pulsar_reflection::Reflectable>::type_info(),
                }),
                method_type: pulsar_reflection::MethodType::Pure,
                caller: Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, _args: pulsar_reflection::MethodArgs| {
                    let concrete = obj.as_any().downcast_ref::<#struct_name>().unwrap();
                    Some(Box::new(concrete.#field_name.clone()) as Box<dyn std::any::Any>)
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
                        type_info: <#field_type as pulsar_reflection::Reflectable>::type_info(),
                    }
                ],
                return_type: None,
                method_type: pulsar_reflection::MethodType::Fn,
                caller: Box::new(|obj: &mut dyn pulsar_reflection::EngineClass, args: pulsar_reflection::MethodArgs| {
                    let concrete = obj.as_any_mut().downcast_mut::<#struct_name>().unwrap();
                    if let Some(value) = args.into_iter().next() {
                        match value.downcast::<#field_type>() {
                            Ok(typed_value) => {
                                concrete.#field_name = *typed_value;
                            }
                            Err(invalid_value) => {
                                tracing::warn!(
                                    "Type mismatch in generated setter {}.{}",
                                    stringify!(#struct_name),
                                    #field_name_str,
                                );
                                let _ = invalid_value;
                            }
                        }
                    }
                    None
                }),
            }
        });
    }

    method_items
}
