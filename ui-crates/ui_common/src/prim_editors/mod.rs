//! Per-type GPUI property editor registrations.
//!
//! Each sub-module owns exactly one type's editor:
//! - defines the render function
//! - submits a [`pulsar_reflection::UiPropertyEditorHint`] via `inventory::submit!`
//!
//! The modules must be included (via `mod`) so the `inventory::submit!` statics
//! are linked into the binary.  No other wiring is required — the
//! [`crate::property_editor_registry::PROPERTY_EDITOR_REGISTRY`] picks them up
//! automatically at startup.

mod bool;
mod color;
mod f32;
mod i32;
mod mesh_asset;
mod string;
mod vec3;
