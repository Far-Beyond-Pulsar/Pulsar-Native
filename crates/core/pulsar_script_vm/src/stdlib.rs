//! The standard natives every module can import: math, strings, logging
//! and entity basics. Arithmetic, comparison, logic and conversions are
//! instructions, not natives.

use std::sync::Arc;

use pulsar_reflection::MethodFlags;
use pulsar_scenedb::Entity;

use crate::native::{Host, NativeFn, NativeRegistry};

pub(crate) fn register(registry: &mut NativeRegistry) {
    let mut add = |native: NativeFn| {
        if let Err(err) = registry.register(native) {
            tracing::error!("script stdlib: {err}");
        }
    };
    let pure = |name: &str, category: &str| NativeFn::builder(name).pure().attr("category", category);

    // Float math.
    macro_rules! unary {
        ($($name:literal => $f:expr),* $(,)?) => {$(
            add(pure($name, "Math").params(["x"]).build($f));
        )*};
    }
    unary!(
        "math::sin" => |x: f64| x.sin(),
        "math::cos" => |x: f64| x.cos(),
        "math::tan" => |x: f64| x.tan(),
        "math::asin" => |x: f64| x.asin(),
        "math::acos" => |x: f64| x.acos(),
        "math::atan" => |x: f64| x.atan(),
        "math::sqrt" => |x: f64| x.sqrt(),
        "math::abs" => |x: f64| x.abs(),
        "math::floor" => |x: f64| x.floor(),
        "math::ceil" => |x: f64| x.ceil(),
        "math::round" => |x: f64| x.round(),
        "math::exp" => |x: f64| x.exp(),
        "math::ln" => |x: f64| x.ln(),
        "math::sign" => |x: f64| x.signum(),
        "math::to_radians" => |x: f64| x.to_radians(),
        "math::to_degrees" => |x: f64| x.to_degrees(),
    );
    add(pure("math::atan2", "Math").params(["y", "x"]).build(|y: f64, x: f64| y.atan2(x)));
    add(pure("math::pow", "Math").params(["base", "exp"]).build(|b: f64, e: f64| b.powf(e)));
    add(pure("math::min", "Math").params(["a", "b"]).build(|a: f64, b: f64| a.min(b)));
    add(pure("math::max", "Math").params(["a", "b"]).build(|a: f64, b: f64| a.max(b)));
    add(pure("math::clamp", "Math")
        .params(["x", "min", "max"])
        .build(|x: f64, lo: f64, hi: f64| x.max(lo).min(hi)));
    add(pure("math::lerp", "Math")
        .params(["a", "b", "t"])
        .build(|a: f64, b: f64, t: f64| a + (b - a) * t));
    add(pure("math::pi", "Math").build(|| std::f64::consts::PI));

    // Integer math.
    add(pure("int::abs", "Math").params(["x"]).build(|x: i64| x.wrapping_abs()));
    add(pure("int::min", "Math").params(["a", "b"]).build(|a: i64, b: i64| a.min(b)));
    add(pure("int::max", "Math").params(["a", "b"]).build(|a: i64, b: i64| a.max(b)));
    add(pure("int::clamp", "Math")
        .params(["x", "min", "max"])
        .build(|x: i64, lo: i64, hi: i64| x.max(lo).min(hi)));

    // Strings.
    add(pure("string::len", "String").params(["s"]).build(|s: Arc<str>| s.chars().count() as i64));
    add(pure("string::contains", "String")
        .params(["s", "pattern"])
        .build(|s: Arc<str>, p: Arc<str>| s.contains(&*p)));
    add(pure("string::to_upper", "String").params(["s"]).build(|s: Arc<str>| s.to_uppercase()));
    add(pure("string::to_lower", "String").params(["s"]).build(|s: Arc<str>| s.to_lowercase()));
    add(pure("string::trim", "String").params(["s"]).build(|s: Arc<str>| s.trim().to_owned()));
    add(pure("string::parse_int", "String")
        .params(["s"])
        .build(|s: Arc<str>| s.trim().parse::<i64>().map_err(|e| e.to_string())));
    add(pure("string::parse_float", "String")
        .params(["s"])
        .build(|s: Arc<str>| s.trim().parse::<f64>().map_err(|e| e.to_string())));

    // Logging. Side effects, so not pure.
    let log = |name: &str| NativeFn::builder(name).attr("category", "Debug").params(["message"]);
    add(log("log::info").build(|host: &mut Host<'_>, m: Arc<str>| {
        tracing::info!(target: "script", entity = ?host.entity, "{m}");
    }));
    add(log("log::warn").build(|host: &mut Host<'_>, m: Arc<str>| {
        tracing::warn!(target: "script", entity = ?host.entity, "{m}");
    }));
    add(log("log::error").build(|host: &mut Host<'_>, m: Arc<str>| {
        tracing::error!(target: "script", entity = ?host.entity, "{m}");
    }));

    // Entities.
    let reads_world = MethodFlags { side_effect_free: true, deterministic: false };
    add(NativeFn::builder("entity::is_alive")
        .flags(reads_world)
        .attr("category", "Entity")
        .params(["entity"])
        .build(|host: &mut Host<'_>, e: Entity| host.world.is_alive(e)));
    add(pure("entity::none", "Entity").build(|| Entity::DANGLING));
    add(NativeFn::builder("entity::spawn")
        .attr("category", "Entity")
        .build(|host: &mut Host<'_>| host.world.spawn()));
    add(NativeFn::builder("entity::despawn")
        .attr("category", "Entity")
        .params(["entity"])
        .build(|host: &mut Host<'_>, e: Entity| {
            if host.world.is_alive(e) {
                host.world.despawn(e);
            }
        }));
}
