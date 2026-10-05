//! glam math types as script value types.
//!
//! Scripts see `Vec2`, `Vec3`, `Vec4`, `DVec3`, `Quat` and `Mat4` as
//! registered value types (`Type::Object("Vec3")`, ..). They have value
//! semantics: every read clones, so mutating a copy never affects the
//! original. Script floats are `f64`; the single-precision types convert
//! with `as f32` on the way in and `as f64` on the way out.
//!
//! # Literal form
//!
//! A [`Constant::Value`](pulsar_script_vm::Constant) carries JSON arrays of
//! numbers, the same layout glam's serde support uses: `Vec3` is `[x, y, z]`,
//! `Quat` is `[x, y, z, w]`, and `Mat4` is 16 numbers in column-major order
//! (`Mat4::to_cols_array`).
//!
//! # Equality
//!
//! The VM never compares objects with `==`. `T::eq` is raw component
//! equality. `T::approx_eq` takes a tolerance, and `Quat::same_rotation`
//! treats `q` and `-q` as the same rotation.

mod natives;

use glam::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use pulsar_script_vm::{script_value_ops, script_value_type};

/// Parse `text` as exactly `N` finite numbers.
fn numbers<const N: usize>(text: &str) -> Result<[f64; N], String> {
    let values: Vec<f64> = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let found = values.len();
    let array: [f64; N] = values.try_into().map_err(|_| format!("expected {N} numbers, found {found}"))?;
    match array.iter().find(|v| !v.is_finite()) {
        Some(v) => Err(format!("`{v}` is not a finite number")),
        None => Ok(array),
    }
}

fn f32s<const N: usize>(text: &str) -> Result<[f32; N], String> {
    Ok(numbers::<N>(text)?.map(|v| v as f32))
}
script_value_type!(Vec2, "Vec2", decode = |t| Ok(Vec2::from_array(f32s::<2>(t)?)), encode = |v| literal::vec2(*v));
script_value_type!(Vec3, "Vec3", decode = |t| Ok(Vec3::from_array(f32s::<3>(t)?)), encode = |v| literal::vec3(*v));
script_value_type!(Vec4, "Vec4", decode = |t| Ok(Vec4::from_array(f32s::<4>(t)?)), encode = |v| literal::vec4(*v));
script_value_type!(DVec3, "DVec3", decode = |t| Ok(DVec3::from_array(numbers::<3>(t)?)), encode = |v| literal::dvec3(*v));
script_value_type!(Quat, "Quat", decode = |t| Ok(Quat::from_array(f32s::<4>(t)?)), encode = |v| literal::quat(*v));
script_value_type!(Mat4, "Mat4", decode = |t| Ok(Mat4::from_cols_array(&f32s::<16>(t)?)), encode = |v| literal::mat4(*v));


/// The literal text of each math value, for compilers emitting constants.
pub mod literal {
    use glam::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4};

    fn json(values: &[f64]) -> String {
        serde_json::to_string(values).expect("finite numbers serialize")
    }

    pub fn vec2(v: Vec2) -> String {
        json(&v.to_array().map(f64::from))
    }
    pub fn vec3(v: Vec3) -> String {
        json(&v.to_array().map(f64::from))
    }
    pub fn vec4(v: Vec4) -> String {
        json(&v.to_array().map(f64::from))
    }
    pub fn dvec3(v: DVec3) -> String {
        json(&v.to_array())
    }
    pub fn quat(q: Quat) -> String {
        json(&q.to_array().map(f64::from))
    }
    pub fn mat4(m: Mat4) -> String {
        json(&m.to_cols_array().map(f64::from))
    }
}

// `==` and string conversion in scripts. Floating-point equality is exact,
// as for `float`; use the types' `approx_eq` natives for tolerance.
script_value_ops!(Vec2, "Vec2", eq = |a, b| a == b, display = |v| v.to_string());
script_value_ops!(Vec3, "Vec3", eq = |a, b| a == b, display = |v| v.to_string());
script_value_ops!(Vec4, "Vec4", eq = |a, b| a == b, display = |v| v.to_string());
script_value_ops!(DVec3, "DVec3", eq = |a, b| a == b, display = |v| v.to_string());
script_value_ops!(Quat, "Quat", eq = |a, b| a == b, display = |v| v.to_string());
script_value_ops!(Mat4, "Mat4", eq = |a, b| a == b, display = |v| v.to_string());
