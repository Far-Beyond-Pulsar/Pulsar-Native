//! Execution semantics shared by every backend.
//!
//! The interpreter ([`crate::interp`]) and code generated from a module
//! (`pulsar_script_codegen`) both run on these functions, so the arithmetic
//! policy (wrapping or checked integers, division by zero, float-to-int
//! saturation), the string conversions, the native-call contract (panics
//! become errors, results must match the declared type) and the error kinds
//! cannot drift between them.

use crate::error::{ScriptError, ScriptErrorKind};
use crate::module::{BinOp, UnOp};
use crate::native::{Host, NativeFn};
use crate::value::Value;

/// Call `native` with `args` under the engine's contract: a panicking native
/// fails the call instead of unwinding into the game loop, the result must
/// have the declared type, and an error raised by the native is attributed
/// to it by name. `inout` parameters are modified in `args`; writing them
/// back to the caller's registers is the caller's job.
pub fn call_native(native: &NativeFn, host: &mut Host<'_>, args: &mut [Value]) -> Result<Value, ScriptErrorKind> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native.call(host, args)))
        .unwrap_or_else(|panic| Err(ScriptError::native(panic_message(&*panic))));
    match result {
        Ok(value) if value.fits(&native.sig.ret) => Ok(value),
        Ok(value) => Err(ScriptErrorKind::Native {
            name: native.name.clone(),
            message: format!("returned {}, declared {}", value.kind(), native.sig.ret),
        }),
        Err(err) => Err(match err.kind {
            ScriptErrorKind::Native { message, .. } => ScriptErrorKind::Native { name: native.name.clone(), message },
            other => other,
        }),
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    let message = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into());
    format!("panicked: {message}")
}

fn overflow(op: &str) -> ScriptErrorKind {
    ScriptErrorKind::Overflow { op: op.to_owned() }
}

pub fn unary(op: UnOp, value: &Value, checked: bool) -> Result<Value, ScriptErrorKind> {
    Ok(match (op, value) {
        (UnOp::Neg, Value::Int(i)) if checked => Value::Int(i.checked_neg().ok_or_else(|| overflow("Neg"))?),
        (UnOp::Neg, Value::Int(i)) => Value::Int(i.wrapping_neg()),
        (UnOp::Neg, Value::Float(f)) => Value::Float(-f),
        (UnOp::Not, Value::Bool(b)) => Value::Bool(!b),
        (UnOp::IntToFloat, Value::Int(i)) => Value::Float(*i as f64),
        // `i64::MAX as f64` rounds up to 2^63, which is already out of range.
        (UnOp::FloatToInt, Value::Float(f)) if checked && !(f.is_finite() && *f >= -(2f64.powi(63)) && *f < 2f64.powi(63)) => {
            return Err(overflow("FloatToInt"));
        }
        // `as` saturates and maps NaN to 0.
        (UnOp::FloatToInt, Value::Float(f)) => Value::Int(*f as i64),
        (UnOp::ToStr, v) => Value::Str(display(v).into()),
        // The verifier rules out every other combination.
        _ => unreachable!("unverified unary operand"),
    })
}

pub fn display(value: &Value) -> String {
    match value {
        Value::Unit => "()".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Str(s) => s.to_string(),
        Value::Entity(e) => e.to_string(),
        Value::Component(c) => format!("{}({})", pulsar_scenedb::component::type_name(c.component), c.entity),
        Value::Object(o) => o.type_name().to_owned(),
    }
}

pub fn binary(op: BinOp, a: &Value, b: &Value, checked: bool) -> Result<Value, ScriptErrorKind> {
    use Value::{Bool, Float, Int, Str};
    if checked {
        if let (Int(x), Int(y)) = (a, b) {
            let result = match op {
                BinOp::Add => Some(x.checked_add(*y)),
                BinOp::Sub => Some(x.checked_sub(*y)),
                BinOp::Mul => Some(x.checked_mul(*y)),
                BinOp::Div | BinOp::Rem if *y == 0 => return Err(ScriptErrorKind::DivideByZero),
                BinOp::Div => Some(x.checked_div(*y)),
                BinOp::Rem => Some(x.checked_rem(*y)),
                _ => None,
            };
            if let Some(result) = result {
                return result.map(Int).ok_or_else(|| overflow(&format!("{op:?}")));
            }
        }
    }
    Ok(match (op, a, b) {
        (BinOp::Add, Int(a), Int(b)) => Int(a.wrapping_add(*b)),
        (BinOp::Sub, Int(a), Int(b)) => Int(a.wrapping_sub(*b)),
        (BinOp::Mul, Int(a), Int(b)) => Int(a.wrapping_mul(*b)),
        (BinOp::Div | BinOp::Rem, Int(_), Int(0)) => return Err(ScriptErrorKind::DivideByZero),
        (BinOp::Div, Int(a), Int(b)) => Int(a.wrapping_div(*b)),
        (BinOp::Rem, Int(a), Int(b)) => Int(a.wrapping_rem(*b)),
        (BinOp::Add, Float(a), Float(b)) => Float(a + b),
        (BinOp::Sub, Float(a), Float(b)) => Float(a - b),
        (BinOp::Mul, Float(a), Float(b)) => Float(a * b),
        (BinOp::Div, Float(a), Float(b)) => Float(a / b),
        (BinOp::Rem, Float(a), Float(b)) => Float(a % b),
        (BinOp::Add, Str(a), Str(b)) => Str(format!("{a}{b}").into()),
        (BinOp::Eq, a, b) => Bool(a == b),
        (BinOp::Ne, a, b) => Bool(a != b),
        (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, a, b) => {
            let ordering = match (a, b) {
                (Int(a), Int(b)) => a.partial_cmp(b),
                (Float(a), Float(b)) => a.partial_cmp(b),
                (Str(a), Str(b)) => a.partial_cmp(b),
                _ => unreachable!("unverified comparison operands"),
            };
            // NaN compares false, like IEEE.
            Bool(ordering.is_some_and(|o| match op {
                BinOp::Lt => o.is_lt(),
                BinOp::Le => o.is_le(),
                BinOp::Gt => o.is_gt(),
                _ => o.is_ge(),
            }))
        }
        (BinOp::And, Bool(a), Bool(b)) => Bool(*a && *b),
        (BinOp::Or, Bool(a), Bool(b)) => Bool(*a || *b),
        _ => unreachable!("unverified binary operands"),
    })
}
