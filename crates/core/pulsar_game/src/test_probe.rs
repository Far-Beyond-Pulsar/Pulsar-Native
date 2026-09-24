//! A registered world component for tests (`VmProbe`, one `charges: i32`
//! property and an `add_charges` method), registered the same way
//! `#[engine_class]` components are.

use pulsar_reflection::{
    ComponentMethodRegistration, EngineClass, EngineClassRegistration, MethodMetadata,
    MethodParameter, MethodReturnType, MethodType, PropertyMetadata, RuntimeTypeInfo,
    RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{component_id, Entity, World};
use pulsar_world_registry::WorldComponentRegistration;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ── Probe component ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct VmProbe {
    pub(crate) charges: i32,
}

impl EngineClass for VmProbe {
    fn class_name() -> &'static str {
        "VmProbe"
    }

    fn get_properties(&self) -> Vec<PropertyMetadata> {
        let type_info: &'static RuntimeTypeInfo =
            RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 registered");
        vec![PropertyMetadata {
            name: "charges",
            display_name: "Charges".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info,
            getter: Box::new(|c: &dyn EngineClass| {
                Box::new(c.as_any().downcast_ref::<VmProbe>().unwrap().charges)
            }),
            setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                if let Some(v) = v.downcast_ref::<i32>() {
                    c.as_any_mut().downcast_mut::<VmProbe>().unwrap().charges = *v;
                }
            }),
        }]
    }

    fn get_methods() -> Vec<MethodMetadata> {
        let i32_info: &'static RuntimeTypeInfo =
            RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 registered");
        vec![MethodMetadata {
            name: "add_charges",
            display_name: "Add Charges".into(),
            category: None,
            params: vec![MethodParameter {
                name: "amount",
                type_info: i32_info,
            }],
            return_type: Some(MethodReturnType {
                type_info: i32_info,
            }),
            method_type: MethodType::Fn,
            caller: Box::new(
                |c: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                    let amount = args
                        .first()
                        .and_then(|a| a.downcast_ref::<i32>())
                        .copied()?;
                    let probe = c.as_any_mut().downcast_mut::<VmProbe>()?;
                    probe.charges += amount;
                    Some(Box::new(probe.charges))
                },
            ),
        }]
    }

    fn create_default() -> Box<dyn EngineClass> {
        Box::new(Self::default())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_boxed(&self) -> Box<dyn EngineClass> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<JsonValue, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}

fn vm_probe_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
    world.get::<VmProbe>(entity).map(|c| c as &dyn EngineClass)
}

fn vm_probe_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
    world
        .get_mut::<VmProbe>(entity)
        .map(|c| c.into_inner() as &mut dyn EngineClass)
}

fn vm_probe_hydrate(world: &mut World, entity: Entity, data: &JsonValue) -> Result<(), String> {
    let parsed: VmProbe = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    world.insert(entity, parsed);
    Ok(())
}

fn vm_probe_remove(world: &mut World, entity: Entity) {
    let _ = world.remove::<VmProbe>(entity);
}

pulsar_world_registry::inventory::submit! {
    WorldComponentRegistration {
        class_name: "VmProbe",
        component_type: component_id::<VmProbe>,
        hydrate: vm_probe_hydrate,
        remove: vm_probe_remove,
        dispatch: |world, entity, _: _, _: usize, _: _| world.get::<VmProbe>(entity).is_some(),
        get_as_engine_class: vm_probe_get,
        get_as_engine_class_mut: vm_probe_get_mut,
        on_removed: |_, _| {},
        refresh_gpu_mirror: |_, _| {},
    }
}

pulsar_reflection::inventory::submit! {
    EngineClassRegistration {
        name: "VmProbe",
        category: None,
        constructor: <VmProbe as EngineClass>::create_default,
        from_json: None,
    }
}

pulsar_reflection::inventory::submit! {
    ComponentMethodRegistration {
        class_name: "VmProbe",
        methods: <VmProbe as EngineClass>::get_methods,
    }
}

