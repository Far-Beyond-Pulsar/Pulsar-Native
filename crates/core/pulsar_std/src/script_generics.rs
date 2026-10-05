//! The generic `array_*` nodes as script natives.
//!
//! `#[blueprint]` cannot register a function generic over its element type
//! (a native has one concrete signature), so these are written against the
//! script VM's untyped lists and registered as [`GenericNative`]s: a module
//! imports `std::array_push@int` with the signature at `int`, and linking
//! instantiates the template at that type. Behaviour is that of the nodes'
//! Rust functions (`array_get` of a bad index is "absent", not an error),
//! and the results that were `Option` or tuples are the tuples
//! `ScriptValue` gives them, with `outputs` naming their pins.

use std::cmp::Ordering;

use pulsar_script_vm::{GenericNative, GenericProvider, Param, ScriptError, Signature, Type, TypeRegistry, Value};

fn list(element: &Type) -> Type {
    Type::list(element.clone())
}

/// `(present, value)`: the type of an `Option<T>` result.
fn option(element: &Type) -> Type {
    Type::Tuple(vec![Type::Bool, element.clone()])
}

fn items(value: &Value) -> Result<&[Value], ScriptError> {
    value.as_list().ok_or_else(|| ScriptError::native(format!("expected a list, got {}", value.kind())))
}

fn int(value: &Value) -> Result<i64, ScriptError> {
    value.as_int().ok_or_else(|| ScriptError::native(format!("expected an int, got {}", value.kind())))
}

/// The element a missing value stands in for.
fn default_of(element: &Type) -> Value {
    TypeRegistry::global().default_value(element).unwrap_or(Value::Unit)
}

fn present(element: &Type, found: Option<&Value>) -> Value {
    match found {
        Some(value) => Value::tuple(vec![Value::Bool(true), value.clone()]),
        None => Value::tuple(vec![Value::Bool(false), default_of(element)]),
    }
}

/// The order `array_sort` uses: numbers, strings and bools sort; other
/// element types have no order.
fn compare(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Float(a), Value::Float(b)) => Some(a.total_cmp(b)),
        (Value::Str(a), Value::Str(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

fn sig(params: &[Type], ret: Type) -> Signature {
    Signature::new(params.iter().cloned().map(Param::new), ret)
}

fn natives() -> Vec<GenericNative> {
    let pure = |native: GenericNative| native.side_effect_free().attr("category", "Array");
    let plain = |native: GenericNative| native.attr("category", "Array");
    vec![
        pure(GenericNative::new("std::array_new", &[], |t| sig(&[], list(t)), |_, _, _| Ok(Value::list(Vec::new()))))
            .doc("An empty array."),
        plain(GenericNative::new(
            "std::array_push",
            &["array", "item"],
            |t| sig(&[list(t), t.clone()], list(t)),
            |_, _, args| {
                let mut array = items(&args[0])?.to_vec();
                array.push(args[1].clone());
                Ok(Value::list(array))
            },
        )),
        plain(GenericNative::new(
            "std::array_pop",
            &["array"],
            |t| sig(&[list(t)], Type::Tuple(vec![list(t), option(t)])),
            |t, _, args| {
                let mut array = items(&args[0])?.to_vec();
                let popped = array.pop();
                Ok(Value::tuple(vec![Value::list(array), present(t, popped.as_ref())]))
            },
        ))
        .attr("outputs", "array,popped"),
        plain(GenericNative::new(
            "std::array_set",
            &["array", "index", "value"],
            |t| sig(&[list(t), Type::Int, t.clone()], list(t)),
            |_, _, args| {
                let mut array = items(&args[0])?.to_vec();
                // A bad index leaves the array as it is.
                if let Some(slot) = usize::try_from(int(&args[1])?).ok().and_then(|i| array.get_mut(i)) {
                    *slot = args[2].clone();
                }
                Ok(Value::list(array))
            },
        )),
        plain(GenericNative::new("std::array_clear", &["array"], |t| sig(&[list(t)], list(t)), |_, _, args| {
            items(&args[0])?;
            Ok(Value::list(Vec::new()))
        })),
        pure(GenericNative::new(
            "std::array_get",
            &["array", "index"],
            |t| sig(&[list(t), Type::Int], option(t)),
            |t, _, args| {
                let array = items(&args[0])?;
                Ok(present(t, usize::try_from(int(&args[1])?).ok().and_then(|i| array.get(i))))
            },
        ))
        .attr("outputs", "present,value"),
        pure(GenericNative::new("std::array_first", &["array"], |t| sig(&[list(t)], option(t)), |t, _, args| {
            Ok(present(t, items(&args[0])?.first()))
        }))
        .attr("outputs", "present,value"),
        pure(GenericNative::new("std::array_last", &["array"], |t| sig(&[list(t)], option(t)), |t, _, args| {
            Ok(present(t, items(&args[0])?.last()))
        }))
        .attr("outputs", "present,value"),
        pure(GenericNative::new("std::array_length", &["array"], |t| sig(&[list(t)], Type::Int), |_, _, args| {
            Ok(Value::Int(items(&args[0])?.len() as i64))
        })),
        pure(GenericNative::new("std::array_is_empty", &["array"], |t| sig(&[list(t)], Type::Bool), |_, _, args| {
            Ok(Value::Bool(items(&args[0])?.is_empty()))
        })),
        pure(GenericNative::new(
            "std::array_contains",
            &["array", "item"],
            |t| sig(&[list(t), t.clone()], Type::Bool),
            |_, _, args| Ok(Value::Bool(items(&args[0])?.contains(&args[1]))),
        )),
        pure(GenericNative::new(
            "std::array_slice",
            &["array", "start", "end"],
            |t| sig(&[list(t), Type::Int, Type::Int], list(t)),
            |_, _, args| {
                let array = items(&args[0])?;
                let (start, end) = (int(&args[1])?, int(&args[2])?);
                // An invalid range is an empty slice, as in the node.
                let range = usize::try_from(start)
                    .ok()
                    .zip(usize::try_from(end).ok())
                    .filter(|(start, end)| *start < array.len() && *end <= array.len() && start <= end);
                Ok(Value::list(range.map_or_else(Vec::new, |(start, end)| array[start..end].to_vec())))
            },
        )),
        plain(GenericNative::new("std::array_reverse", &["array"], |t| sig(&[list(t)], list(t)), |_, _, args| {
            let mut array = items(&args[0])?.to_vec();
            array.reverse();
            Ok(Value::list(array))
        })),
        plain(GenericNative::new("std::array_sort", &["array"], |t| sig(&[list(t)], list(t)), |_, _, args| {
            let mut array = items(&args[0])?.to_vec();
            if array.len() > 1 && compare(&array[0], &array[0]).is_none() {
                return Err(ScriptError::native(format!("{} elements have no order", array[0].kind())));
            }
            array.sort_by(|a, b| compare(a, b).unwrap_or(Ordering::Equal));
            Ok(Value::list(array))
        })),
        plain(GenericNative::new(
            "std::array_concat",
            &["a", "b"],
            |t| sig(&[list(t), list(t)], list(t)),
            |_, _, args| {
                let mut array = items(&args[0])?.to_vec();
                array.extend_from_slice(items(&args[1])?);
                Ok(Value::list(array))
            },
        )),
    ]
}

pulsar_script_vm::__private::inventory::submit! {
    GenericProvider { natives }
}
