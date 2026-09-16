//! Primitive type implementations module
//!
//! Primitive registration is organized by feature-gated submodules.
//!
//! - `core`: language/core primitives and fixed-size numeric arrays
//! - `std`: standard library primitives (currently `String`)
//! - `serde`: serde-focused primitives (currently `serde_json::Value`)
//!
//! Primitive modules use `#[pulsar_type(primitive)]` aliases to generate
//! `Reflectable` implementations and inventory registration.

#[cfg(feature = "prims-core")]
pub mod core;

#[cfg(feature = "prims-std")]
pub mod std;

#[cfg(feature = "prims-serde")]
pub mod serde;

#[cfg(feature = "prims-glam")]
pub mod glam;

#[cfg(feature = "prims-helio")]
pub mod helio;

/// Shared layout for a built-in editor: muted label on the left, the editor's
/// own control on the right.
///
/// Kept here so every primitive editor lays out identically without any of
/// them having to agree on styling by hand.
#[cfg(feature = "prims-gpui")]
pub fn editor_row(
    label: &str,
    control: impl gpui::IntoElement,
    cx: &gpui::App,
) -> gpui::AnyElement {
    use gpui::prelude::*;
    use ui::{ActiveTheme, h_flex};

    h_flex()
        .w_full()
        .justify_between()
        .items_center()
        .gap_2()
        .child(
            gpui::div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(control)
        .into_any_element()
}
