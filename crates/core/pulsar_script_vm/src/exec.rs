//! Execution semantics shared by every backend.
//!
//! The interpreter ([`crate::interp`]) and code generated from a module
//! (`pulsar_script_codegen`) both run on these functions, so the arithmetic
//! policy (wrapping or checked integers, division by zero, float-to-int
//! saturation), the string conversions, the native-call contract (panics
//! become errors, results must match the declared type) and the error kinds
//! cannot drift between them.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::error::{ScriptError, ScriptErrorKind};
use crate::module::{BinOp, CollOp, UnOp};
use crate::native::{Host, NativeFn};
use crate::value::{MapKey, Value};

/// Call `native` with `args` under the engine's contract: a panicking native
/// fails the call instead of unwinding into the game loop, the result must
/// have the declared type, and an error raised by the native is attributed
/// to it by name. `inout` parameters are modified in `args`; writing them
/// back to the caller's registers is the caller's job.
pub fn call_native(
    native: &NativeFn,
    host: &mut Host<'_>,
    args: &mut [Value],
) -> Result<Value, ScriptErrorKind> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| native.call(host, args)))
        .unwrap_or_else(|panic| Err(ScriptError::native(panic_message(&*panic))));
    match result {
        Ok(value) if value.fits(&native.sig.ret) => Ok(value),
        Ok(value) => Err(ScriptErrorKind::Native {
            name: native.name.clone(),
            message: format!("returned {}, declared {}", value.kind(), native.sig.ret),
        }),
        Err(err) => Err(match err.kind {
            ScriptErrorKind::Native { message, .. } => ScriptErrorKind::Native {
                name: native.name.clone(),
                message,
            },
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
        (UnOp::Neg, Value::Int(i)) if checked => {
            Value::Int(i.checked_neg().ok_or_else(|| overflow("Neg"))?)
        }
        (UnOp::Neg, Value::Int(i)) => Value::Int(i.wrapping_neg()),
        (UnOp::Neg, Value::Float(f)) => Value::Float(-f),
        (UnOp::Not, Value::Bool(b)) => Value::Bool(!b),
        (UnOp::IntToFloat, Value::Int(i)) => Value::Float(*i as f64),
        (UnOp::IntToI32Checked, Value::Int(i)) if *i < i32::MIN as i64 || *i > i32::MAX as i64 => {
            return Err(overflow("IntToI32Checked"));
        }
        (UnOp::IntToI32Checked, Value::Int(i)) => Value::Int(*i),
        // `i64::MAX as f64` rounds up to 2^63, which is already out of range.
        (UnOp::FloatToInt, Value::Float(f))
            if checked && !(f.is_finite() && *f >= -(2f64.powi(63)) && *f < 2f64.powi(63)) =>
        {
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
        Value::Component(c) => format!(
            "{}@{}",
            pulsar_scenedb::component::type_name(c.component),
            c.entity
        ),
        Value::Object(o) => crate::types::TypeRegistry::global().display_object(o),
        Value::List(items) => format!(
            "[{}]",
            items.iter().map(display).collect::<Vec<_>>().join(", ")
        ),
        Value::Map(entries) => format!(
            "{{{}}}",
            entries
                .iter()
                .map(|(k, v)| format!("{}: {}", display(&k.to_value()), display(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Tuple(items) => format!(
            "({})",
            items.iter().map(display).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// The hot scalar cases of [`binary`], small enough to inline into the
/// interpreter loop: the loop-counter and float arithmetic and the numeric
/// comparisons scripts spend most instructions on. `None` means "take the
/// general path" (checked integer arithmetic, division, strings, ..); a
/// `Some` is exactly what `binary` would return.
#[inline(always)]
pub fn binary_scalar(op: BinOp, a: &Value, b: &Value, checked: bool) -> Option<Value> {
    use Value::{Bool, Float, Int};
    Some(match (a, b) {
        (Int(a), Int(b)) => match op {
            BinOp::Add if !checked => Int(a.wrapping_add(*b)),
            BinOp::Sub if !checked => Int(a.wrapping_sub(*b)),
            BinOp::Mul if !checked => Int(a.wrapping_mul(*b)),
            BinOp::Eq => Bool(a == b),
            BinOp::Ne => Bool(a != b),
            BinOp::Lt => Bool(a < b),
            BinOp::Le => Bool(a <= b),
            BinOp::Gt => Bool(a > b),
            BinOp::Ge => Bool(a >= b),
            _ => return None,
        },
        (Float(a), Float(b)) => match op {
            BinOp::Add => Float(a + b),
            BinOp::Sub => Float(a - b),
            BinOp::Mul => Float(a * b),
            BinOp::Lt => Bool(a < b),
            BinOp::Le => Bool(a <= b),
            BinOp::Gt => Bool(a > b),
            BinOp::Ge => Bool(a >= b),
            _ => return None,
        },
        _ => return None,
    })
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

/// Run a collection operation over `args` (verified for `op`). The
/// arguments are the caller's copies; the first is consumed, so a
/// collection that the caller moved in (rather than cloned) is edited in
/// place, and a shared one is copied once, on this write.
pub fn collection(op: CollOp, args: &mut [Value]) -> Result<Value, ScriptErrorKind> {
    fn take(value: &mut Value) -> Value {
        std::mem::replace(value, Value::Unit)
    }
    fn list(value: &mut Value) -> Arc<Vec<Value>> {
        match take(value) {
            Value::List(items) => items,
            _ => unreachable!("unverified list operand"),
        }
    }
    fn map(value: &mut Value) -> Arc<BTreeMap<MapKey, Value>> {
        match take(value) {
            Value::Map(entries) => entries,
            _ => unreachable!("unverified map operand"),
        }
    }
    fn key(value: &Value) -> MapKey {
        MapKey::from_value(value).expect("unverified map key")
    }
    fn index(value: &Value) -> i64 {
        value.as_int().expect("unverified index")
    }
    let out_of_bounds = |index: i64, len: usize| ScriptErrorKind::IndexOutOfBounds { index, len };
    // `index` as a position in `0..len`.
    let position = |at: i64, len: usize| {
        usize::try_from(at)
            .ok()
            .filter(|p| *p < len)
            .ok_or_else(|| out_of_bounds(at, len))
    };

    Ok(match op {
        CollOp::MakeList => Value::list(args.iter_mut().map(take).collect()),
        CollOp::ListLen => match &args[0] {
            Value::List(items) => Value::Int(items.len() as i64),
            _ => unreachable!("unverified list operand"),
        },
        CollOp::ListGet => {
            let items = list(&mut args[0]);
            items[position(index(&args[1]), items.len())?].clone()
        }
        CollOp::ListSet => {
            let mut items = list(&mut args[0]);
            let at = position(index(&args[1]), items.len())?;
            Arc::make_mut(&mut items)[at] = take(&mut args[2]);
            Value::List(items)
        }
        CollOp::ListPush => {
            let mut items = list(&mut args[0]);
            Arc::make_mut(&mut items).push(take(&mut args[1]));
            Value::List(items)
        }
        CollOp::ListInsert => {
            let mut items = list(&mut args[0]);
            let at = index(&args[1]);
            let at = usize::try_from(at)
                .ok()
                .filter(|p| *p <= items.len())
                .ok_or_else(|| out_of_bounds(at, items.len()))?;
            Arc::make_mut(&mut items).insert(at, take(&mut args[2]));
            Value::List(items)
        }
        CollOp::ListRemove => {
            let mut items = list(&mut args[0]);
            let at = position(index(&args[1]), items.len())?;
            Arc::make_mut(&mut items).remove(at);
            Value::List(items)
        }
        CollOp::MakeMap => {
            let mut entries = BTreeMap::new();
            for pair in args.chunks_mut(2) {
                entries.insert(key(&pair[0]), take(&mut pair[1]));
            }
            Value::Map(Arc::new(entries))
        }
        CollOp::MapLen => Value::Int(map(&mut args[0]).len() as i64),
        CollOp::MapGet => {
            let entries = map(&mut args[0]);
            let key = key(&args[1]);
            match entries.get(&key) {
                Some(value) => value.clone(),
                None => {
                    return Err(ScriptErrorKind::KeyNotFound {
                        key: display(&key.to_value()),
                    });
                }
            }
        }
        CollOp::MapHas => Value::Bool(map(&mut args[0]).contains_key(&key(&args[1]))),
        CollOp::MapSet => {
            let mut entries = map(&mut args[0]);
            Arc::make_mut(&mut entries).insert(key(&args[1]), take(&mut args[2]));
            Value::Map(entries)
        }
        CollOp::MapRemove => {
            let mut entries = map(&mut args[0]);
            let key = key(&args[1]);
            if entries.contains_key(&key) {
                Arc::make_mut(&mut entries).remove(&key);
            }
            Value::Map(entries)
        }
        CollOp::MapKeys => Value::list(map(&mut args[0]).keys().map(MapKey::to_value).collect()),
        CollOp::MakeTuple => Value::tuple(args.iter_mut().map(take).collect()),
        CollOp::TupleGet(at) => match &args[0] {
            Value::Tuple(items) => items[at as usize].clone(),
            _ => unreachable!("unverified tuple operand"),
        },
    })
}

/// Whether the native that just ran asked for the call to suspend (a
/// latent native: see [`crate::latent`]). Generated code checks this after
/// every native call, as the interpreter does.
pub fn latent_requested(host: &Host<'_>) -> bool {
    host.latent
        .as_deref()
        .is_some_and(crate::latent::Latent::suspend_requested)
}
