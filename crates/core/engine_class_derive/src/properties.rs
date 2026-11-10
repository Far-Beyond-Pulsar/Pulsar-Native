//! `#[property]` / `#[category]` parsing and the generated reflection
//! metadata and accessors.

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Expr, Field, Lit, Meta, MetaNameValue,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

#[derive(Default)]
pub(crate) struct PropertyAttrOptions {
    pub(crate) is_property: bool,
    pub(crate) category: Option<String>,
    pub(crate) category_color: Option<String>,
    /// Display label (`label = "..."`), replacing the title-cased field name.
    pub(crate) label: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct PropertyCategoryDefinition {
    pub(crate) name: String,
    pub(crate) category_color: Option<String>,
    pub(crate) default_collapsed: bool,
    pub(crate) order: usize,
}

pub(crate) struct CategoryAttrArgs {
    pub(crate) name: syn::LitStr,
    pub(crate) options: Punctuated<MetaNameValue, syn::Token![,]>,
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
pub(crate) fn parse_property_attr(field: &Field) -> PropertyAttrOptions {
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
                if name_value.path.is_ident("label")
                    && let Expr::Lit(expr_lit) = &name_value.value
                    && let Lit::Str(lit_str) = &expr_lit.lit
                {
                    out.label = Some(lit_str.value());
                }
            }
        }
    }

    out
}

/// Extract engine-class category (registry grouping) from struct-level attributes.
pub(crate) fn extract_class_category(attrs: &[Attribute]) -> Option<String> {
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
pub(crate) fn extract_property_categories(
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
pub(crate) fn generate_property_metadata(
    field: &Field,
    struct_name: &syn::Ident,
    property_attr: &PropertyAttrOptions,
    category_decl: Option<&PropertyCategoryDefinition>,
) -> proc_macro2::TokenStream {
    let field_name = field.ident.as_ref().unwrap();
    let field_name_str = field_name.to_string();
    // Title-cased, space-separated label (#645): "linear_damping" renders
    // as "Linear Damping", consistent with method display names.
    let display_name = property_attr
        .label
        .clone()
        .unwrap_or_else(|| title_case(&field_name_str.replace('_', " ")));
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
pub(crate) fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Title-case every whitespace-separated word -- "add charges" -> "Add
/// Charges". Node display names are palette/search surfaces (#645): mixed
/// casing ("Set linear damping" vs "Add Charges") reads as inconsistency,
/// so every generated display name goes through this.
pub(crate) fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(capitalize_first)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Generate getter and setter method metadata items for properties.
///
/// `field_categories[i]` parallels `fields[i]`: the property's resolved
/// category, so generated accessors group with their field in the palette
/// instead of landing as ungrouped strays (#645).
pub(crate) fn generate_property_method_items(
    fields: &[&Field],
    field_categories: &[Option<String>],
    struct_name: &syn::Ident,
) -> Vec<proc_macro2::TokenStream> {
    let mut method_items = Vec::new();

    for (index, field) in fields.iter().enumerate() {
        let field_name = field.ident.as_ref().unwrap();
        let field_name_str = field_name.to_string();
        let getter_name = format!("get_{}", field_name_str);
        let setter_name = format!("set_{}", field_name_str);
        let getter_display = title_case(&format!("Get {}", field_name_str));
        let setter_display = title_case(&format!("Set {}", field_name_str));
        let category_expr = match field_categories.get(index).and_then(|c| c.as_deref()) {
            Some(cat) => quote! { Some(#cat) },
            None => quote! { None },
        };
        let field_type = &field.ty;

        method_items.push(quote! {
            pulsar_reflection::MethodMetadata {
                name: #getter_name,
                display_name: #getter_display.to_string(),
                category: #category_expr,
                params: vec![],
                return_type: Some(pulsar_reflection::MethodReturnType {
                    type_info: <#field_type as pulsar_reflection::Reflectable>::type_info(),
                }),
                flags: pulsar_reflection::MethodFlags { side_effect_free: true, deterministic: false },
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
                category: #category_expr,
                params: vec![
                    pulsar_reflection::MethodParameter {
                        name: "value",
                        type_info: <#field_type as pulsar_reflection::Reflectable>::type_info(),
                    }
                ],
                return_type: None,
                flags: pulsar_reflection::MethodFlags::NONE,
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
