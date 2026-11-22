//! Reading values out of macro attribute text and `#[bp_import]` attributes.

use quote::quote;
use syn::ItemFn;

/// Extract a string value from an attribute string like `category: "Math"`
pub(crate) fn extract_string_value(attr_str: &str, key: &str) -> Option<String> {
    if let Some(key_pos) = attr_str.find(key) {
        if let Some(quote_start) = attr_str[key_pos..].find('"') {
            let quote_start = key_pos + quote_start + 1;
            if let Some(quote_end) = attr_str[quote_start..].find('"') {
                return Some(attr_str[quote_start..quote_start + quote_end].to_string());
            }
        }
    }
    None
}

/// Extract bp_import attributes from a function
pub(crate) fn extract_bp_imports(func: &ItemFn) -> Vec<proc_macro2::TokenStream> {
    let mut imports = Vec::new();

    for attr in &func.attrs {
        if attr.path().is_ident("bp_import") {
            // Parse the import specification
            if let Ok(import_spec) = parse_bp_import_attr(attr) {
                imports.push(import_spec);
            }
        }
    }

    imports
}

/// Parse a bp_import attribute into NodeImport tokens
/// Handles forms like:
/// - #[bp_import(reqwest)]
/// - #[bp_import(reqwest::Client)]
/// - #[bp_import(reqwest::{Client, Error})]
fn parse_bp_import_attr(attr: &syn::Attribute) -> syn::Result<proc_macro2::TokenStream> {
    let tokens = attr.meta.require_list()?.tokens.clone();
    let tokens_str = tokens.to_string();

    // Parse the import path
    // Format can be: "crate_name" or "crate_name :: item" or "crate_name :: { item1 , item2 }"
    let (crate_name, items) = parse_import_path(&tokens_str);

    let items_array = if items.is_empty() {
        quote! { &[] }
    } else {
        quote! { &[#(#items),*] }
    };

    Ok(quote! {
        crate::NodeImport {
            crate_name: #crate_name,
            items: #items_array,
        }
    })
}

/// Parse an import path string like "reqwest::{Client, Error}" into (crate_name, [items])
fn parse_import_path(path_str: &str) -> (String, Vec<String>) {
    let path_str = path_str.trim();

    // Check if there's a :: separator
    if let Some(sep_pos) = path_str.find("::") {
        let crate_name = path_str[..sep_pos].trim().to_string();
        let rest = path_str[sep_pos + 2..].trim();

        // Check if items are in braces
        if rest.starts_with('{') && rest.ends_with('}') {
            // Parse items from braces
            let items_str = &rest[1..rest.len() - 1];
            let items: Vec<String> = items_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (crate_name, items)
        } else {
            // Single item without braces
            (crate_name, vec![rest.to_string()])
        }
    } else {
        // No ::, just a crate name
        (path_str.to_string(), vec![])
    }
}

/// Extract a number value from an attribute string like `params: 1`
pub(crate) fn extract_number_value(attr_str: &str, key: &str) -> Option<usize> {
    if let Some(key_pos) = attr_str.find(key) {
        let after_key = &attr_str[key_pos + key.len()..];
        if let Some(colon_pos) = after_key.find(':') {
            let after_colon = &after_key[colon_pos + 1..];
            // Find the first sequence of digits
            let digits: String = after_colon
                .chars()
                .skip_while(|c| !c.is_ascii_digit())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(num) = digits.parse() {
                return Some(num);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_values_are_read_after_their_key() {
        let attrs = r##"type: NodeTypes::pure, category: "Math", color: "#fff""##;
        assert_eq!(
            extract_string_value(attrs, "category").as_deref(),
            Some("Math")
        );
        assert_eq!(
            extract_string_value(attrs, "color").as_deref(),
            Some("#fff")
        );
        assert_eq!(extract_string_value(attrs, "missing"), None);
        assert_eq!(
            extract_string_value(r#"category: "unterminated"#, "category"),
            None
        );
    }

    #[test]
    fn number_values_take_first_digit_run_after_colon() {
        assert_eq!(
            extract_number_value("params: 3, other: 9", "params"),
            Some(3)
        );
        assert_eq!(extract_number_value("params:12", "params"), Some(12));
        assert_eq!(extract_number_value("params: none", "params"), None);
        assert_eq!(extract_number_value("other: 1", "params"), None);
    }

    #[test]
    fn import_paths_split_into_crate_and_items() {
        assert_eq!(
            parse_import_path("reqwest::{Client, Error}"),
            (
                "reqwest".to_string(),
                vec!["Client".to_string(), "Error".to_string()]
            )
        );
        assert_eq!(
            parse_import_path("  serde::Serialize "),
            ("serde".to_string(), vec!["Serialize".to_string()])
        );
        assert_eq!(
            parse_import_path("regex"),
            ("regex".to_string(), Vec::new())
        );
        assert_eq!(
            parse_import_path("a::{ , B ,}"),
            ("a".to_string(), vec!["B".to_string()])
        );
    }
}
