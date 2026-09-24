//! Proc macro for deriving `Reflectable` trait
//!
//! This crate provides the `#[derive(Reflectable)]` macro that automatically
//! implements runtime type reflection for structs and enums.
//!
//! # Example
//!
//! ```ignore
//! use pulsar_reflection_derive::Reflectable;
//!
//! #[derive(Reflectable, Clone)]
//! pub struct Transform {
//!     pub position: Vec3,
//!     pub rotation: Quat,
//!     pub scale: Vec3,
//! }
//! ```

mod deprecation;
mod derive;
mod enum_impl;
mod field_info;
mod methods;
mod pulsar_type;
mod struct_impl;
mod util;

use proc_macro::TokenStream;

/// Derive macro for Reflectable trait
#[proc_macro_derive(Reflectable, attributes(reflect))]
pub fn derive_reflectable(input: TokenStream) -> TokenStream {
    derive::derive_reflectable(input)
}

/// Attribute macro for runtime type registration.
#[proc_macro_attribute]
pub fn pulsar_type(attr: TokenStream, item: TokenStream) -> TokenStream {
    pulsar_type::pulsar_type(attr, item)
}

/// Register an inherent impl block's methods for reflection.
///
/// Every method marked `#[reflect_method]` is added to
/// `pulsar_reflection::methods`, keyed by the impl's `TypeId`. Methods take
/// `&self`, `&mut self` or no receiver; parameters are owned values, `&T`,
/// `&mut T` (visible to the caller as out-parameters), `&str` (passed as a
/// `String`) or `&[T]` (passed as a `Vec<T>`). A `Result<T, E>` return
/// with `E: Display` reports `Err` as `CallError::Failed`.
///
/// `#[reflect_method(...)]` accepts the flags `pure`, `side_effect_free`
/// and `deterministic`, `name = "..."` to rename the method, and any other
/// `key = "value"` pair as a free-form attribute.
///
/// ```ignore
/// #[reflect_methods]
/// impl Health {
///     /// Remaining hit points.
///     #[reflect_method(pure, category = "Combat")]
///     pub fn current(&self) -> f32 { self.value }
///
///     #[reflect_method]
///     pub fn damage(&mut self, amount: f32) { self.value -= amount; }
/// }
/// ```
#[proc_macro_attribute]
pub fn reflect_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
    methods::reflect_methods(attr, item)
}
