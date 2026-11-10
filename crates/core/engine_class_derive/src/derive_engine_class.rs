//! `#[derive(EngineClass)]`: reflection metadata, SceneDB storage wiring and
//! GPU mirror impls generated off one struct. Entry point: [`expand`].

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

use crate::field_attrs::has_sub_props_attr;
use crate::gpu::{GpuLeafField, gpu_mirror_codegen, has_gpu_attr, parse_gpu_attr};
use crate::properties::{
    extract_class_category, extract_property_categories, generate_property_metadata,
    generate_property_method_items, parse_property_attr,
};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
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

    // GPU ownership boundary signal (see the field-scan comment below): the
    // attribute macro stamps `#[engine_class_scene_store]` when the
    // `scene_store` flag routed this struct's GPU storage to SceneDB's own
    // `#[derive(SceneStore)]`. Same mechanism/limitation as the serialize /
    // deserialize markers further down -- a derive never sees the derive
    // list, only these stamped markers.
    let scene_store_routed = input
        .attrs
        .iter()
        .any(|a| a.path().is_ident("engine_class_scene_store"));
    // `#[engine_class(gpu_rows)]` stamps this marker: only then does the
    // generated GPU companion get a SceneDB row (see `gpu_mirror_codegen`).
    let gpu_rows = input
        .attrs
        .iter()
        .any(|a| a.path().is_ident("engine_class_gpu_rows"));

    // Extract direct #[property] fields and optional #[sub_props] flattening fields.
    let (property_impls, property_fields, sub_props_fields, gpu_leaf_fields): (
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
                    // GPU ownership boundary (engine-class GPU de-duplication
                    // audit): packed-scalar `#[gpu]` fields are THIS derive's
                    // only storage concern. Var-len (`Vec<T>`) and heavy
                    // (`GpuHeavy<T>`) shapes belong exclusively to SceneDB's
                    // own `#[derive(SceneStore)]` -- a struct declares that
                    // routing via the `scene_store` flag, which the attribute
                    // macro stamps as an `#[engine_class_scene_store]` marker
                    // (the derive can never see the derive list itself).
                    // Unrouted misuse is a compile error, not silent
                    // inertness.
                    if is_gpu {
                        if is_gpu_heavy_shape(&field.ty) {
                            if !scene_store_routed {
                                return syn::Error::new_spanned(
                                    field,
                                    "#[gpu] GpuHeavy<T> fields use SceneDB's heavy/handle-split \
                                     storage, owned by this struct's own \
                                     #[derive(pulsar_scenedb::SceneStore)] -- add the \
                                     `scene_store` flag to #[engine_class]. This derive \
                                     generates packed-scalar companions only.",
                                )
                                .to_compile_error()
                                .into();
                            }
                        } else if is_vec_type(&field.ty) {
                            if !scene_store_routed {
                                return syn::Error::new_spanned(
                                    field,
                                    "#[gpu] Vec<T> fields store through this struct's own \
                                     #[derive(pulsar_scenedb::SceneStore)] (add the \
                                     `scene_store` flag to #[engine_class]). This derive \
                                     generates packed-scalar companions only.",
                                )
                                .to_compile_error()
                                .into();
                            }
                        } else {
                            let field_ident = field.ident.clone().unwrap();
                            let override_ = match parse_gpu_attr(field) {
                                Ok(o) => o,
                                Err(err) => return err.to_compile_error().into(),
                            };
                            gpu_fields.push(GpuLeafField {
                                ident: field_ident,
                                field_ty: field.ty.clone(),
                                override_,
                            });
                        }
                    }

                    if has_sub_props {
                        sub_props.push(field);
                    }
                }
                let (impls, fields): (Vec<_>, Vec<_>) = props.into_iter().unzip();
                (impls, fields, sub_props, gpu_fields)
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

    // A `scene_store` struct's `#[gpu]` fields -- scalar ones included --
    // are mirrored by its own `#[derive(SceneStore)]`, which registers the
    // struct's GPU dispatch. A generated companion would register a second
    // dispatch for the same component, so it gets none; `#[sub_props]`
    // groups (which only a companion can carry) are refused.
    let gpu_leaf_fields = if scene_store_routed {
        if !sub_props_fields.is_empty() {
            return syn::Error::new_spanned(
                &input,
                "a `scene_store` #[engine_class] mirrors its #[gpu] fields through its own \
                 #[derive(pulsar_scenedb::SceneStore)], which cannot carry #[sub_props] GPU \
                 groups; move the sub-props' #[gpu] fields onto the struct itself",
            )
            .to_compile_error()
            .into();
        }
        Vec::new()
    } else {
        gpu_leaf_fields
    };
    let gpu_mirror_tokens = gpu_mirror_codegen(name, &gpu_leaf_fields, &sub_props_fields, gpu_rows);

    // Generate auto-property methods (getters and setters). Their metadata
    // rides the property's own category so blueprint palette grouping keeps
    // accessors next to the properties they expose (#645).
    let accessor_categories: Vec<Option<String>> = property_fields
        .iter()
        .map(|field| parse_property_attr(field).category)
        .collect();
    let property_method_items =
        generate_property_method_items(&property_fields, &accessor_categories, name);
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

                // Methods registered by hand through `ComponentMethodRegistration`
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
pub(crate) fn struct_has_gpu_vec_field(fields: &syn::Fields) -> bool {
    fields.iter().any(|field| {
        let has_gpu_attr = field.attrs.iter().any(|attr| attr.path().is_ident("gpu"));
        has_gpu_attr && is_vec_type(&field.ty)
    })
}

pub(crate) fn is_vec_type(ty: &syn::Type) -> bool {
    let syn::Type::Path(type_path) = ty else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .map(|s| s.ident == "Vec")
        .unwrap_or(false)
}

/// Syntactic-only check that `ty` is `pulsar_world_registry::GpuHeavy<T>` --
/// same last-segment style as `is_vec_type`. Used solely by the GPU ownership
/// boundary errors; heavy storage itself is SceneDB's SceneStore concern.
pub(crate) fn is_gpu_heavy_shape(ty: &syn::Type) -> bool {
    let syn::Type::Path(type_path) = ty else {
        return false;
    };
    type_path
        .path
        .segments
        .last()
        .map(|s| s.ident == "GpuHeavy")
        .unwrap_or(false)
}
