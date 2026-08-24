//! Shared test fixtures (compiled only under `cfg(test)`).
//!
//! One hand-registered component class (`TestGizmo`) proves the full
//! reflection-dispatched pipeline end-to-end without depending on any
//! renderer-side component crate -- the same pattern
//! `pulsar_world_registry`'s own tests use (`TestComponent` there,
//! `TestGizmo` here so both crates' test binaries can coexist).

#![allow(dead_code)]

use pulsar_reflection::{
    ComponentMethodRegistration, EngineClass, MethodMetadata, MethodReturnType, MethodType,
    PropertyMetadata, RuntimeTypeInfo, RUNTIME_TYPE_REGISTRY,
};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use crate::instances::{ComponentInstanceStore, InstanceRecord};

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
        let type_info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 prim registered");
        vec![PropertyMetadata {
            name: "charges",
            display_name: "Charges".into(),
            category: None,
            category_color: None,
            category_default_collapsed: false,
            category_order: None,
            type_info,
            getter: Box::new(|c: &dyn EngineClass| Box::new(c.as_any().downcast_ref::<TestGizmo>().unwrap().charges)),
            setter: Box::new(|c: &mut dyn EngineClass, v: Box<dyn std::any::Any>| {
                if let Some(v) = v.downcast_ref::<i32>() {
                    c.as_any_mut().downcast_mut::<TestGizmo>().unwrap().charges = *v;
                }
            }),
        }]
    }

    fn get_methods() -> Vec<MethodMetadata> {
        let i32_info: &'static RuntimeTypeInfo = RUNTIME_TYPE_REGISTRY.get::<i32>().expect("i32 prim registered");
        vec![MethodMetadata {
            name: "add_charges",
            display_name: "Add Charges".into(),
            category: None,
            params: vec![pulsar_reflection::MethodParameter { name: "amount", type_info: i32_info }],
            return_type: Some(MethodReturnType { type_info: i32_info }),
            method_type: MethodType::Fn,
            caller: Box::new(|c: &mut dyn EngineClass, args: Vec<Box<dyn std::any::Any>>| {
                let amount = args.first().and_then(|a| a.downcast_ref::<i32>()).copied()?;
                let gizmo = c.as_any_mut().downcast_mut::<TestGizmo>()?;
                gizmo.charges += amount;
                Some(Box::new(gizmo.charges))
            }),
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
    world.get::<TestGizmo>(entity).map(|c| c as &dyn EngineClass)
}

fn test_gizmo_get_mut(world: &mut World, entity: Entity) -> Option<&mut dyn EngineClass> {
    // `World::get_mut` hands back SceneDB's dirty-tracking `Mut` guard;
    // `.into_inner()` extracts the raw reference exactly like the generated
    // shims do (a write-through here counts as a real mutation for
    // subscriptions/GPU mirrors).
    world.get_mut::<TestGizmo>(entity).map(|c| c.into_inner() as &mut dyn EngineClass)
}

fn test_gizmo_hydrate(world: &mut World, entity: Entity, data: &Value) -> Result<(), String> {
    let parsed: TestGizmo = serde_json::from_value(data.clone()).map_err(|e| e.to_string())?;
    world.insert(entity, parsed);
    Ok(())
}

fn test_gizmo_remove(world: &mut World, entity: Entity) {
    let _ = world.remove::<TestGizmo>(entity);
}

fn test_gizmo_on_removed(
    _owner: &pulsar_reflection::RuntimeComponentOwner,
    _context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
) {
}

fn test_gizmo_dispatch(
    world: &World,
    entity: Entity,
    _owner: &pulsar_reflection::RuntimeComponentOwner,
    _component_index: usize,
    _context: &mut dyn pulsar_reflection::ComponentRuntimeContext,
) -> bool {
    world.get::<TestGizmo>(entity).is_some()
}

fn test_gizmo_refresh_gpu_mirror(_world: &mut World, _entity: Entity) {}

fn test_gizmo_test_methods() -> Vec<pulsar_reflection::MethodMetadata> {
    <TestGizmo as EngineClass>::get_methods()
}

fn test_gizmo_from_json(
    data: &serde_json::Value,
) -> Result<Box<dyn EngineClass>, String> {
    serde_json::from_value::<TestGizmo>(data.clone())
        .map(|g| Box::new(g) as Box<dyn EngineClass>)
        .map_err(|e| e.to_string())
}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::WorldComponentRegistration {
        class_name: "TestGizmo",
        component_type: pulsar_scenedb::component_id::<TestGizmo>,
        hydrate: test_gizmo_hydrate,
        remove: test_gizmo_remove,
        dispatch: test_gizmo_dispatch,
        get_as_engine_class: test_gizmo_get,
        get_as_engine_class_mut: test_gizmo_get_mut,
        on_removed: test_gizmo_on_removed,
        refresh_gpu_mirror: test_gizmo_refresh_gpu_mirror,
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

/// In-memory [`ComponentInstanceStore`] mirroring the editor's persisted
/// component list shape -- records attached positionally to one entity.
#[derive(Default)]
pub(crate) struct FakeInstanceStore {
    entity: Option<Entity>,
    records: Vec<InstanceRecord>,
}

impl FakeInstanceStore {
    /// Attach records to one entity, in list order.
    pub fn attach(&mut self, entity: Entity, records: &[InstanceRecord]) {
        self.entity = Some(entity);
        self.records = records.to_vec();
    }

    pub fn record_data(&self, index: u32) -> Option<&Value> {
        self.records.get(index as usize).map(|r| &r.data)
    }
}

impl ComponentInstanceStore for FakeInstanceStore {
    fn live_component_index(&self, entity: Entity, class_name: &str) -> Option<u32> {
        if self.entity != Some(entity) {
            return None;
        }
        self.records
            .iter()
            .position(|r| r.enabled && r.class_name == class_name)
            .map(|i| i as u32)
    }

    fn instance_record(&self, entity: Entity, index: u32) -> Option<InstanceRecord> {
        if self.entity != Some(entity) {
            return None;
        }
        self.records.get(index as usize).cloned()
    }

    fn set_instance_data(&mut self, entity: Entity, index: u32, data: Value) -> bool {
        if self.entity != Some(entity) {
            return false;
        }
        match self.records.get_mut(index as usize) {
            Some(record) => {
                record.data = data;
                true
            }
            None => false,
        }
    }
}
