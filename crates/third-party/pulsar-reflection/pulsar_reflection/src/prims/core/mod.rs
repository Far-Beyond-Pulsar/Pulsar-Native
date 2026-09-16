//! Core primitive type registrations.

mod bool;
pub mod enum_dropdown;
mod f32;
mod f64;
mod i32;
mod i64;
mod u64;
mod vec3;

/// Public so consumers can reuse the RGBA↔HSLA conversion the colour editor
/// uses — several panels colour-code pin badges with the same maths.
pub mod color;
mod u32;
