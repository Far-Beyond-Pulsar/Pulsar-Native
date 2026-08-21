//! Proc macros for `#[engine_class(...)]`/`#[derive(EngineClass)]` --
//! reflection (the properties panel), `World`/SceneDB storage wiring, and
//! GPU mirroring, all generated off ONE struct definition's own
//! `#[property]`/`#[sub_props]`/`#[gpu]` field attributes.
//!
//! # What this crate generates, by attribute
//!
//! - `#[property]` (any field): reflection metadata (`EngineClass::
//!   get_properties`) -- name, category, min/max, a typed getter/setter --
//!   for the properties panel to render an editor for. See the example
//!   below.
//! - `#[sub_props]` (a field whose type is itself `#[engine_class(...,
//!   no_register)]`): flattens that nested type's own properties into the
//!   containing struct's property list, AND (independently) composes its
//!   GPU mirror into the containing struct's, if either has one -- see the
//!   `#[gpu]` bullet.
//! - `#[gpu]` (a `#[property]` field, or a `Vec<T>` field): opts the field
//!   into an auto-generated, `#[derive(pulsar_scenedb::SceneStore)]`-backed
//!   companion component -- `pulsar_world_registry::GpuMirrored` for a
//!   fixed-size/packed field (numeric primitives and arrays as-is, `bool`/
//!   a plain enum cast to `u32`), `GpuListMirrored` for a `Vec<T>` one (a
//!   SEPARATE companion, deliberately -- see that trait's own doc). Every
//!   `#[engine_class(...)]`-processed struct gets both impls
//!   unconditionally, `NoGpuMirror` when there's nothing to mirror, so
//!   `#[sub_props]` composition never needs special-casing. See
//!   `gpu_mirror_codegen`'s doc for the full packing rules.
//! - `scene_store` (a struct-level `#[engine_class(...)]` flag, not a field
//!   attribute): a DIFFERENT, older mechanism -- routes the WHOLE struct
//!   through `#[derive(SceneStore)]` directly instead of a companion. The
//!   right choice when the struct's `Vec<T>` `#[gpu]` field payload itself
//!   IS the component (`StaticMeshComponent::vertices`/`indices`), not a
//!   translation of some other editor-facing shape.
//! - `#[register_world_component(...)]`: wires a `ComponentRuntimeBehavior`
//!   impl into `pulsar_world_registry`'s `World`-storage bridge --
//!   `hydrate`/`remove`/`on_removed`/`dispatch`, plus (via the `gpu_mirror`
//!   bare flag) auto-syncing the `#[gpu]`-derived companions above. See
//!   that macro's own doc for the full option list.
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
        engine_class_no_register,
        engine_class_serialize,
        engine_class_deserialize,
        gpu
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
    let (property_impls, property_fields, sub_props_fields, gpu_leaf_fields, gpu_list_leaf_fields, gpu_heavy_leaf_fields): (
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
        Vec<_>,
    ) = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields) => {
                let mut props = Vec::new();
                let mut sub_props = Vec::new();
                let mut gpu_fields = Vec::new();
                let mut gpu_list_fields = Vec::new();
                let mut gpu_heavy_fields = Vec::new();
                for field in &fields.named {
                    let has_sub_props = has_sub_props_attr(field);
                    let property_attr = parse_property_attr(field);
                    let is_gpu = has_gpu_attr(field);

                    if property_attr.is_property && has_sub_props {
                        return syn::Error::new_spanned(
                            field,
                            "field cannot use both #[property] and #[sub_props]",
                        )
                        .to_compile_error()
                        .into();
                    }
                    if is_gpu && has_sub_props {
                        return syn::Error::new_spanned(
                            field,
                            "#[gpu] on a #[sub_props] field has no effect -- every #[sub_props] \
                             field is composed into the generated GPU mirror automatically \
                             (see that sub-props type's own #[gpu]-marked fields); remove #[gpu] here",
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

                    // A `#[gpu] Vec<T>` field either belongs to the SEPARATE
                    // var-len list mirror (below) or -- for a struct that
                    // ALSO uses `#[engine_class(..., scene_store)]` on
                    // itself (`StaticMeshComponent::vertices`/`indices`,
                    // Pulsar-Native#561 Phase D) -- is ALREADY handled
                    // directly by that struct's own `#[derive(SceneStore)]`.
                    // `derive_engine_class` has no visibility into whether
                    // `scene_store` was requested (a separate macro
                    // invocation's own parsed args), so it generates the
                    // list-mirror companion unconditionally regardless --
                    // harmlessly inert for a `scene_store` struct, since
                    // nothing calls `sync_gpu_list_mirror` for it unless
                    // `#[register_world_component(gpu_mirror)]` is ALSO
                    // used, which such a struct's own custom hydrate
                    // (`hydrate_static_mesh_component`) doesn't.
                    if is_gpu && is_gpu_heavy_type(&field.ty) {
                        let field_ident = field.ident.clone().unwrap();
                        let handle_ty = gpu_heavy_inner_type(&field.ty)
                            .expect("is_gpu_heavy_type confirmed this is GpuHeavy<T>, so T must parse");
                        gpu_heavy_fields.push(GpuHeavyLeafField { ident: field_ident, handle_ty });
                    } else if is_gpu && !is_vec_type(&field.ty) {
                        let field_ident = field.ident.clone().unwrap();
                        gpu_fields.push(GpuLeafField { ident: field_ident, field_ty: field.ty.clone() });
                    } else if is_gpu {
                        // is_vec_type(&field.ty) == true here.
                        let field_ident = field.ident.clone().unwrap();
                        let elem_ty = vec_elem_type(&field.ty)
                            .expect("is_vec_type confirmed this is Vec<T>, so T must parse");
                        gpu_list_fields.push(GpuListLeafField { ident: field_ident, elem_ty });
                    }

                    if has_sub_props {
                        sub_props.push(field);
                    }
                }
                let (impls, fields): (Vec<_>, Vec<_>) = props.into_iter().unzip();
                (impls, fields, sub_props, gpu_fields, gpu_list_fields, gpu_heavy_fields)
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

    let gpu_mirror_tokens = gpu_mirror_codegen(name, &gpu_leaf_fields, &sub_props_fields);
    let gpu_list_mirror_tokens = gpu_list_mirror_codegen(name, &gpu_list_leaf_fields);
    let gpu_heavy_mirror_tokens = gpu_heavy_mirror_codegen(name, &gpu_heavy_leaf_fields);

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

    // Whole-instance JSON round trip (Pulsar-Native#561's properties-panel
    // fix): `to_json`/`from_json` only make sense for classes that actually
    // derive `Serialize`/`Deserialize`. Can't detect that with `has_derive`
    // here the way `engine_class`'s attribute-macro pass does on
    // `item_struct.attrs` -- a `#[proc_macro_derive]` never sees the
    // `#[derive(...)]` list that triggered it (confirmed by instrumenting
    // this function: `input.attrs` for a real component contained only its
    // OTHER attributes -- `category`, `engine_class_category`, etc. --
    // never `derive` itself). So `#[engine_class(..., serialize,
    // deserialize, ...)]` instead stamps two marker attributes
    // (`engine_class_serialize`/`engine_class_deserialize`) onto the struct
    // alongside `category_attr`/`no_register_attr` below, and THOSE are
    // what this derive macro can actually see.
    let has_serialize = input
        .attrs
        .iter()
        .any(|a| a.path().is_ident("engine_class_serialize"));
    let has_deserialize = input
        .attrs
        .iter()
        .any(|a| a.path().is_ident("engine_class_deserialize"));

    let to_json_impl = if has_serialize {
        quote! {
            fn to_json(&self) -> Result<::serde_json::Value, String> {
                ::serde_json::to_value(self).map_err(|error| error.to_string())
            }
        }
    } else {
        // No override -- inherit `EngineClass::to_json`'s default `Err`.
        quote! {}
    };

    let from_json_shim = if has_deserialize {
        let shim_fn_name = quote::format_ident!("__pulsar_reflection_from_json_shim_{}", name);
        quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #shim_fn_name(
                data: &::serde_json::Value,
            ) -> Result<Box<dyn pulsar_reflection::EngineClass>, String> {
                let parsed: #name = ::serde_json::from_value(data.clone())
                    .map_err(|error| error.to_string())?;
                Ok(Box::new(parsed) as Box<dyn pulsar_reflection::EngineClass>)
            }
        }
    } else {
        quote! {}
    };
    let from_json_registration_value = if has_deserialize {
        let shim_fn_name = quote::format_ident!("__pulsar_reflection_from_json_shim_{}", name);
        quote! { Some(#shim_fn_name) }
    } else {
        quote! { None }
    };

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

            #to_json_impl
        }

        #from_json_shim
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
                    from_json: #from_json_registration_value,
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
        #gpu_mirror_tokens
        #gpu_list_mirror_tokens
        #gpu_heavy_mirror_tokens
    }
    .into()
}

/// Mirrors `pulsar_scenedb_derive`'s own syntactic `Vec<T>` field-type check
/// (independently duplicated, not shared -- it's a few lines of pure syntax
/// matching, not worth a cross-crate dependency for). Used only to decide
/// whether `scene_store`'s auto-added `Copy` derive (below) would be a hard
/// compile error: a struct with a `#[gpu]` field whose type is syntactically
/// `Vec<...>` gets routed through SceneDB's variable-length codegen path
/// instead of the classic `Pod`/`SceneColumnSet`/`GpuColumnSet` one -- that
/// path generates none of those impls, so `Copy` is neither required (no
/// `Pod: Copy` bound ever applies) nor even possible to satisfy (`Vec` is
/// never `Copy`).
fn struct_has_gpu_vec_field(fields: &syn::Fields) -> bool {
    fields.iter().any(|field| {
        let has_gpu_attr = field.attrs.iter().any(|attr| attr.path().is_ident("gpu"));
        has_gpu_attr && is_vec_type(&field.ty)
    })
}

fn is_vec_type(ty: &syn::Type) -> bool {
    let syn::Type::Path(type_path) = ty else { return false };
    type_path.path.segments.last().map(|s| s.ident == "Vec").unwrap_or(false)
}

/// Returns `Some(T)` if `ty` is syntactically `Vec<T>` -- same syntactic-
/// only check `is_vec_type` already makes, extended to also hand back the
/// element type for the auto-derived var-len GPU list mirror
/// (`gpu_list_mirror_codegen`).
fn vec_elem_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(type_path) = ty else { return None };
    let last = type_path.path.segments.last()?;
    if last.ident != "Vec" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else { return None };
    if args.args.len() != 1 {
        return None;
    }
    match args.args.first()? {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    }
}

/// Same syntactic-only check `is_vec_type` makes for `Vec<T>`, for
/// `pulsar_world_registry::GpuHeavy<T>` -- a proc macro has no way to ask
/// "does this field's type implement `GpuUploadSource`" (trait resolution
/// happens after macro expansion, not during it), so `GpuHeavy<T>` being a
/// distinct, purpose-built wrapper the field's own type signature names is
/// what makes this field shape detectable at all -- see that type's own
/// doc for the full rationale.
fn is_gpu_heavy_type(ty: &syn::Type) -> bool {
    let syn::Type::Path(type_path) = ty else { return false };
    type_path.path.segments.last().map(|s| s.ident == "GpuHeavy").unwrap_or(false)
}

/// Returns `Some(T)` if `ty` is syntactically `GpuHeavy<T>` -- the handle
/// type to splice, unwrapped, into the generated heavy/handle mirror
/// (`gpu_heavy_mirror_codegen`).
fn gpu_heavy_inner_type(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(type_path) = ty else { return None };
    let last = type_path.path.segments.last()?;
    if last.ident != "GpuHeavy" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else { return None };
    if args.args.len() != 1 {
        return None;
    }
    match args.args.first()? {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    }
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
    let has_scene_store_derive = has_derive(&item_struct.attrs, "SceneStore");
    let has_copy_derive = has_derive(&item_struct.attrs, "Copy");
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
    // Delegates to `pulsar_scenedb_derive`'s own derive (re-exported as
    // `pulsar_scenedb::SceneStore`) instead of hand-rolling equivalent
    // Pod/HasTypeToken/SceneColumnSet/GpuColumnSet codegen here -- see this
    // block's doc below for why. Requires the consuming crate to depend on
    // `pulsar_scenedb` (with the `gpu` feature on, for the GPU-column half);
    // `#[gpu(...)]` per-field attributes on the struct are consumed by
    // SceneStore's own attribute parsing (`attributes(gpu)`), not by this
    // macro -- they were already valid syntax here (the old codegen parsed
    // them by hand too), so no field-level change is needed to opt in.
    //
    // GOTCHA for whoever writes the first real `#[engine_class(scene_store,
    // ...)]` struct with a `#[gpu(...)]` field (Phase C+): SceneStore's
    // generated GPU-column methods are wrapped in `#[cfg(feature = "gpu")]`.
    // That `cfg`, once spliced into the crate where the struct is actually
    // DEFINED by macro expansion, checks THAT crate's own Cargo features --
    // not `pulsar_scenedb`'s. So the crate defining the struct (e.g.
    // `helio_component`) needs its own feature named exactly "gpu", same
    // as `engine_class_derive`'s own `[features] gpu` (Cargo.toml) exists
    // solely to make this crate's own `tests/scene_store_delegation.rs`
    // compile -- `helio-scenedb/Cargo.toml` documents the identical gotcha.
    //
    // `pulsar_scenedb::Pod` requires `Copy` (a GPU-mirrored/CellStorage-row
    // value is memcpy'd, never dropped in place) -- `scene_store` implies
    // `Copy` for the same reason `default`/`clone`/etc. each imply their
    // own derive, so a component author doesn't have to separately remember
    // it every time.
    if add_scene_store {
        if !has_scene_store_derive {
            derive_additions.push(quote!(::pulsar_scenedb::SceneStore));
        }
        // Skipped for a struct with a `#[gpu] Vec<T>` field -- see
        // `struct_has_gpu_vec_field`'s doc.
        if !has_copy_derive && !struct_has_gpu_vec_field(&item_struct.fields) {
            derive_additions.push(quote!(::core::marker::Copy));
        }
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

    // Tell the `EngineClass` derive macro (below, via `derive_attr`) whether
    // `Serialize`/`Deserialize` end up on this struct at all -- whether we
    // just added them or the user already had them written by hand either
    // way. Has to be a marker attribute, not something the derive macro
    // infers from the `#[derive(...)]` list itself: a `#[proc_macro_derive]`
    // never sees the derive list that triggered it (only its OTHER
    // attributes), confirmed by instrumenting `derive_engine_class` directly
    // -- see that function's own comment on `has_serialize`/`has_deserialize`.
    let serialize_marker_attr = if add_serialize || has_serialize_derive {
        quote! { #[engine_class_serialize] }
    } else {
        quote! {}
    };
    let deserialize_marker_attr = if add_deserialize || has_deserialize_derive {
        quote! { #[engine_class_deserialize] }
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
    // Same deserialize-shim reasoning as `register_runtime_behavior`'s own
    // codegen below (see its comment): `sync_component` is typed `&Self`,
    // but `RuntimeBehaviorRegistration.sync` must be a concrete, non-generic
    // `fn` pointer for `inventory::submit!` and still deals in
    // `&serde_json::Value` (most callers only have JSON at dispatch time),
    // so a small per-type shim bridges the two.
    let runtime_registration = if register_runtime {
        let shim_fn_name = quote::format_ident!("__pulsar_reflection_sync_shim_{}", name);
        quote! {
            #[doc(hidden)]
            #[allow(non_snake_case)]
            fn #shim_fn_name(
                owner: &pulsar_reflection::RuntimeComponentOwner,
                component_index: usize,
                component_data: &::serde_json::Value,
                context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
            ) {
                let parsed: #name = match ::serde_json::from_value(component_data.clone()) {
                    Ok(value) => value,
                    Err(error) => {
                        context.report_error(format!(
                            "{} on '{}' is invalid: {error}",
                            <#name as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                            owner.scene_object_id,
                        ));
                        return;
                    }
                };
                <#name as pulsar_reflection::ComponentRuntimeBehavior>::sync_component(owner, component_index, &parsed, context);
            }

            pulsar_reflection::inventory::submit! {
                pulsar_reflection::RuntimeBehaviorRegistration {
                    class_name: <#name as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                    sync: #shim_fn_name,
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

    // SceneDB storage (Pod/HasTypeToken/SceneColumnSet/GpuColumnSet) is no
    // longer hand-generated here -- `add_scene_store` instead adds
    // `::pulsar_scenedb::SceneStore` to `derive_additions` above, which
    // `#derive_attr` below splices onto `#item_struct` alongside
    // EngineClass/Serialize/etc. This targets `pulsar_scenedb`'s current
    // `World`/`Entity`/`#[gpu(buffer = "...")]` storage model (the one
    // `World::insert`/`get_mut` and the GPU world-mirror actually use)
    // instead of the older `CellStorage`/`Handle`-mirrored model the
    // previous hand-rolled codegen targeted -- confirmed unexercised
    // anywhere in this workspace before this rewrite (nothing called
    // `write_gpu`/`.gpu_columns()`/`SceneColumnSet::cell_type()` outside
    // this file), so retargeting it is purely additive, not a break.

    quote! {
        #derive_attr
        #category_attr
        #no_register_attr
        #serialize_marker_attr
        #deserialize_marker_attr
        #item_struct
        #sub_props_marker_impl
        #runtime_registration
        #scene_props_registration
    }
    .into()
}

#[proc_macro_derive(RegisterRuntimeBehavior)]
pub fn derive_register_runtime_behavior(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let shim_fn_name = quote::format_ident!("__pulsar_reflection_sync_shim_{}", name);

    let generated = quote! {
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #shim_fn_name(
            owner: &pulsar_reflection::RuntimeComponentOwner,
            component_index: usize,
            component_data: &::serde_json::Value,
            context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
        ) {
            let parsed: #name = match ::serde_json::from_value(component_data.clone()) {
                Ok(value) => value,
                Err(error) => {
                    context.report_error(format!(
                        "{} on '{}' is invalid: {error}",
                        <#name as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                        owner.scene_object_id,
                    ));
                    return;
                }
            };
            <#name as pulsar_reflection::ComponentRuntimeBehavior>::sync_component(owner, component_index, &parsed, context);
        }

        pulsar_reflection::inventory::submit! {
            pulsar_reflection::RuntimeBehaviorRegistration {
                class_name: <#name as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                sync: #shim_fn_name,
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
    let Some(self_ty_ident) = (match &**self_ty {
        syn::Type::Path(type_path) => type_path.path.segments.last().map(|s| &s.ident),
        _ => None,
    }) else {
        return syn::Error::new_spanned(
            self_ty,
            "#[register_runtime_behavior] requires a simple named type (no generics, no qualified paths)",
        )
        .to_compile_error()
        .into();
    };
    let shim_fn_name = quote::format_ident!("__pulsar_reflection_sync_shim_{}", self_ty_ident);

    // `RuntimeBehaviorRegistration.sync` is a plain `fn` pointer (`inventory::
    // submit!` needs a concrete static, not a generic) and still deals in
    // `&serde_json::Value` (most callers -- e.g. a scene-file loader -- only
    // have JSON on hand at dispatch time), while `sync_component` itself is
    // typed `&Self` (see `ComponentRuntimeBehavior`'s doc in pulsar_reflection
    // for why). This shim is the one deserialize call that bridges the two,
    // generated here so component authors never hand-write JSON parsing. A
    // parse failure is reported via `ComponentRuntimeContext::report_error`,
    // not a panic.
    let output = quote! {
        #impl_block

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #shim_fn_name(
            owner: &pulsar_reflection::RuntimeComponentOwner,
            component_index: usize,
            component_data: &::serde_json::Value,
            context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
        ) {
            let parsed: #self_ty = match ::serde_json::from_value(component_data.clone()) {
                Ok(value) => value,
                Err(error) => {
                    context.report_error(format!(
                        "{} on '{}' is invalid: {error}",
                        <#self_ty as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                        owner.scene_object_id,
                    ));
                    return;
                }
            };
            <#self_ty as pulsar_reflection::ComponentRuntimeBehavior>::sync_component(owner, component_index, &parsed, context);
        }

        pulsar_reflection::inventory::submit! {
            pulsar_reflection::RuntimeBehaviorRegistration {
                class_name: <#self_ty as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                sync: #shim_fn_name,
            }
        }
    };

    output.into()
}

/// Opt a component into `pulsar_world_registry`'s `World` bridge
/// (Pulsar-Native#555/#556, Phase B4/B5): its typed value can be hydrated
/// from JSON once per edit and inserted into `pulsar_scenedb::World`, then
/// `HelioRenderer::sync_scene` dispatches `ComponentRuntimeBehavior::
/// sync_component` directly off that typed value -- no per-frame
/// `serde_json::from_value` for this component's class.
///
/// Applied *in addition to* `#[register_runtime_behavior]` (same `impl
/// ComponentRuntimeBehavior for Type` block, stack both attributes) -- this
/// is deliberately a separate, opt-in macro so migrating a component onto
/// `World`-backed storage doesn't touch the already-shipped
/// `RuntimeBehaviorRegistration`/JSON dispatch path at all. Components that
/// haven't been migrated yet keep working exactly as before, through that
/// unchanged path.
///
/// Same validation as `#[register_runtime_behavior]` -- see that macro's
/// implementation for why each check exists; kept as a near-identical
/// sibling rather than factored together, since the two attributes are
/// meant to be readable and removable independently as B5 rolls out one
/// component at a time.
/// `#[register_world_component]`'s optional arguments -- `hydrate =
/// path::to::fn`, `remove = path::to::fn`, and `on_removed = path::to::fn`.
/// `hydrate` is an escape hatch for a type that needs to do
/// more at hydrate time than "deserialize this JSON, `world.insert` it"
/// (the auto-generated default). The motivating case (Pulsar-Native#561
/// Phase D): `StaticMeshComponent` owns loading its own mesh file (project-
/// root-relative path resolution, `engine_state::get_project_path()` --
/// already globally accessible, no context object needed -- then parsing
/// the file into vertex/index data) and populating its own `#[gpu]`-mirrored
/// `Vec<T>` fields with the result, once, at the exact point its data
/// changes -- not per render frame, and not through any Helio-specific
/// code (`sync_component`'s dispatch only ever gets `&World`, deliberately
/// -- see that fn's own doc -- so it structurally can't do this; hydrate is
/// the one call site that already has `&mut World`).
///
/// `path` must name a function with EXACTLY the signature the auto-
/// generated hydrate would have had: `fn(&mut pulsar_scenedb::World,
/// pulsar_scenedb::Entity, &serde_json::Value) -> Result<(), String>` --
/// used directly as the registration's function pointer, no wrapper
/// generated, so a signature mismatch is a plain, ordinary compile error at
/// the `WorldComponentRegistration` construction site below, not a
/// mysterious one inside macro-generated code.
struct RegisterWorldComponentArgs {
    custom_hydrate: Option<syn::Path>,
    /// `#[register_world_component(on_removed = path::to::fn)]` -- the
    /// consumer-side teardown counterpart to `hydrate`, called when this
    /// class's component is removed/disabled/despawned (see
    /// `WorldComponentRegistration::on_removed`'s doc, `pulsar_world_registry`).
    /// `path` must name a function with EXACTLY the signature
    /// `fn(&pulsar_reflection::RuntimeComponentOwner, &mut dyn
    /// pulsar_reflection::ComponentRuntimeContext)` -- same "used directly as
    /// the fn pointer, no wrapper" rule as `custom_hydrate` above. Omitted by
    /// default: most components create nothing outside `World` that needs
    /// tearing down, so a generated no-op is the right default (see
    /// `on_removed_fn_def`/`on_removed_fn_ref` below).
    on_removed: Option<syn::Path>,
    /// `#[register_world_component(remove = path::to::fn)]` -- an escape
    /// hatch for a type whose hydrate ALSO populates a companion `World`
    /// component (e.g. `LightComponent`'s auto-generated `#[gpu]`-mirrored
    /// `LightComponentGpuMirror`, Pulsar-Native#561) that removing just
    /// `Self` would leave orphaned.
    /// `path` must name a function with EXACTLY the signature
    /// `fn(&mut pulsar_scenedb::World, pulsar_scenedb::Entity)` -- same
    /// "used directly as the fn pointer, no wrapper" rule as `custom_hydrate`.
    /// Omitted by default: the generated `world.remove::<Self>(entity)` is
    /// correct for any class that doesn't hydrate a companion component.
    custom_remove: Option<syn::Path>,
    /// `#[register_world_component(gpu_mirror)]` -- a bare flag (like
    /// `custom_remove`'s inverse, no `= path`) that makes the DEFAULT
    /// (non-custom) generated `hydrate`/`remove` also call `Self`'s
    /// `pulsar_world_registry::GpuMirrored::sync_gpu_mirror`/
    /// `remove_gpu_mirror`, its `GpuListMirrored::sync_gpu_list_mirror`/
    /// `remove_gpu_list_mirror`, AND its `GpuHeavyMirrored::sync_gpu_heavy_
    /// mirror`/`remove_gpu_heavy_mirror` (the packed-scalar, var-len-list,
    /// and heavy/handle-split companions respectively -- see
    /// `GpuListMirrored`'s/`GpuHeavyMirrored`'s docs for why each is
    /// separate) -- the auto-derived counterpart to
    /// `LightComponent`'s hand-written `hydrate_light_component`/
    /// `remove_light_component` (Pulsar-Native#561). Explicit opt-in rather
    /// than automatic for every type (`#[engine_class]` already generates a
    /// `GpuMirrored` impl unconditionally, including a trivial `NoGpuMirror`
    /// one): `#[register_world_component]` is a SEPARATE macro invocation
    /// (on the `impl ComponentRuntimeBehavior` block, not the struct) with
    /// no visibility into whether `#[engine_class]` found any `#[gpu]`
    /// fields on `Self` -- inserting a `NoGpuMirror` component onto every
    /// entity of every class, unconditionally, would be a real archetype-
    /// fragmentation cost for the overwhelming majority of classes that
    /// have nothing to mirror, so the human states it instead of the two
    /// macros trying to silently coordinate. Only meaningful alongside the
    /// DEFAULT hydrate/remove -- combined with `hydrate = ...`/`remove =
    /// ...`, this flag has no effect (the custom function fully replaces
    /// the generated body); call `sync_gpu_mirror`/`remove_gpu_mirror`
    /// directly from the custom function instead, same as `LightComponent`
    /// does today for its own hand-written (pre-auto-mirror) case.
    gpu_mirror: bool,
}

impl syn::parse::Parse for RegisterWorldComponentArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut custom_hydrate = None;
        let mut on_removed = None;
        let mut custom_remove = None;
        let mut gpu_mirror = false;
        while !input.is_empty() {
            let key: syn::Ident = input.parse()?;
            match key.to_string().as_str() {
                "hydrate" => {
                    let _: syn::Token![=] = input.parse()?;
                    custom_hydrate = Some(input.parse()?);
                }
                "on_removed" => {
                    let _: syn::Token![=] = input.parse()?;
                    on_removed = Some(input.parse()?);
                }
                "remove" => {
                    let _: syn::Token![=] = input.parse()?;
                    custom_remove = Some(input.parse()?);
                }
                "gpu_mirror" => {
                    gpu_mirror = true;
                }
                other => {
                    return Err(syn::Error::new(
                        key.span(),
                        format!("unknown #[register_world_component] option `{other}` (expected `hydrate`, `remove`, `on_removed`, or `gpu_mirror`)"),
                    ))
                }
            }
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            } else {
                break;
            }
        }
        Ok(RegisterWorldComponentArgs { custom_hydrate, on_removed, custom_remove, gpu_mirror })
    }
}

#[proc_macro_attribute]
pub fn register_world_component(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = if attr.is_empty() {
        RegisterWorldComponentArgs {
            custom_hydrate: None,
            on_removed: None,
            custom_remove: None,
            gpu_mirror: false,
        }
    } else {
        match syn::parse::<RegisterWorldComponentArgs>(attr) {
            Ok(args) => args,
            Err(error) => return error.to_compile_error().into(),
        }
    };

    let impl_block = parse_macro_input!(item as ItemImpl);

    if !impl_block.generics.params.is_empty() {
        return syn::Error::new_spanned(
            &impl_block.generics,
            "#[register_world_component] does not support generic impl blocks",
        )
        .to_compile_error()
        .into();
    }

    let Some((_, trait_path, _)) = &impl_block.trait_ else {
        return syn::Error::new_spanned(
            &impl_block.self_ty,
            "#[register_world_component] must be used on `impl ComponentRuntimeBehavior for Type`",
        )
        .to_compile_error()
        .into();
    };

    let Some(trait_ident) = trait_path.segments.last().map(|s| &s.ident) else {
        return syn::Error::new_spanned(
            trait_path,
            "invalid trait path for #[register_world_component]",
        )
        .to_compile_error()
        .into();
    };

    if trait_ident != "ComponentRuntimeBehavior" {
        return syn::Error::new_spanned(
            trait_path,
            "#[register_world_component] must target `ComponentRuntimeBehavior` impl",
        )
        .to_compile_error()
        .into();
    }

    let self_ty = &impl_block.self_ty;
    let Some(self_ty_ident) = (match &**self_ty {
        syn::Type::Path(type_path) => type_path.path.segments.last().map(|s| &s.ident),
        _ => None,
    }) else {
        return syn::Error::new_spanned(
            self_ty,
            "#[register_world_component] requires a simple named type (no generics, no qualified paths)",
        )
        .to_compile_error()
        .into();
    };
    let hydrate_fn_name = quote::format_ident!("__pulsar_world_hydrate_{}", self_ty_ident);
    let remove_fn_name = quote::format_ident!("__pulsar_world_remove_{}", self_ty_ident);
    let dispatch_fn_name = quote::format_ident!("__pulsar_world_dispatch_{}", self_ty_ident);
    let get_fn_name = quote::format_ident!("__pulsar_world_get_engine_class_{}", self_ty_ident);
    let get_mut_fn_name =
        quote::format_ident!("__pulsar_world_get_engine_class_mut_{}", self_ty_ident);
    let on_removed_fn_name = quote::format_ident!("__pulsar_world_on_removed_{}", self_ty_ident);

    // Note this macro does NOT emit `#impl_block` -- unlike
    // `#[register_runtime_behavior]`, it's meant to be stacked alongside
    // that macro on the same impl block, and only one of the two attributes
    // on an item should re-emit the original block (attribute macros
    // compose top-to-bottom; whichever runs first passes its output to the
    // next, so re-emitting from both would duplicate the impl). Convention
    // here: `#[register_runtime_behavior]` keeps ownership of emitting the
    // block; `#[register_world_component]` is written *above* it and must
    // only add new items.
    // Default auto-generated hydrate, emitted only when no `hydrate = path`
    // override was given (see `RegisterWorldComponentArgs`'s doc) -- when
    // one was, the registration below points its `hydrate` field straight
    // at the caller-named function instead, and this default is skipped
    // entirely (never generated, so a hand-written hydrate never competes
    // with an unused generated one under the same name).
    let gpu_mirror_sync = args.gpu_mirror.then(|| {
        quote! {
            <#self_ty as pulsar_world_registry::GpuMirrored>::sync_gpu_mirror(&parsed, world, entity);
            <#self_ty as pulsar_world_registry::GpuListMirrored>::sync_gpu_list_mirror(&parsed, world, entity);
            <#self_ty as pulsar_world_registry::GpuHeavyMirrored>::sync_gpu_heavy_mirror(&parsed, world, entity);
        }
    });
    let (hydrate_fn_def, hydrate_fn_ref) = match &args.custom_hydrate {
        None => (
            quote! {
                #[doc(hidden)]
                #[allow(non_snake_case)]
                fn #hydrate_fn_name(
                    world: &mut pulsar_scenedb::World,
                    entity: pulsar_scenedb::Entity,
                    data: &::serde_json::Value,
                ) -> ::std::result::Result<(), ::std::string::String> {
                    let parsed: #self_ty = ::serde_json::from_value(data.clone())
                        .map_err(|error| error.to_string())?;
                    #gpu_mirror_sync
                    world.insert(entity, parsed);
                    Ok(())
                }
            },
            quote! { #hydrate_fn_name },
        ),
        Some(custom) => (quote! {}, quote! { #custom }),
    };

    // Same optional-override shape as `hydrate` above: a generated no-op
    // when no `on_removed = path` was given (the common case -- most
    // components create nothing outside `World` for `sync_component` to
    // have to unwind), or the caller-named function used directly as the
    // registration's fn pointer otherwise.
    let (on_removed_fn_def, on_removed_fn_ref) = match &args.on_removed {
        None => (
            quote! {
                #[doc(hidden)]
                #[allow(non_snake_case)]
                fn #on_removed_fn_name(
                    _owner: &pulsar_reflection::RuntimeComponentOwner,
                    _context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
                ) {
                }
            },
            quote! { #on_removed_fn_name },
        ),
        Some(custom) => (quote! {}, quote! { #custom }),
    };

    // Same optional-override shape as `hydrate`/`on_removed` above: a
    // generated `world.remove::<Self>(entity)` when no `remove = path` was
    // given (correct for any class that doesn't hydrate a companion
    // component), or the caller-named function used directly otherwise.
    let gpu_mirror_remove = args.gpu_mirror.then(|| {
        quote! {
            <#self_ty as pulsar_world_registry::GpuMirrored>::remove_gpu_mirror(world, entity);
            <#self_ty as pulsar_world_registry::GpuListMirrored>::remove_gpu_list_mirror(world, entity);
            <#self_ty as pulsar_world_registry::GpuHeavyMirrored>::remove_gpu_heavy_mirror(world, entity);
        }
    });
    let (remove_fn_def, remove_fn_ref) = match &args.custom_remove {
        None => (
            quote! {
                #[doc(hidden)]
                #[allow(non_snake_case)]
                fn #remove_fn_name(world: &mut pulsar_scenedb::World, entity: pulsar_scenedb::Entity) {
                    let _ = world.remove::<#self_ty>(entity);
                    #gpu_mirror_remove
                }
            },
            quote! { #remove_fn_name },
        ),
        Some(custom) => (quote! {}, quote! { #custom }),
    };

    let output = quote! {
        #hydrate_fn_def
        #on_removed_fn_def
        #remove_fn_def

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #dispatch_fn_name(
            world: &pulsar_scenedb::World,
            entity: pulsar_scenedb::Entity,
            owner: &pulsar_reflection::RuntimeComponentOwner,
            component_index: usize,
            context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
        ) -> bool {
            match world.get::<#self_ty>(entity) {
                Some(component) => {
                    <#self_ty as pulsar_reflection::ComponentRuntimeBehavior>::sync_component(
                        owner, component_index, component, context,
                    );
                    true
                }
                None => false,
            }
        }

        // Direct live access to the real `World`-resident value as `&(mut)
        // dyn EngineClass` -- this is the properties panel's edit path
        // (Pulsar-Native#561): `get_properties()`'s getter/setter closures
        // already walk `#[sub_props]` nesting correctly, so applying them
        // straight to this reference mutates the one real component in
        // place. No JSON, no throwaway instance, no second copy of the
        // state to keep in sync.
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #get_fn_name(
            world: &pulsar_scenedb::World,
            entity: pulsar_scenedb::Entity,
        ) -> Option<&dyn pulsar_reflection::EngineClass> {
            world
                .get::<#self_ty>(entity)
                .map(|component| component as &dyn pulsar_reflection::EngineClass)
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #get_mut_fn_name(
            world: &mut pulsar_scenedb::World,
            entity: pulsar_scenedb::Entity,
        ) -> Option<&mut dyn pulsar_reflection::EngineClass> {
            // `World::get_mut` returns `Mut<'_, T>` (SceneDB's GPU
            // dirty-mark guard, not a bare `&mut T`) as of the
            // pulsar_scenedb rev this workspace pins post-2026-08-15
            // (Pulsar-Native#561 Phase D). `WorldComponentRegistration.
            // get_as_engine_class_mut` is a plain `fn` pointer with no room
            // to carry `Mut`'s guard through it, so `.into_inner()` (added
            // to `Mut` for exactly this) extracts the raw reference, firing
            // the guard's GPU dispatch immediately for #self_ty's current
            // field values first. None of `helio_component`'s
            // `#[register_world_component]` classes are `#[gpu]`-mirrored
            // today, so that dispatch is a no-op here either way -- see
            // `Mut::into_inner`'s own doc for the (currently moot) caveat
            // that would apply to a future GPU-mirrored class taking this
            // path.
            world
                .get_mut::<#self_ty>(entity)
                .map(|component| component.into_inner() as &mut dyn pulsar_reflection::EngineClass)
        }

        pulsar_world_registry::inventory::submit! {
            pulsar_world_registry::WorldComponentRegistration {
                class_name: <#self_ty as pulsar_reflection::ComponentRuntimeBehavior>::CLASS_NAME,
                component_type: pulsar_scenedb::component_id::<#self_ty>,
                hydrate: #hydrate_fn_ref,
                remove: #remove_fn_ref,
                dispatch: #dispatch_fn_name,
                get_as_engine_class: #get_fn_name,
                get_as_engine_class_mut: #get_mut_fn_name,
                on_removed: #on_removed_fn_ref,
            }
        }

        #impl_block
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

// ── Auto-derived GPU mirroring (Pulsar-Native#561) ──────────────────────────
//
// `#[gpu]` on a `#[property]` field opts it into an auto-generated, `Pod`,
// SceneDB-mirrored companion component -- the pattern `LightComponent`'s own
// hand-written `LightGpuData`/`LightGpuRow` companion (`helio_component`)
// proved out by hand before this generator existed; `LightComponent` itself
// has since been normalized onto this exact generated path (its
// `LightComponentGpuMirror`), with no hand-written companion of its own left
// at all. See `gpu_mirror_codegen`'s doc for the composed-struct shape and
// `pulsar_world_registry::GpuMirrored`'s doc for the runtime contract this
// generates an impl of.

fn has_gpu_attr(field: &Field) -> bool {
    field.attrs.iter().any(|attr| attr.path().is_ident("gpu"))
}

/// One `#[gpu]`-marked leaf field, ready to splice into the generated
/// mirror struct/conversion. Every field is wrapped in `pulsar_world_
/// registry::GpuRepr<FieldTy>` -- see that type's doc for why this is
/// universal (any `Copy` type, no per-shape classification) rather than a
/// hand-maintained list of "recognized" shapes: a `bool` stays 1 byte, an
/// enum stays whatever bytes its own `#[repr]` gives it, an already-Pod
/// custom struct passes through unchanged, all via the exact same wrapper.
/// A field whose type isn't `Copy` fails at the generated struct's own
/// `derive(Clone, Copy)` / `GpuRepr<T>` bound -- a plain, ordinary rustc
/// error at the point that's actually wrong, not a hand-rolled one here.
struct GpuLeafField {
    ident: syn::Ident,
    /// The field's own declared type (unwrapped -- `gpu_mirror_codegen`
    /// wraps it in `GpuRepr<..>` when splicing the mirror struct's field
    /// definition).
    field_ty: syn::Type,
}

/// One `#[gpu] Vec<T>`-marked leaf field for the SEPARATE var-len list
/// mirror (`gpu_list_mirror_codegen`) -- see [`pulsar_world_registry::
/// GpuListMirrored`]'s doc for why this is a second companion, not folded
/// into [`GpuLeafField`]'s packed one. Same universal `GpuRepr<T>` wrapping
/// as the scalar case, applied to the `Vec`'s element type.
struct GpuListLeafField {
    ident: syn::Ident,
    /// `T` in the source field's `Vec<T>`.
    elem_ty: syn::Type,
}

/// Generates the composed `#[gpu]` mirror for one `#[engine_class]`-derived
/// struct (Pulsar-Native#561): a `Pod`, `#[derive(SceneStore)]`,
/// packed-layout companion struct (named `{Struct}GpuMirror`) holding one
/// field per `#[gpu]`-marked `#[property]` leaf PLUS one field per
/// `#[sub_props]` field (that sub-struct's own, independently-generated
/// `<SubTy as GpuMirrored>::GpuMirror` -- this is what makes nesting
/// compose with no special-casing: every `#[engine_class]`-derived struct,
/// `#[sub_props]` groups included, gets a `GpuMirrored` impl unconditionally,
/// so a containing struct never needs to know whether a given sub-props
/// group happens to contribute real fields this time or the zero-sized
/// `NoGpuMirror`).
///
/// No error return: every `#[gpu]` field is accepted unconditionally (see
/// [`GpuLeafField`]'s doc) -- a type that genuinely can't work here (not
/// `Copy`) fails at the generated `GpuRepr<T>` field's own bound, an
/// ordinary rustc error pointing at the real cause.
fn gpu_mirror_codegen(
    name: &syn::Ident,
    gpu_leaf_fields: &[GpuLeafField],
    sub_props_fields: &[&Field],
) -> proc_macro2::TokenStream {
    if gpu_leaf_fields.is_empty() && sub_props_fields.is_empty() {
        // Nothing to mirror -- the trivial, common-case impl.
        return quote! {
            impl pulsar_world_registry::GpuMirrored for #name {
                type GpuMirror = pulsar_world_registry::NoGpuMirror;
                fn to_gpu_mirror(&self) -> pulsar_world_registry::NoGpuMirror {
                    pulsar_world_registry::NoGpuMirror
                }
            }
        };
    }

    let mirror_name = quote::format_ident!("{}GpuMirror", name);

    let leaf_field_defs = gpu_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        let ty = &leaf.field_ty;
        quote! { #[gpu] pub #ident: pulsar_world_registry::GpuRepr<#ty> }
    });
    let leaf_field_inits = gpu_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        quote! { #ident: pulsar_world_registry::GpuRepr(self.#ident) }
    });

    let sub_props_field_defs = sub_props_fields.iter().map(|field| {
        let ident = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        quote! { #[gpu] pub #ident: <#ty as pulsar_world_registry::GpuMirrored>::GpuMirror }
    });
    let sub_props_field_inits = sub_props_fields.iter().map(|field| {
        let ident = field.ident.as_ref().unwrap();
        quote! { #ident: pulsar_world_registry::GpuMirrored::to_gpu_mirror(&self.#ident) }
    });

    quote! {
        // `#[repr(C)]` is load-bearing, not cosmetic: this struct's own
        // field order matters not just for ITS packed buffer, but for
        // correctness when a CONTAINING struct's mirror embeds this one as
        // a plain field value (composition via #[sub_props]) -- the
        // containing struct's own packed view (`pulsar_scenedb_derive`'s
        // `#[repr(C)]`-guaranteed internal `__ScenedbGpuPacked_*` struct)
        // copies THIS struct's bytes as one opaque, `size_of`-sized blob,
        // which is only well-defined if this struct's OWN internal field
        // order is deterministic too -- without `#[repr(C)]` here, Rust's
        // default (unspecified) layout is free to reorder these fields,
        // silently corrupting every OUTER struct that nests this one.
        //
        // No `Default` in this derive list: every field is `GpuRepr<T>`,
        // which only derives `Default` if `T: Default` -- an unnecessary
        // constraint nothing here actually needs (see `pulsar_world_
        // registry::GpuMirrored`'s trait bound, which dropped it for the
        // same reason).
        #[derive(pulsar_scenedb::SceneStore, Clone, Copy)]
        #[gpu(layout = packed)]
        #[repr(C)]
        #[allow(non_snake_case)]
        pub struct #mirror_name {
            #(#leaf_field_defs,)*
            #(#sub_props_field_defs,)*
        }

        impl pulsar_world_registry::GpuMirrored for #name {
            type GpuMirror = #mirror_name;
            fn to_gpu_mirror(&self) -> #mirror_name {
                #mirror_name {
                    #(#leaf_field_inits,)*
                    #(#sub_props_field_inits,)*
                }
            }
        }
    }
}

/// Generates the SEPARATE var-len list mirror for one `#[engine_class]`-
/// derived struct's `#[gpu] Vec<T>` leaf fields -- see
/// [`pulsar_world_registry::GpuListMirrored`]'s doc for why this is its own
/// companion type, not folded into `gpu_mirror_codegen`'s packed one.
///
/// Scope note (v1): does NOT compose `#[sub_props]` fields' own `Vec<T>`
/// leaves into this mirror the way `gpu_mirror_codegen` composes their
/// scalar ones -- only DIRECT `#[gpu] Vec<T>` fields on `#name` itself are
/// collected. No real component needs nested list composition yet (see the
/// design discussion this landed from); extending it is a bounded follow-up
/// if/when one does, not a redesign.
fn gpu_list_mirror_codegen(
    name: &syn::Ident,
    gpu_list_leaf_fields: &[GpuListLeafField],
) -> proc_macro2::TokenStream {
    if gpu_list_leaf_fields.is_empty() {
        return quote! {
            impl pulsar_world_registry::GpuListMirrored for #name {
                type GpuListMirror = pulsar_world_registry::NoGpuMirror;
                fn to_gpu_list_mirror(&self) -> pulsar_world_registry::NoGpuMirror {
                    pulsar_world_registry::NoGpuMirror
                }
            }
        };
    }

    let mirror_name = quote::format_ident!("{}GpuListMirror", name);

    let leaf_field_defs = gpu_list_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        let ty = &leaf.elem_ty;
        quote! { #[gpu] pub #ident: ::std::vec::Vec<pulsar_world_registry::GpuRepr<#ty>> }
    });
    let leaf_field_inits = gpu_list_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        quote! {
            #ident: self.#ident.iter().copied().map(pulsar_world_registry::GpuRepr).collect()
        }
    });

    quote! {
        // No `Default` here either, same reasoning `gpu_mirror_codegen`
        // documents -- `Vec` is already `Default` regardless, but the
        // struct-level derive isn't needed by anything that consumes this.
        #[derive(pulsar_scenedb::SceneStore, Clone)]
        #[allow(non_snake_case)]
        pub struct #mirror_name {
            #(#leaf_field_defs,)*
        }

        impl pulsar_world_registry::GpuListMirrored for #name {
            type GpuListMirror = #mirror_name;
            fn to_gpu_list_mirror(&self) -> #mirror_name {
                #mirror_name {
                    #(#leaf_field_inits,)*
                }
            }
        }
    }
}

/// One `GpuHeavy<T>`-marked leaf field for the SEPARATE heavy/handle-split
/// mirror (`gpu_heavy_mirror_codegen`) -- see [`pulsar_world_registry::
/// GpuHeavyMirrored`]'s doc for why this is a third companion, not folded
/// into either `GpuLeafField`'s packed mirror or `GpuListLeafField`'s var-len
/// one.
struct GpuHeavyLeafField {
    ident: syn::Ident,
    /// `T` in the source field's `GpuHeavy<T>` -- the lightweight CPU
    /// handle type itself, unwrapped. SceneDB's `#[gpu(mirror = Once,
    /// heavy)]` applies directly to the handle field, never to a wrapper
    /// around it.
    handle_ty: syn::Type,
}

/// Generates the SEPARATE heavy/handle-split mirror for one
/// `#[engine_class]`-derived struct's `GpuHeavy<T>` leaf fields -- see
/// [`pulsar_world_registry::GpuHeavyMirrored`]'s doc for why this is its own
/// companion type, and [`pulsar_world_registry::GpuHeavy`]'s doc for why
/// this field shape needs a type-level marker at all (a proc macro can't
/// ask "does this field's type implement `GpuUploadSource`").
///
/// Never `#[gpu(layout = packed)]` -- SceneDB's own derive rejects a
/// `heavy` field inside a packed struct outright (a packed buffer's element
/// is the struct's own interleaved record, not any one field's
/// `GpuUploadSource::Element`), so every handle field here gets its own
/// independently-registered buffer instead, through the ordinary fixed,
/// one-buffer-per-field path -- exactly how SceneDB's `#[gpu(mirror = Once,
/// heavy)]` has always worked, just reached here with no attribute of its
/// own to write.
fn gpu_heavy_mirror_codegen(
    name: &syn::Ident,
    gpu_heavy_leaf_fields: &[GpuHeavyLeafField],
) -> proc_macro2::TokenStream {
    if gpu_heavy_leaf_fields.is_empty() {
        return quote! {
            impl pulsar_world_registry::GpuHeavyMirrored for #name {
                type GpuHeavyMirror = pulsar_world_registry::NoGpuMirror;
                fn to_gpu_heavy_mirror(&self) -> pulsar_world_registry::NoGpuMirror {
                    pulsar_world_registry::NoGpuMirror
                }
            }
        };
    }

    let mirror_name = quote::format_ident!("{}GpuHeavyMirror", name);

    let leaf_field_defs = gpu_heavy_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        let ty = &leaf.handle_ty;
        quote! { #[gpu(mirror = Once, heavy)] pub #ident: #ty }
    });
    let leaf_field_inits = gpu_heavy_leaf_fields.iter().map(|leaf| {
        let ident = &leaf.ident;
        // `self.#ident` is `GpuHeavy<T>`; `.0` is the plain handle SceneDB's
        // `heavy` mechanism actually stores/dirty-tracks.
        quote! { #ident: self.#ident.0 }
    });

    quote! {
        #[derive(pulsar_scenedb::SceneStore, Clone, Copy)]
        #[allow(non_snake_case)]
        pub struct #mirror_name {
            #(#leaf_field_defs,)*
        }

        impl pulsar_world_registry::GpuHeavyMirrored for #name {
            type GpuHeavyMirror = #mirror_name;
            fn to_gpu_heavy_mirror(&self) -> #mirror_name {
                #mirror_name {
                    #(#leaf_field_inits,)*
                }
            }
        }
    }
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
