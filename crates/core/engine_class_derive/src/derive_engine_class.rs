//! `#[derive(EngineClass)]`: reflection metadata, SceneDB storage wiring and
//! GPU mirror impls generated off one struct. Entry point: [`expand`].

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Field, Fields, parse_macro_input};

use crate::field_attrs::has_sub_props_attr;
use crate::gpu::{GpuLeafField, gpu_mirror_codegen, has_gpu_attr, parse_gpu_attr};
use crate::properties::{
    PropertyAttrOptions, PropertyCategoryDefinition, extract_class_category,
    extract_property_categories, generate_property_metadata, generate_property_method_items,
    parse_property_attr,
};

pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_class(&input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// The marker attributes the `#[engine_class]` attribute macro stamps on the
/// struct. A derive never sees the `#[derive(...)]` list, only these markers.
struct ClassMarkers {
    /// The `scene_store` flag routed this struct's GPU storage to SceneDB's own
    /// `#[derive(SceneStore)]` (the GPU ownership boundary).
    scene_store_routed: bool,
    /// `#[engine_class(gpu_rows)]`: only then does the generated GPU companion
    /// get a SceneDB row (see `gpu_mirror_codegen`).
    gpu_rows: bool,
    skip_registration: bool,
    /// The class derives `Serialize`, so `to_json` is generated.
    has_serialize: bool,
    /// The class derives `Deserialize`, so a `from_json` shim is registered.
    has_deserialize: bool,
}

impl ClassMarkers {
    fn from_attrs(attrs: &[syn::Attribute]) -> Self {
        let has = |marker: &str| attrs.iter().any(|a| a.path().is_ident(marker));
        Self {
            scene_store_routed: has("engine_class_scene_store"),
            gpu_rows: has("engine_class_gpu_rows"),
            skip_registration: has("engine_class_no_register"),
            has_serialize: has("engine_class_serialize"),
            has_deserialize: has("engine_class_deserialize"),
        }
    }
}

/// What a scan of the struct's fields collected.
struct ScannedFields<'a> {
    property_impls: Vec<proc_macro2::TokenStream>,
    property_fields: Vec<&'a Field>,
    sub_props_fields: Vec<&'a Field>,
    gpu_leaf_fields: Vec<GpuLeafField>,
}

/// The tokens that depend on whether the class is serializable.
struct JsonTokens {
    to_json_impl: proc_macro2::TokenStream,
    from_json_shim: proc_macro2::TokenStream,
    from_json_registration_value: proc_macro2::TokenStream,
}

fn expand_class(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;

    let class_category = extract_class_category(&input.attrs);
    let property_categories = extract_property_categories(&input.attrs)?;
    let category_token = if let Some(cat) = &class_category {
        quote! { Some(#cat) }
    } else {
        quote! { None }
    };

    let markers = ClassMarkers::from_attrs(&input.attrs);
    let ScannedFields {
        property_impls,
        property_fields,
        sub_props_fields,
        gpu_leaf_fields,
    } = scan_fields(input, &property_categories, markers.scene_store_routed)?;

    // A `scene_store` struct's `#[gpu]` fields -- scalar ones included --
    // are mirrored by its own `#[derive(SceneStore)]`, which registers the
    // struct's GPU dispatch. A generated companion would register a second
    // dispatch for the same component, so it gets none; `#[sub_props]`
    // groups (which only a companion can carry) are refused.
    let gpu_leaf_fields = if markers.scene_store_routed {
        if !sub_props_fields.is_empty() {
            return Err(syn::Error::new_spanned(
                input,
                "a `scene_store` #[engine_class] mirrors its #[gpu] fields through its own \
                 #[derive(pulsar_scenedb::SceneStore)], which cannot carry #[sub_props] GPU \
                 groups; move the sub-props' #[gpu] fields onto the struct itself",
            ));
        }
        Vec::new()
    } else {
        gpu_leaf_fields
    };
    let gpu_mirror_tokens =
        gpu_mirror_codegen(name, &gpu_leaf_fields, &sub_props_fields, markers.gpu_rows);

    // Generate auto-property methods (getters and setters). Their metadata
    // rides the property's own category so blueprint palette grouping keeps
    // accessors next to the properties they expose.
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
        .map(|field| sub_props_extender(field, name, &category_order_arms))
        .collect();

    let sub_props_assertions = sub_props_assertions(&sub_props_fields);

    let json = json_tokens(&markers, name);
    let generated = engine_class_impl(
        input,
        &property_impls,
        &sub_props_extenders,
        &property_method_items,
        &json,
    );
    let events_opt_outs = events_opt_outs(&input.data);
    let registration = if markers.skip_registration {
        quote! {}
    } else {
        registration_tokens(
            name,
            &category_token,
            &json.from_json_registration_value,
            &property_method_items,
        )
    };

    Ok(quote! {
        #generated
        #registration
        #(#sub_props_assertions)*
        #(
            pulsar_reflection::inventory::submit! {
                pulsar_reflection::PropertyEventsOptOut {
                    class_name: stringify!(#name),
                    property: #events_opt_outs,
                }
            }
        )*
        #gpu_mirror_tokens
    })
}

/// Compile-time assertions that every #[sub_props] field implements EngineSubProps.
fn sub_props_assertions(sub_props_fields: &[&Field]) -> Vec<proc_macro2::TokenStream> {
    sub_props_fields
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
        .collect()
}

/// Walks the struct's named fields, collecting `#[property]` metadata,
/// `#[sub_props]` fields and packed-scalar `#[gpu]` leaves.
fn scan_fields<'a>(
    input: &'a DeriveInput,
    property_categories: &[PropertyCategoryDefinition],
    scene_store_routed: bool,
) -> syn::Result<ScannedFields<'a>> {
    let Data::Struct(data_struct) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "EngineClass can only be derived for structs",
        ));
    };
    let Fields::Named(fields) = &data_struct.fields else {
        return Err(syn::Error::new_spanned(
            input,
            "EngineClass can only be derived for structs with named fields",
        ));
    };

    let mut scanned = ScannedFields {
        property_impls: Vec::new(),
        property_fields: Vec::new(),
        sub_props_fields: Vec::new(),
        gpu_leaf_fields: Vec::new(),
    };
    for field in &fields.named {
        let has_sub_props = has_sub_props_attr(field);
        let property_attr = parse_property_attr(field);
        let is_gpu = has_gpu_attr(field);

        if property_attr.is_property && has_sub_props {
            return Err(syn::Error::new_spanned(
                field,
                "field cannot use both #[property] and #[sub_props]",
            ));
        }
        if is_gpu && has_sub_props {
            return Err(syn::Error::new_spanned(
                field,
                "#[gpu] on a #[sub_props] field has no effect -- every #[sub_props] \
                 field is composed into the generated GPU mirror automatically \
                 (see that sub-props type's own #[gpu]-marked fields); remove #[gpu] here",
            ));
        }

        if property_attr.is_property {
            let metadata =
                property_metadata_tokens(field, &input.ident, &property_attr, property_categories)?;
            scanned.property_impls.push(metadata);
            scanned.property_fields.push(field);
        }
        if is_gpu && let Some(leaf) = gpu_leaf_field(field, scene_store_routed)? {
            scanned.gpu_leaf_fields.push(leaf);
        }
        if has_sub_props {
            scanned.sub_props_fields.push(field);
        }
    }
    Ok(scanned)
}

/// The metadata tokens of one `#[property]` field, resolving its declared
/// category against the struct's `#[category(..)]` attributes.
fn property_metadata_tokens(
    field: &Field,
    name: &syn::Ident,
    property_attr: &PropertyAttrOptions,
    property_categories: &[PropertyCategoryDefinition],
) -> syn::Result<proc_macro2::TokenStream> {
    let category_decl = if let Some(cat) = property_attr.category.as_ref() {
        let Some(decl) = property_categories.iter().find(|d| d.name == *cat) else {
            return Err(syn::Error::new_spanned(
                field,
                format!(
                    "property category '{}' is not declared; add #[category(\"{}\", ...)] on the struct",
                    cat, cat
                ),
            ));
        };
        Some(decl)
    } else {
        None
    };
    Ok(generate_property_metadata(
        field,
        name,
        property_attr,
        category_decl,
    ))
}

/// Classifies a `#[gpu]` field. GPU ownership boundary: packed-scalar
/// `#[gpu]` fields are THIS derive's only storage concern. Var-len (`Vec<T>`)
/// and heavy (`GpuHeavy<T>`) shapes belong exclusively to SceneDB's own
/// `#[derive(SceneStore)]` -- a struct declares that routing via the
/// `scene_store` flag, which the attribute macro stamps as an
/// `#[engine_class_scene_store]` marker (the derive can never see the derive
/// list itself). Unrouted misuse is a compile error, not silent inertness.
/// Returns the leaf for a packed scalar and `None` for a routed shape.
fn gpu_leaf_field(field: &Field, scene_store_routed: bool) -> syn::Result<Option<GpuLeafField>> {
    if is_gpu_heavy_shape(&field.ty) {
        if !scene_store_routed {
            return Err(syn::Error::new_spanned(
                field,
                "#[gpu] GpuHeavy<T> fields use SceneDB's heavy/handle-split \
                 storage, owned by this struct's own \
                 #[derive(pulsar_scenedb::SceneStore)] -- add the \
                 `scene_store` flag to #[engine_class]. This derive \
                 generates packed-scalar companions only.",
            ));
        }
        return Ok(None);
    }
    if is_vec_type(&field.ty) {
        if !scene_store_routed {
            return Err(syn::Error::new_spanned(
                field,
                "#[gpu] Vec<T> fields store through this struct's own \
                 #[derive(pulsar_scenedb::SceneStore)] (add the \
                 `scene_store` flag to #[engine_class]). This derive \
                 generates packed-scalar companions only.",
            ));
        }
        return Ok(None);
    }
    let ident = field
        .ident
        .clone()
        .expect("this is only called for named fields, which have identifiers");
    let override_ = parse_gpu_attr(field)?;
    Ok(Some(GpuLeafField {
        ident,
        field_ty: field.ty.clone(),
        override_,
    }))
}

/// Flattens a `#[sub_props]` field's properties into the parent's property
/// list, remapping their category order through the parent's declarations.
fn sub_props_extender(
    field: &Field,
    name: &syn::Ident,
    category_order_arms: &[proc_macro2::TokenStream],
) -> proc_macro2::TokenStream {
    let field_name = field
        .ident
        .as_ref()
        .expect("sub_props fields come from Fields::Named, so they have identifiers");
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
                let concrete = obj.as_any().downcast_ref::<#name>().expect("the registry only calls this accessor with an instance of the class that generated it");
                nested_getter(&concrete.#field_name as &dyn pulsar_reflection::EngineClass)
            });

            let setter = Box::new(move |obj: &mut dyn pulsar_reflection::EngineClass, value: Box<dyn std::any::Any>| {
                let concrete = obj.as_any_mut().downcast_mut::<#name>().expect("the registry only calls this accessor with an instance of the class that generated it");
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
}

/// Whole-instance JSON round trip: `to_json`/`from_json` only make sense for
/// classes that actually derive `Serialize`/`Deserialize`. A
/// `#[proc_macro_derive]` never sees the `#[derive(...)]` list that
/// triggered it, so `#[engine_class(..., serialize, deserialize, ...)]`
/// stamps marker attributes onto the struct, and those are what `markers`
/// carries.
fn json_tokens(markers: &ClassMarkers, name: &syn::Ident) -> JsonTokens {
    let to_json_impl = if markers.has_serialize {
        quote! {
            fn to_json(&self) -> Result<::serde_json::Value, String> {
                ::serde_json::to_value(self).map_err(|error| error.to_string())
            }
        }
    } else {
        // No override -- inherit `EngineClass::to_json`'s default `Err`.
        quote! {}
    };

    let shim_fn_name = quote::format_ident!("__pulsar_reflection_from_json_shim_{}", name);
    let from_json_shim = if markers.has_deserialize {
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
    let from_json_registration_value = if markers.has_deserialize {
        quote! { Some(#shim_fn_name) }
    } else {
        quote! { None }
    };
    JsonTokens {
        to_json_impl,
        from_json_shim,
        from_json_registration_value,
    }
}

/// The `EngineClass` trait implementation plus the optional `from_json` shim.
fn engine_class_impl(
    input: &DeriveInput,
    property_impls: &[proc_macro2::TokenStream],
    sub_props_extenders: &[proc_macro2::TokenStream],
    property_method_items: &[proc_macro2::TokenStream],
    json: &JsonTokens,
) -> proc_macro2::TokenStream {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let JsonTokens {
        to_json_impl,
        from_json_shim,
        ..
    } = json;
    quote! {
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
    }
}

/// Properties that opted out of raising asset events (`events = false`).
fn events_opt_outs(data: &Data) -> Vec<String> {
    match data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields) => fields
                .named
                .iter()
                .filter(|field| parse_property_attr(field).no_events)
                .filter_map(|field| field.ident.as_ref().map(|ident| ident.to_string()))
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The inventory submissions that register the class and its property methods.
fn registration_tokens(
    name: &syn::Ident,
    category_token: &proc_macro2::TokenStream,
    from_json_registration_value: &proc_macro2::TokenStream,
    property_method_items: &[proc_macro2::TokenStream],
) -> proc_macro2::TokenStream {
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
