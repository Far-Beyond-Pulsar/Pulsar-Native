//! Natives generated from reflection and SceneDB, for every type bound in
//! the [`TypeRegistry`]:
//!
//! | name                   | signature                     | from                      |
//! |------------------------|-------------------------------|---------------------------|
//! | `C::of`                | `(entity) -> C&`              | component binding         |
//! | `C::exists`            | `(C&) -> bool`                | component binding         |
//! | `C::entity`            | `(C&) -> entity`              | component binding         |
//! | `C::<method>`          | `(C&, ..) -> ..`              | SceneDB component methods |
//! | `C::get_<f>`/`set_<f>` | `(C&) -> T` / `(C&, T)`       | reflected struct fields   |
//! | `V::<method>`          | `(V / inout V, ..) -> ..`     | reflected methods         |
//! | `V::get_<f>`/`set_<f>` | `(V) -> T` / `(inout V, T)`   | reflected struct fields   |
//! | `X::<fn>`              | `(..) -> ..`                  | reflected associated fns  |
//!
//! Methods and fields whose types scripts cannot represent are skipped.

use std::any::{Any, TypeId};

use pulsar_reflection::methods::{
    methods_of, MethodInfo, PassMode, Receiver, ReceiverKind, ReflectedMethod,
};
use pulsar_reflection::{FieldInfo, TypeStructure, RUNTIME_TYPE_REGISTRY};
use pulsar_scenedb::component_methods::component_methods_of_type;
use pulsar_scenedb::{ComponentId, ComponentMethod, ComponentRef, Entity};

use crate::error::ScriptError;
use crate::module::{Param, Signature};
use crate::native::{NativeFn, NativeRegistry};
use crate::types::{ComponentBinding, Type, TypeBinding, TypeRegistry};
use crate::value::Value;

pub(crate) fn register(registry: &mut NativeRegistry) {
    let types = TypeRegistry::global();
    let mut natives = Vec::new();

    for component in types.components() {
        let ty = Type::Component(component.name.to_owned());
        accessors(component, &ty, &mut natives);
        let cid = component.component_id();
        for method in component_methods_of_type(component.ty.type_id()) {
            natives.extend(component_method(component.name, &ty, cid, *method));
        }
        for method in methods_of(component.ty.type_id()) {
            if method.receiver == ReceiverKind::None {
                natives.extend(reflected(component.name, None, method));
            }
        }
        for field in struct_fields(component.ty.type_id()) {
            natives.extend(component_field(component.name, &ty, cid, field));
        }
    }

    for reg in types.value_type_registrations() {
        let ty = Type::Object(reg.name.to_owned());
        for method in methods_of(reg.ty.type_id()) {
            let receiver = (method.receiver != ReceiverKind::None).then(|| ty.clone());
            natives.extend(reflected(reg.name, receiver, method));
        }
        for field in struct_fields(reg.ty.type_id()) {
            natives.extend(value_field(reg.name, reg.ty.type_id(), &ty, field));
        }
    }

    for native in natives {
        if let Err(err) = registry.register(native) {
            tracing::error!("script natives: {err}");
        }
    }
}

fn struct_fields(ty: TypeId) -> &'static [FieldInfo] {
    match RUNTIME_TYPE_REGISTRY.get_by_id(ty).map(|info| &info.structure) {
        Some(TypeStructure::Struct { fields }) => fields,
        _ => &[],
    }
}

/// Script parameters and bindings for a method's parameters, or `None` if
/// any type is not script-visible.
fn params(info: &MethodInfo) -> Option<(Vec<Param>, Vec<&'static TypeBinding>)> {
    let types = TypeRegistry::global();
    let mut params = Vec::new();
    let mut bindings = Vec::new();
    for p in info.params {
        let binding = types.binding(p.ty.type_id())?;
        let ty = binding.script_type();
        params.push(if p.mode == PassMode::Mut { Param::inout(ty) } else { Param::new(ty) });
        bindings.push(binding);
    }
    Some((params, bindings))
}

fn ret(info: &MethodInfo) -> Option<(Type, Option<&'static TypeBinding>)> {
    match &info.ret {
        None => Some((Type::Unit, None)),
        Some(ret) => {
            let binding = TypeRegistry::global().binding(ret.type_id())?;
            Some((binding.script_type(), Some(binding)))
        }
    }
}

fn skip(owner: &str, info: &MethodInfo) {
    tracing::debug!(
        "script natives: skipping {owner}::{}: a parameter or return type is not script-visible",
        info.name
    );
}

fn builder(owner: &str, info: &MethodInfo, receiver: Option<&Type>) -> crate::native::NativeBuilder {
    let mut names: Vec<&str> = Vec::new();
    if receiver.is_some() {
        names.push("self");
    }
    names.extend(info.params.iter().map(|p| p.name));
    let mut builder = NativeFn::builder(format!("{owner}::{}", info.name))
        .doc(info.doc)
        .flags(info.flags)
        .params(names);
    for (k, v) in info.attrs {
        builder = builder.attr(*k, *v);
    }
    if let Some(ty) = receiver {
        builder = builder.method_of(ty.clone());
    }
    builder
}

/// Convert script arguments to the boxed slots a reflected call takes.
fn box_args(bindings: &[&TypeBinding], args: &[Value]) -> Result<Vec<Box<dyn Any>>, ScriptError> {
    bindings
        .iter()
        .zip(args)
        .map(|(b, v)| b.from_value(v).map_err(ScriptError::native))
        .collect()
}

/// Copy `&mut` parameters back to their script arguments.
fn write_back(info: &MethodInfo, bindings: &[&TypeBinding], boxed: &[Box<dyn Any>], args: &mut [Value]) {
    for (i, p) in info.params.iter().enumerate() {
        if p.mode == PassMode::Mut {
            args[i] = bindings[i].to_value(&*boxed[i]);
        }
    }
}

fn to_ret(binding: Option<&TypeBinding>, value: Option<Box<dyn Any>>) -> Value {
    match (binding, value) {
        (Some(binding), Some(value)) => binding.to_value(&*value),
        _ => Value::Unit,
    }
}

/// A reflected method or associated fn of `owner`. `receiver` is the
/// value type for methods (`None` for associated fns).
fn reflected(owner: &str, receiver: Option<Type>, method: &'static ReflectedMethod) -> Option<NativeFn> {
    let info = &method.info;
    let (Some((mut sig_params, bindings)), Some((ret_ty, ret_binding))) = (params(info), ret(info))
    else {
        skip(owner, info);
        return None;
    };
    let kind = method.receiver;
    if let Some(ty) = &receiver {
        let param = if kind == ReceiverKind::Mut { Param::inout(ty.clone()) } else { Param::new(ty.clone()) };
        sig_params.insert(0, param);
    }
    let offset = usize::from(receiver.is_some());
    let native = builder(owner, info, receiver.as_ref()).build_raw(
        Signature::new(sig_params, ret_ty),
        Box::new(move |_host, args| {
            let (receiver_arg, rest) = args.split_at_mut(offset);
            let mut boxed = box_args(&bindings, rest)?;
            let receiver = match (kind, receiver_arg.first_mut()) {
                (ReceiverKind::None, _) => Receiver::None,
                (ReceiverKind::Ref, Some(Value::Object(obj))) => Receiver::Ref(obj.as_any()),
                (ReceiverKind::Mut, Some(Value::Object(obj))) => Receiver::Mut(obj.as_any_mut()),
                _ => return Err(ScriptError::native("receiver is not an object")),
            };
            let result = method
                .call(receiver, &mut boxed)
                .map_err(|e| ScriptError::native(e.to_string()))?;
            write_back(&method.info, &bindings, &boxed, rest);
            Ok(to_ret(ret_binding, result))
        }),
    );
    Some(native)
}

fn component_method(
    owner: &str,
    ty: &Type,
    cid: ComponentId,
    method: ComponentMethod,
) -> Option<NativeFn> {
    let info = method.info();
    let (Some((mut sig_params, bindings)), Some((ret_ty, ret_binding))) = (params(info), ret(info))
    else {
        skip(owner, info);
        return None;
    };
    sig_params.insert(0, Param::new(ty.clone()));
    let native = builder(owner, info, Some(ty)).build_raw(
        Signature::new(sig_params, ret_ty),
        Box::new(move |host, args| {
            let entity = component_entity(&args[0])?;
            let rest = &mut args[1..];
            let mut boxed = box_args(&bindings, rest)?;
            let result = host
                .world
                .invoke_component_method(entity, cid, method, &mut boxed)
                .map_err(|e| ScriptError::native(e.to_string()))?;
            write_back(method.info(), &bindings, &boxed, rest);
            Ok(to_ret(ret_binding, result))
        }),
    );
    Some(native)
}

fn component_entity(value: &Value) -> Result<Entity, ScriptError> {
    value
        .as_component()
        .map(|c| c.entity)
        .ok_or_else(|| ScriptError::native("expected a component reference"))
}

fn accessors(component: &ComponentBinding, ty: &Type, natives: &mut Vec<NativeFn>) {
    let name = component.name;
    let cid = component.component_id();
    let sig = |params: Vec<Param>, ret: Type| Signature::new(params, ret);

    natives.push(
        NativeFn::builder(format!("{name}::of"))
            .doc(format!("Reference to the entity's {name}. Resolves only while it has one."))
            .pure()
            .params(["entity"])
            .build_raw(
                sig(vec![Param::new(Type::Entity)], ty.clone()),
                Box::new(move |_host, args| {
                    let entity = args[0].as_entity().unwrap_or(Entity::DANGLING);
                    Ok(Value::Component(ComponentRef::new(entity, cid)))
                }),
            ),
    );
    natives.push(
        NativeFn::builder(format!("{name}::exists"))
            .doc(format!("Whether the referenced entity is alive and has a {name}."))
            .flags(pulsar_reflection::MethodFlags { side_effect_free: true, deterministic: false })
            .method_of(ty.clone())
            .params(["self"])
            .build_raw(
                sig(vec![Param::new(ty.clone())], Type::Bool),
                Box::new(move |host, args| {
                    let entity = component_entity(&args[0])?;
                    Ok(Value::Bool(host.world.has_component(entity, cid)))
                }),
            ),
    );
    natives.push(
        NativeFn::builder(format!("{name}::entity"))
            .doc("The entity this reference points at.")
            .pure()
            .method_of(ty.clone())
            .params(["self"])
            .build_raw(
                sig(vec![Param::new(ty.clone())], Type::Entity),
                Box::new(move |_host, args| Ok(Value::Entity(component_entity(&args[0])?))),
            ),
    );
}

fn missing(entity: Entity, component: &str) -> ScriptError {
    ScriptError::native(format!("{entity:?} has no {component}"))
}

/// The object in `value`, checked to hold Rust type `ty` (field access
/// does pointer arithmetic on it).
fn object_of(value: &Value, ty: TypeId) -> Result<&crate::value::Object, ScriptError> {
    match value {
        Value::Object(obj) if obj.as_any().type_id() == ty => Ok(obj),
        other => Err(ScriptError::native(format!("unexpected {}", other.kind()))),
    }
}

fn field_binding(owner: &str, field: &FieldInfo) -> Option<&'static TypeBinding> {
    let binding = TypeRegistry::global().binding(field.type_info.type_id);
    if binding.is_none() {
        tracing::debug!("script natives: skipping field {owner}.{}: type not script-visible", field.name);
    }
    binding
}

fn component_field(owner: &str, ty: &Type, cid: ComponentId, field: &'static FieldInfo) -> Vec<NativeFn> {
    let Some(binding) = field_binding(owner, field) else { return Vec::new() };
    let field_ty = binding.script_type();
    let offset = field.offset;
    let (get_name, set_name) = (owner.to_owned(), owner.to_owned());

    let get = NativeFn::builder(format!("{owner}::get_{}", field.name))
        .doc(format!("The {owner}'s `{}`.", field.name))
        .flags(pulsar_reflection::MethodFlags { side_effect_free: true, deterministic: false })
        .method_of(ty.clone())
        .params(["self"])
        .attr("property", field.name)
        .build_raw(
            Signature::new([Param::new(ty.clone())], field_ty.clone()),
            Box::new(move |host, args| {
                let entity = component_entity(&args[0])?;
                let value =
                    host.world.get_dyn(entity, cid).ok_or_else(|| missing(entity, &get_name))?;
                let base = (value as *const dyn Any).cast::<u8>();
                // SAFETY: `value` is a live component of the reflected type,
                // and `offset`/`binding` come from its reflected field
                // (`offset_of!` and the field's own TypeId).
                Ok(unsafe { binding.load(base.add(offset)) })
            }),
        );
    let set = NativeFn::builder(format!("{owner}::set_{}", field.name))
        .doc(format!("Set the {owner}'s `{}`.", field.name))
        .method_of(ty.clone())
        .params(["self", "value"])
        .attr("property", field.name)
        .build_raw(
            Signature::new([Param::new(ty.clone()), Param::new(field_ty)], Type::Unit),
            Box::new(move |host, args| {
                let entity = component_entity(&args[0])?;
                let mut guard =
                    host.world.get_dyn_mut(entity, cid).ok_or_else(|| missing(entity, &set_name))?;
                let base = (&mut *guard as *mut dyn Any).cast::<u8>();
                // SAFETY: as for the getter; the guard holds the unique
                // borrow and reports the write when dropped.
                unsafe { binding.store(base.add(offset), &args[1]) }.map_err(ScriptError::native)?;
                Ok(Value::Unit)
            }),
        );
    vec![get, set]
}

fn value_field(owner: &str, owner_ty: TypeId, ty: &Type, field: &'static FieldInfo) -> Vec<NativeFn> {
    let Some(binding) = field_binding(owner, field) else { return Vec::new() };
    let field_ty = binding.script_type();
    let offset = field.offset;

    let get = NativeFn::builder(format!("{owner}::get_{}", field.name))
        .doc(format!("The {owner}'s `{}`.", field.name))
        .pure()
        .method_of(ty.clone())
        .params(["self"])
        .attr("property", field.name)
        .build_raw(
            Signature::new([Param::new(ty.clone())], field_ty.clone()),
            Box::new(move |_host, args| {
                let obj = object_of(&args[0], owner_ty)?;
                let base = (obj.as_any() as *const dyn Any).cast::<u8>();
                // SAFETY: `obj` holds the reflected type (checked above);
                // offset/binding come from its reflected field.
                Ok(unsafe { binding.load(base.add(offset)) })
            }),
        );
    let set = NativeFn::builder(format!("{owner}::set_{}", field.name))
        .doc(format!("Set the {owner}'s `{}`.", field.name))
        .pure()
        .method_of(ty.clone())
        .params(["self", "value"])
        .attr("property", field.name)
        .build_raw(
            Signature::new([Param::inout(ty.clone()), Param::new(field_ty)], Type::Unit),
            Box::new(move |_host, args| {
                let (target, value) = args.split_at_mut(1);
                object_of(&target[0], owner_ty)?;
                let Value::Object(obj) = &mut target[0] else { unreachable!("checked above") };
                let base = (obj.as_any_mut() as *mut dyn Any).cast::<u8>();
                // SAFETY: as for the getter; `obj` is uniquely borrowed.
                unsafe { binding.store(base.add(offset), &value[0]) }.map_err(ScriptError::native)?;
                Ok(Value::Unit)
            }),
        );
    vec![get, set]
}
