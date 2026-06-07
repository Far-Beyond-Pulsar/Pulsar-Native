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
