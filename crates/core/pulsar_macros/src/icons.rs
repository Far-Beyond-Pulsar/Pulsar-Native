//! `generate_icon_enum!`: an icon enum generated from a directory of SVG files.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse_macro_input;

/// Convert an SVG filename to a PascalCase identifier.
///
/// Convention: lowercase the filename, strip `.svg`, split on `-`,
/// capitalize the first letter of each segment, join.
/// Underscores are preserved as-is (e.g. `android_dark.svg` → `Android_dark`).
fn filename_to_pascal(filename: &str) -> String {
    let name = filename.strip_suffix(".svg").unwrap_or(filename);
    let lowered = name.to_lowercase();
    lowered
        .split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect()
}

// Proc macro: runs inside rustc at compile time and reads the crate's own source dir; it is not engine runtime code.
#[allow(clippy::disallowed_methods)]
pub(crate) fn expand(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::LitStr);
    let relative_path = input.value();

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let icons_dir = std::path::Path::new(&manifest_dir).join(&relative_path);

    let mut entries: Vec<(String, String)> = Vec::new();

    let dir = std::fs::read_dir(&icons_dir).unwrap_or_else(|e| {
        panic!(
            "generate_icon_enum: failed to read directory '{}': {}",
            icons_dir.display(),
            e
        )
    });

    for entry in dir {
        let entry = entry.expect("failed to read directory entry");
        let filename = entry.file_name().to_string_lossy().to_string();
        if filename.ends_with(".svg") {
            let variant_name = filename_to_pascal(&filename);
            let path = format!("icons/{}", filename);
            entries.push((variant_name, path));
        }
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let variants: Vec<proc_macro2::Ident> = entries
        .iter()
        .map(|(name, _)| proc_macro2::Ident::new(name, proc_macro2::Span::call_site()))
        .collect();
    let paths: Vec<&str> = entries.iter().map(|(_, p)| p.as_str()).collect();

    let expanded = quote! {
        #[derive(IntoElement, Clone, Debug)]
        pub enum IconName {
            #(#variants,)*
        }

        impl IconName {
            pub fn path(self) -> SharedString {
                match self {
                    #(Self::#variants => #paths,)*
                }
                .into()
            }
        }
    };

    TokenStream::from(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_conversion_follows_documented_convention() {
        assert_eq!(filename_to_pascal("arrow-up.svg"), "ArrowUp");
        assert_eq!(filename_to_pascal("ARROW-UP-RIGHT.svg"), "ArrowUpRight");
        assert_eq!(filename_to_pascal("android_dark.svg"), "Android_dark");
        assert_eq!(filename_to_pascal("plain"), "Plain");
        assert_eq!(filename_to_pascal(".svg"), "");
    }
}
