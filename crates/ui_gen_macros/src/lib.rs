//! UI Generation Macros - Compile-time type introspection for automatic UI generation
//!
//! This crate provides procedural macros that analyze Rust types at compile time
//! and generate field metadata that can be used to create data-driven UIs.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{ImplItem, ItemImpl, Type, parse_macro_input};

// ─── register_window attribute macro ─────────────────────────────────────────

/// Attribute for `impl PulsarWindow for T` blocks.
///
/// When `type Params = ()`, automatically generates an `inventory::submit!` that
/// registers the window in the global [`WindowRegistry`] via [`PulsarWindowExt::register`].
///
/// Non-zero-param windows pass through unchanged (they must register manually,
/// e.g. from `PulsarApp` init with a captured entity).
///
/// # Example
/// ```ignore
/// #[window_manager::register_window]
/// impl PulsarWindow for SettingsWindow {
///     type Params = ();
///     fn window_name() -> &'static str { "SettingsWindow" }
///     fn window_options(_: &()) -> WindowOptions { default_window_options(1000.0, 700.0) }
///     fn build(_: (), window: &mut Window, cx: &mut App) -> Entity<Self> { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn register_window(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let impl_block = parse_macro_input!(item as ItemImpl);

    let self_ty = &impl_block.self_ty;

    let is_zero_param = impl_block.items.iter().any(|item| {
        if let ImplItem::Type(ty_item) = item {
            if ty_item.ident == "Params" {
                return matches!(&ty_item.ty, Type::Tuple(t) if t.elems.is_empty());
            }
        }
        false
    });

    // Generate a unique-ish identifier using the type name for the static.
    let type_name = quote!(#self_ty)
        .to_string()
        .replace("::", "_")
        .replace(" ", "");
    let static_ident = syn::Ident::new(
        &format!("__REGISTER_WINDOW_{}", type_name),
        proc_macro2::Span::call_site(),
    );

    let submit: TokenStream2 = if is_zero_param {
        quote! {
            #[::linkme::distributed_slice(::window_manager::WINDOW_REGISTRANTS)]
            static #static_ident: fn(&mut ::gpui::App) = |cx| {
                use ::ui_common::PulsarWindowExt as _;
                <#self_ty as ::ui_common::PulsarWindowExt>::register(cx);
            };
        }
    } else {
        quote! {}
    };

    quote! {
        #impl_block
        #submit
    }
    .into()
}
