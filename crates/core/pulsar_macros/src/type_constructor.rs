//! `#[blueprint_type]`: registers a type constructor for the Blueprint type system.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

use crate::attrs::{extract_number_value, extract_string_value};

pub(crate) fn expand(args: TokenStream, input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::ItemType);
    let args_str = args.to_string();

    let type_name = &input.ident;
    let type_name_str = type_name.to_string();

    // Extract the unwrapped name (the actual Rust type like "Arc", "Box")
    let constructor_name = extract_string_value(&args_str, "unwrapped_name").unwrap_or_else(|| {
        // Default: strip 'P' prefix if present (PArc -> Arc)
        type_name_str
            .strip_prefix('P')
            .unwrap_or(&type_name_str)
            .to_string()
    });

    // Parse parameters count
    let params_count = extract_number_value(&args_str, "params").unwrap_or(1);

    // Extract category
    let category =
        extract_string_value(&args_str, "category").unwrap_or_else(|| "Other".to_string());

    // Extract description
    let description = extract_string_value(&args_str, "description")
        .unwrap_or_else(|| format!("{} type constructor", constructor_name));

    // Extract example
    let example = extract_string_value(&args_str, "example").unwrap_or_else(|| {
        if params_count == 1 {
            format!("{}<T>", constructor_name)
        } else if params_count == 2 {
            format!("{}<T, E>", constructor_name)
        } else {
            format!("{}<...>", constructor_name)
        }
    });

    // Generate the registration const
    let registry_ident = syn::Ident::new(
        &format!("__TYPE_CONSTRUCTOR__{}", constructor_name.to_uppercase()),
        type_name.span(),
    );

    let expanded = quote! {
        #[allow(dead_code)]
        #input

        #[cfg(feature = "native")]
        #[::linkme::distributed_slice(crate::registry::native_type_registry::TYPE_CONSTRUCTOR_REGISTRY)]
        #[linkme(crate = ::linkme)]
        static #registry_ident: crate::TypeConstructorMetadata = crate::TypeConstructorMetadata {
            name: #constructor_name,
            params_count: #params_count,
            category: #category,
            description: #description,
            example: #example,
        };
    };

    TokenStream::from(expanded)
}
