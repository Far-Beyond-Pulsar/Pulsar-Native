//! Converting level-file JSON into script values.

use pulsar_scenedb::Entity;
use pulsar_script_vm::{Type, TypeRegistry, Value};

/// Convert a JSON value to a script value of type `ty`, where no entity
/// can be named (an entity or component reference is an error unless it is
/// `null`).
pub fn value_from_json(json: &serde_json::Value, ty: &Type) -> Result<Value, String> {
    value_from_json_with(json, ty, &|_| None)
}

/// Convert a JSON value (from a level file) to a script value of type `ty`.
///
/// An entity is its StableId string, resolved with `entity_for`, or `null`
/// for none. A component reference is
/// `{ "entity": "<stable id>", "component": "Health" }`; the component must
/// be the variable's component type, and whether the entity has it is
/// checked when the reference is used, like any other reference.
pub fn value_from_json_with(
    json: &serde_json::Value,
    ty: &Type,
    entity_for: &dyn Fn(&str) -> Option<Entity>,
) -> Result<Value, String> {
    use serde_json::Value as J;
    let entity = |json: &J| match json {
        J::Null => Ok(Entity::DANGLING),
        J::String(stable_id) => entity_for(stable_id)
            .ok_or_else(|| format!("no object has the stable id `{stable_id}`")),
        other => Err(format!(
            "an entity is its stable id string or null, got {other}"
        )),
    };
    match (ty, json) {
        (Type::Entity, json) => entity(json).map(Value::Entity),
        (Type::Component(name), json) => {
            let binding = TypeRegistry::global()
                .component(name)
                .ok_or_else(|| format!("`{name}` is not a registered component"))?;
            let target = match json {
                J::Null => Entity::DANGLING,
                J::Object(fields) => {
                    if let Some(component) = fields.get("component") {
                        if component.as_str() != Some(name.as_str()) {
                            return Err(format!("expected a `{name}` reference, got {component}"));
                        }
                    }
                    entity(fields.get("entity").unwrap_or(&J::Null))?
                }
                other => {
                    return Err(format!(
                        "a component reference is {{\"entity\": \"<stable id>\", \"component\": \"{name}\"}} or null, got {other}"
                    ))
                }
            };
            Ok(Value::Component(pulsar_scenedb::ComponentRef::new(
                target,
                binding.component_id(),
            )))
        }
        (Type::Bool, J::Bool(b)) => Ok(Value::Bool(*b)),
        (Type::Int, J::Number(n)) => n
            .as_i64()
            .map(Value::Int)
            .ok_or_else(|| format!("{n} is not an integer")),
        (Type::Float, J::Number(n)) => n
            .as_f64()
            .map(Value::Float)
            .ok_or_else(|| format!("{n} is not a number")),
        // NaN and the infinities have no JSON number; a save writes them as
        // the strings `NaN`, `inf` and `-inf` (see `state::value_to_json`).
        (Type::Float, J::String(s)) => s
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|f| !f.is_finite())
            .map(Value::Float)
            .ok_or_else(|| format!("\"{s}\" is not a number")),
        (Type::Str, J::String(s)) => Ok(Value::Str(s.as_str().into())),
        // Value types use their registered literal form (see `Constant::Value`).
        (Type::Object(name), json) => TypeRegistry::global().decode_value(name, &json.to_string()),
        (Type::List(element), J::Array(items)) => items
            .iter()
            .map(|item| value_from_json_with(item, element, entity_for))
            .collect::<Result<_, _>>()
            .map(Value::list),
        (Type::Tuple(types), J::Array(items)) if items.len() == types.len() => items
            .iter()
            .zip(types)
            .map(|(item, ty)| value_from_json_with(item, ty, entity_for))
            .collect::<Result<_, _>>()
            .map(Value::tuple),
        (Type::Map(key, value), J::Array(entries)) => {
            let mut map = std::collections::BTreeMap::new();
            for entry in entries {
                let [k, v] = entry.as_array().map(Vec::as_slice).unwrap_or_default() else {
                    return Err(format!("a map entry is `[key, value]`, got {entry}"));
                };
                let k = value_from_json_with(k, key, entity_for)?;
                let k = pulsar_script_vm::MapKey::from_value(&k)
                    .ok_or_else(|| format!("{k:?} cannot be a map key"))?;
                map.insert(k, value_from_json_with(v, value, entity_for)?);
            }
            Ok(Value::Map(std::sync::Arc::new(map)))
        }
        (ty, json) => Err(format!("cannot use {json} as {ty}")),
    }
}
