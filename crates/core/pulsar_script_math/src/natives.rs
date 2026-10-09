//! Constructor, field, arithmetic, equality and display natives.

use glam::{DVec3, Mat4, Quat, Vec2, Vec3, Vec4};
use pulsar_script_vm::{NativeBuilder, NativeFn, NativeProvider, Obj, Type};

pulsar_script_vm::__private::inventory::submit! {
    NativeProvider { natives }
}

/// A side-effect-free, deterministic native on a value type.
fn pure_method(name: &str, receiver: &str, params: &[&str]) -> NativeBuilder {
    NativeFn::builder(name)
        .pure()
        .attr("category", receiver)
        .params(params.iter().copied())
        .method_of(Type::object(receiver))
}

/// `T::new`, per-component getters and value-semantic setters (`with_x`
/// returns a changed copy), arithmetic, equality and display for a vector.
macro_rules! vec_natives {
    ($out:ident, $ty:ident, $scalar:ty, [$($c:ident),+]) => {{
        let name = stringify!($ty);
        $out.push(
            NativeFn::builder(format!("{name}::new"))
                .pure()
                .attr("category", name)
                .params([$(stringify!($c)),+])
                .build(|$($c: f64),+| Obj(<$ty>::new($($c as $scalar),+))),
        );
        $(
            $out.push(pure_method(&format!("{name}::{}", stringify!($c)), name, &["v"])
                .build(|v: Obj<$ty>| v.0.$c as f64));
            $out.push(pure_method(&format!("{name}::with_{}", stringify!($c)), name, &["v", "value"])
                .build(|v: Obj<$ty>, value: f64| { let mut v = v.0; v.$c = value as $scalar; Obj(v) }));
        )+
        $out.push(pure_method(&format!("{name}::add"), name, &["a", "b"]).build(|a: Obj<$ty>, b: Obj<$ty>| Obj(a.0 + b.0)));
        $out.push(pure_method(&format!("{name}::sub"), name, &["a", "b"]).build(|a: Obj<$ty>, b: Obj<$ty>| Obj(a.0 - b.0)));
        $out.push(pure_method(&format!("{name}::scale"), name, &["v", "k"]).build(|v: Obj<$ty>, k: f64| Obj(v.0 * k as $scalar)));
        $out.push(pure_method(&format!("{name}::dot"), name, &["a", "b"]).build(|a: Obj<$ty>, b: Obj<$ty>| a.0.dot(b.0) as f64));
        $out.push(pure_method(&format!("{name}::eq"), name, &["a", "b"]).build(|a: Obj<$ty>, b: Obj<$ty>| a.0 == b.0));
        $out.push(pure_method(&format!("{name}::approx_eq"), name, &["a", "b", "epsilon"])
            .build(|a: Obj<$ty>, b: Obj<$ty>, eps: f64| (a.0 - b.0).abs().max_element() as f64 <= eps));
        $out.push(pure_method(&format!("{name}::to_string"), name, &["v"]).build(|v: Obj<$ty>| format!("{}", v.0)));
    }};
}

/// Geometric operations every float vector has.
macro_rules! float_vec_natives {
    ($out:ident, $ty:ident, $scalar:ty) => {{
        let name = stringify!($ty);
        $out.push(
            pure_method(&format!("{name}::length"), name, &["v"])
                .build(|v: Obj<$ty>| v.0.length() as f64),
        );
        $out.push(
            pure_method(&format!("{name}::distance"), name, &["a", "b"])
                .build(|a: Obj<$ty>, b: Obj<$ty>| a.0.distance(b.0) as f64),
        );
        // A zero vector normalizes to zero rather than NaN.
        $out.push(
            pure_method(&format!("{name}::normalize"), name, &["v"])
                .build(|v: Obj<$ty>| Obj(v.0.normalize_or_zero())),
        );
        $out.push(
            pure_method(&format!("{name}::lerp"), name, &["a", "b", "t"])
                .build(|a: Obj<$ty>, b: Obj<$ty>, t: f64| Obj(a.0.lerp(b.0, t as $scalar))),
        );
    }};
}

fn natives() -> Vec<NativeFn> {
    let mut out = Vec::new();
    vec_natives!(out, Vec2, f32, [x, y]);
    vec_natives!(out, Vec3, f32, [x, y, z]);
    vec_natives!(out, Vec4, f32, [x, y, z, w]);
    vec_natives!(out, DVec3, f64, [x, y, z]);
    float_vec_natives!(out, Vec2, f32);
    float_vec_natives!(out, Vec3, f32);
    float_vec_natives!(out, Vec4, f32);
    float_vec_natives!(out, DVec3, f64);
    out.push(
        pure_method("Vec3::cross", "Vec3", &["a", "b"])
            .build(|a: Obj<Vec3>, b: Obj<Vec3>| Obj(a.0.cross(b.0))),
    );
    quat_natives(&mut out);
    mat4_natives(&mut out);
    out
}

fn quat_natives(out: &mut Vec<NativeFn>) {
    let name = "Quat";
    out.push(
        NativeFn::builder("Quat::new")
            .pure()
            .attr("category", name)
            .params(["x", "y", "z", "w"])
            .build(|x: f64, y: f64, z: f64, w: f64| {
                Obj(Quat::from_xyzw(x as f32, y as f32, z as f32, w as f32))
            }),
    );
    out.push(
        NativeFn::builder("Quat::identity")
            .pure()
            .attr("category", name)
            .build(|| Obj(Quat::IDENTITY)),
    );
    out.push(
        NativeFn::builder("Quat::from_axis_angle")
            .pure()
            .attr("category", name)
            .params(["axis", "radians"])
            // A zero axis is not a rotation; fall back to the Y axis.
            .build(|axis: Obj<Vec3>, radians: f64| {
                Obj(Quat::from_axis_angle(
                    axis.0.try_normalize().unwrap_or(Vec3::Y),
                    radians as f32,
                ))
            }),
    );
    out.push(pure_method("Quat::x", name, &["q"]).build(|q: Obj<Quat>| q.0.x as f64));
    out.push(pure_method("Quat::y", name, &["q"]).build(|q: Obj<Quat>| q.0.y as f64));
    out.push(pure_method("Quat::z", name, &["q"]).build(|q: Obj<Quat>| q.0.z as f64));
    out.push(pure_method("Quat::w", name, &["q"]).build(|q: Obj<Quat>| q.0.w as f64));
    out.push(
        pure_method("Quat::mul", name, &["a", "b"])
            .build(|a: Obj<Quat>, b: Obj<Quat>| Obj(a.0 * b.0)),
    );
    out.push(
        pure_method("Quat::normalize", name, &["q"]).build(|q: Obj<Quat>| {
            Obj(if q.0.length_squared() > 0.0 {
                q.0.normalize()
            } else {
                Quat::IDENTITY
            })
        }),
    );
    out.push(pure_method("Quat::inverse", name, &["q"]).build(|q: Obj<Quat>| Obj(q.0.inverse())));
    out.push(
        pure_method("Quat::rotate", name, &["q", "v"])
            .build(|q: Obj<Quat>, v: Obj<Vec3>| Obj(q.0 * v.0)),
    );
    out.push(
        pure_method("Quat::eq", name, &["a", "b"]).build(|a: Obj<Quat>, b: Obj<Quat>| a.0 == b.0),
    );
    // `q` and `-q` are the same rotation, so compare by |dot|.
    out.push(
        pure_method("Quat::same_rotation", name, &["a", "b", "epsilon"])
            .build(|a: Obj<Quat>, b: Obj<Quat>, eps: f64| 1.0 - (a.0.dot(b.0).abs() as f64) <= eps),
    );
    out.push(pure_method("Quat::to_string", name, &["q"]).build(|q: Obj<Quat>| format!("{}", q.0)));
}

fn mat4_natives(out: &mut Vec<NativeFn>) {
    let name = "Mat4";
    out.push(
        NativeFn::builder("Mat4::identity")
            .pure()
            .attr("category", name)
            .build(|| Obj(Mat4::IDENTITY)),
    );
    out.push(
        NativeFn::builder("Mat4::from_translation")
            .pure()
            .attr("category", name)
            .params(["translation"])
            .build(|t: Obj<Vec3>| Obj(Mat4::from_translation(t.0))),
    );
    out.push(
        NativeFn::builder("Mat4::from_scale_rotation_translation")
            .pure()
            .attr("category", name)
            .params(["scale", "rotation", "translation"])
            .build(|s: Obj<Vec3>, r: Obj<Quat>, t: Obj<Vec3>| {
                Obj(Mat4::from_scale_rotation_translation(
                    s.0,
                    r.0.normalize(),
                    t.0,
                ))
            }),
    );
    out.push(
        pure_method("Mat4::mul", name, &["a", "b"])
            .build(|a: Obj<Mat4>, b: Obj<Mat4>| Obj(a.0 * b.0)),
    );
    out.push(
        pure_method("Mat4::inverse", name, &["m"]).build(|m: Obj<Mat4>| {
            // A singular matrix has no inverse; fail rather than return NaNs.
            if m.0.determinant().abs() > f32::EPSILON {
                Ok(Obj(m.0.inverse()))
            } else {
                Err("matrix is not invertible".to_owned())
            }
        }),
    );
    out.push(
        pure_method("Mat4::transform_point", name, &["m", "point"])
            .build(|m: Obj<Mat4>, p: Obj<Vec3>| Obj(m.0.transform_point3(p.0))),
    );
    // Column-major: `col` selects a column, `row` the element within it.
    out.push(
        pure_method("Mat4::element", name, &["m", "col", "row"]).build(
            |m: Obj<Mat4>, col: i64, row: i64| match (usize::try_from(col), usize::try_from(row)) {
                (Ok(c @ 0..=3), Ok(r @ 0..=3)) => Ok(m.0.col(c)[r] as f64),
                _ => Err(format!("element ({col}, {row}) is outside a 4x4 matrix")),
            },
        ),
    );
    out.push(
        pure_method("Mat4::eq", name, &["a", "b"]).build(|a: Obj<Mat4>, b: Obj<Mat4>| a.0 == b.0),
    );
    out.push(
        pure_method("Mat4::approx_eq", name, &["a", "b", "epsilon"])
            .build(|a: Obj<Mat4>, b: Obj<Mat4>, eps: f64| a.0.abs_diff_eq(b.0, eps as f32)),
    );
    out.push(pure_method("Mat4::to_string", name, &["m"]).build(|m: Obj<Mat4>| format!("{}", m.0)));
}
