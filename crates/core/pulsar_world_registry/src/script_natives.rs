//! Every registered world component, visible to scripts.
//!
//! Registers each [`WorldComponentRegistration`](crate::WorldComponentRegistration)
//! with the script VM under its class name (so `Class&` references,
//! `Class::of` and `Class::exists` work), plus natives for its reflected
//! properties and methods:
//!
//! - `Class::get_<property>(Class&) -> T` and
//!   `Class::set_<property>(Class&, T)`,
//! - `Class::<method>(Class&, ..) -> ..`,
//!
//! for every property and method whose types scripts can represent. They
//! read and write the live component through the same bridge the
//! properties panel uses, so writes reach SceneDB's change hooks.

use std::any::Any;
use std::sync::Arc;

use pulsar_reflection::{MethodFlags, MethodType, PropertyMetadata, REGISTRY};
use pulsar_scenedb::Entity;
use pulsar_script_vm::{
    ComponentProvider, NativeFn, NativeProvider, Param, ProvidedComponent, ScriptError,
    Signature, Type, TypeRegistry, Value,
};

use crate::WorldComponentRegistration;

inventory::submit! {
    ComponentProvider { components: world_components }
}

inventory::submit! {
    NativeProvider { natives: world_component_natives }
}

fn world_components() -> Vec<ProvidedComponent> {
    inventory::iter::<WorldComponentRegistration>
        .into_iter()
        .map(|r| ProvidedComponent { name: r.class_name, id: r.component_type })
        .collect()
}

fn world_component_natives() -> Vec<NativeFn> {
    let mut natives = Vec::new();
    for registration in inventory::iter::<WorldComponentRegistration> {
        let class = registration.class_name;
        let ty = Type::Component(class.to_owned());
        if let Some(instance) = REGISTRY.create_instance(class) {
            for property in instance.get_properties() {
                natives.extend(property_natives(registration, &ty, property));
            }
        }
        for method in REGISTRY.get_methods(class).unwrap_or_default() {
            natives.extend(method_native(registration, &ty, method));
        }
    }
    natives
}

fn entity_of(value: &Value) -> Result<Entity, ScriptError> {
    value
        .as_component()
        .map(|c| c.entity)
        .ok_or_else(|| ScriptError::native("expected a component reference"))
}

fn missing(entity: Entity, class: &str) -> ScriptError {
    ScriptError::native(format!("{entity:?} has no {class}"))
}

fn property_natives(
    registration: &'static WorldComponentRegistration,
    ty: &Type,
    property: PropertyMetadata,
) -> Vec<NativeFn> {
    let class = registration.class_name;
    let Some(binding) = TypeRegistry::global().binding(property.type_info.type_id) else {
        tracing::debug!("script natives: skipping {class}.{}: type not script-visible", property.name);
        return Vec::new();
    };
    let value_ty = binding.script_type();
    let name = property.name;
    let category = property.category;
    let property = Arc::new(property);

    let getter = Arc::clone(&property);
    let mut get = NativeFn::builder(format!("{class}::get_{name}"))
        .doc(format!("The {class}'s {}.", getter.display_name))
        .flags(MethodFlags { side_effect_free: true, deterministic: false })
        .method_of(ty.clone())
        .params(["self"])
        .attr("property", name);
    let mut set = NativeFn::builder(format!("{class}::set_{name}"))
        .doc(format!("Set the {class}'s {}.", property.display_name))
        .method_of(ty.clone())
        .params(["self", "value"])
        .attr("property", name);
    if let Some(category) = category {
        get = get.attr("category", category);
        set = set.attr("category", category);
    }

    let get = get.build_raw(
        Signature::new([Param::new(ty.clone())], value_ty.clone()),
        Box::new(move |host, args| {
            let entity = entity_of(&args[0])?;
            let instance = (registration.get_as_engine_class)(host.world, entity)
                .ok_or_else(|| missing(entity, class))?;
            let value = (getter.getter)(instance);
            Ok(binding.to_value(&*value))
        }),
    );
    let set = set.build_raw(
        Signature::new([Param::new(ty.clone()), Param::new(value_ty)], Type::Unit),
        Box::new(move |host, args| {
            let entity = entity_of(&args[0])?;
            let value = binding.from_value(&args[1]).map_err(ScriptError::native)?;
            let instance = (registration.get_as_engine_class_mut)(host.world, entity)
                .ok_or_else(|| missing(entity, class))?;
            (property.setter)(instance, value);
            Ok(Value::Unit)
        }),
    );
    vec![get, set]
}

fn method_native(
    registration: &'static WorldComponentRegistration,
    ty: &Type,
    method: pulsar_reflection::MethodMetadata,
) -> Option<NativeFn> {
    let class = registration.class_name;
    let types = TypeRegistry::global();
    let mut bindings = Vec::with_capacity(method.params.len());
    let mut params = vec![Param::new(ty.clone())];
    for param in &method.params {
        let Some(binding) = types.binding(param.type_info.type_id) else {
            tracing::debug!("script natives: skipping {class}::{}: parameter type not script-visible", method.name);
            return None;
        };
        params.push(Param::new(binding.script_type()));
        bindings.push(binding);
    }
    let (ret_ty, ret_binding) = match &method.return_type {
        None => (Type::Unit, None),
        Some(ret) => {
            let Some(binding) = types.binding(ret.type_info.type_id) else {
                tracing::debug!("script natives: skipping {class}::{}: return type not script-visible", method.name);
                return None;
            };
            (binding.script_type(), Some(binding))
        }
    };

    let mut names = vec!["self"];
    names.extend(method.params.iter().map(|p| p.name));
    let mut builder = NativeFn::builder(format!("{class}::{}", method.name))
        .doc(method.display_name.clone())
        .method_of(ty.clone())
        .params(names);
    if method.method_type == MethodType::Pure {
        builder = builder.side_effect_free();
    }
    if let Some(category) = method.category {
        builder = builder.attr("category", category);
    }
    let name = method.name;
    let caller = method.caller;
    Some(builder.build_raw(
        Signature::new(params, ret_ty),
        Box::new(move |host, args| {
            let entity = entity_of(&args[0])?;
            // Built from each parameter's own binding, so every argument has
            // exactly the type the generated caller downcasts to.
            let boxed: Vec<Box<dyn Any>> = bindings
                .iter()
                .zip(&args[1..])
                .map(|(b, v)| b.from_value(v).map_err(ScriptError::native))
                .collect::<Result<_, _>>()?;
            let instance = (registration.get_as_engine_class_mut)(host.world, entity)
                .ok_or_else(|| missing(entity, class))?;
            let result = caller(instance, boxed);
            match (ret_binding, result) {
                (Some(binding), Some(value)) => Ok(binding.to_value(&*value)),
                (Some(_), None) => Err(ScriptError::native(format!("{class}::{name} returned nothing"))),
                (None, _) => Ok(Value::Unit),
            }
        }),
    ))
}
