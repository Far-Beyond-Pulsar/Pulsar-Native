//! Shared test fixtures (compiled only under `cfg(test)`).
//!
//! One hand-registered component class (`TestGizmo`) proves the full
//! reflection-dispatched pipeline end-to-end without depending on any
//! renderer-side component crate -- the same pattern
//! `pulsar_world_registry`'s own tests use (`TestComponent` there,
//! `TestGizmo` here so both crates' test binaries can coexist).

#![allow(dead_code)]

use pulsar_reflection::{
    ComponentMethodRegistration, EngineClass, MethodFlags, MethodMetadata, MethodReturnType,
    PropertyMetadata, RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

/// Test component: one reflected `i32` property and one blueprint-callable
/// method, registered into BOTH registries the real classes register into.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TestGizmo {
    pub charges: i32,
}

impl EngineClass for TestGizmo {
    fn class_name() -> &'static str {
        "TestGizmo"
    }

    fn get_properties(&self) -> Vec<PropertyMetadata> {
        let type_info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY
            .get::<i32>()
            .expect("i32 prim registered");
        vec![PropertyMetadata {
            name: "charges",
            display_name: "Charges".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info,
            getter: Box::new(|c: &dyn EngineClass| {
                Box::new(c.as_any().downcast_ref::<TestGizmo>().unwrap().charges)
            }),
            setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                if let Some(v) = v.downcast_ref::<i32>() {
                    c.as_any_mut().downcast_mut::<TestGizmo>().unwrap().charges = *v;
                }
            }),
        }]
    }

    fn get_methods() -> Vec<MethodMetadata> {
        let i32_info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY
            .get::<i32>()
            .expect("i32 prim registered");
        vec![MethodMetadata {
            name: "add_charges",
            display_name: "Add Charges".into(),
            category: None,
            params: vec![pulsar_reflection::MethodParameter {
                name: "amount",
                type_info: i32_info,
            }],
            return_type: Some(MethodReturnType {
                type_info: i32_info,
            }),
            flags: MethodFlags::NONE,
            caller: Box::new(
                |c: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                    let amount = args
                        .first()
                        .and_then(|a| a.downcast_ref::<i32>())
                        .copied()?;
                    let gizmo = c.as_any_mut().downcast_mut::<TestGizmo>()?;
                    gizmo.charges += amount;
                    Some(Box::new(gizmo.charges))
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

    fn to_json(&self) -> Result<Value, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }
}

fn test_gizmo_get(world: &World, entity: Entity) -> Option<&dyn EngineClass> {
    world
        .get::<TestGizmo>(entity)
        .map(|c| c as &dyn EngineClass)
}

fn test_gizmo_get_mut(
    world: &mut World,
    entity: Entity,
) -> Option<pulsar_world_registry::EngineClassMut<'_>> {
    pulsar_world_registry::EngineClassMut::of::<TestGizmo>(world, entity)
}

fn test_gizmo_remove(world: &mut World, entity: Entity) {
    let _ = world.remove::<TestGizmo>(entity);
}

fn test_gizmo_on_removed(
    _owner: &pulsar_reflection::RuntimeComponentOwner,
    _context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
) {
}

fn test_gizmo_test_methods() -> Vec<pulsar_reflection::MethodMetadata> {
    <TestGizmo as EngineClass>::get_methods()
}

fn test_gizmo_from_json(data: &serde_json::Value) -> Result<Box<dyn EngineClass>, String> {
    serde_json::from_value::<TestGizmo>(data.clone())
        .map(|g| Box::new(g) as Box<dyn EngineClass>)
        .map_err(|e| e.to_string())
}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::WorldComponentRegistration {
        class_name: "TestGizmo",
        component_type: pulsar_scenedb::component_id::<TestGizmo>,
        default_value: pulsar_world_registry::values::erased::default_value::<TestGizmo>,
        decode: pulsar_world_registry::values::erased::decode_json::<TestGizmo>,
        clone_value: pulsar_world_registry::values::erased::clone_value::<TestGizmo>,
        value_as_engine_class: pulsar_world_registry::values::erased::as_engine_class::<TestGizmo>,
        value_as_engine_class_mut: pulsar_world_registry::values::erased::as_engine_class_mut::<TestGizmo>,
        register_erased: pulsar_scenedb::register_component::<TestGizmo>,
        remove: test_gizmo_remove,
        get_as_engine_class: test_gizmo_get,
        get_as_engine_class_mut: test_gizmo_get_mut,
        on_removed: test_gizmo_on_removed,
        property_written: pulsar_world_registry::values::erased::no_property_written,
    }
}

pulsar_reflection::inventory::submit! {
    pulsar_reflection::EngineClassRegistration {
        name: "TestGizmo",
        category: None,
        constructor: <TestGizmo as EngineClass>::create_default,
        from_json: Some(test_gizmo_from_json),
    }
}

pulsar_reflection::inventory::submit! {
    ComponentMethodRegistration {
        class_name: "TestGizmo",
        methods: test_gizmo_test_methods,
    }
}
