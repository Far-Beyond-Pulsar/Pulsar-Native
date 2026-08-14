use engine_class_derive::{register_runtime_behavior, register_world_component};
use pulsar_reflection::{
    ComponentRuntimeBehavior, ComponentRuntimeContext, LiveKeySet, RuntimeComponentOwner,
};
use pulsar_terrain::TerrainRuntimeHandle;

use super::{
    ComponentError, PLANET_TERRAIN_CLASS_NAME, PlanetTerrainComponent, PlanetTerrainComponentCache,
};

// Phase B5 (Pulsar-Native#556).
#[register_world_component]
#[register_runtime_behavior]
impl ComponentRuntimeBehavior for PlanetTerrainComponent {
    const CLASS_NAME: &'static str = PLANET_TERRAIN_CLASS_NAME;

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component: &Self,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        // Component discovery is shared by editor and game contexts. The
        // production terrain runtime is registered only by hosts that have
        // enabled planetary terrain, so its absence is not a component error.
        let Some(runtime) = context
            .subsystems_mut()
            .get_mut::<TerrainRuntimeHandle>()
            .cloned()
        else {
            return;
        };

        let source_key = format!("{}:{component_index}", owner.scene_object_id);
        let result = if component.enabled {
            component.definition(&source_key).and_then(|definition| {
                let planet_id = definition.planet_id;
                runtime
                    .upsert_component(source_key.clone(), definition)
                    .map_err(ComponentError::Runtime)?;
                if let Some(cache) = context
                    .subsystems_mut()
                    .get_mut::<PlanetTerrainComponentCache>()
                {
                    cache.record(source_key.clone(), planet_id);
                }
                if let Some(live_keys) = context.subsystems_mut().get_mut::<LiveKeySet>() {
                    live_keys.insert(source_key.clone());
                }
                Ok(())
            })
        } else {
            let result = runtime
                .remove_component(&source_key)
                .map_err(ComponentError::Runtime);
            if result.is_ok() {
                if let Some(cache) = context
                    .subsystems_mut()
                    .get_mut::<PlanetTerrainComponentCache>()
                {
                    cache.remove(&source_key);
                }
            }
            result
        };
        if let Err(error) = result {
            context.report_error(format!(
                "{PLANET_TERRAIN_CLASS_NAME} on '{}': {error}",
                owner.scene_object_id
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_subsystems::{Subsystem, SubsystemContext};
    use pulsar_reflection::{Subsystems, apply_runtime_behavior_for_class};
    use pulsar_terrain::{TerrainRuntimeConfig, TerrainSubsystem};
    use serde_json::Value;
    use std::{
        collections::HashMap,
        path::{Path, PathBuf},
    };

    struct TestRuntimeContext {
        project_root: PathBuf,
        subsystems: Subsystems,
        errors: Vec<String>,
    }

    impl ComponentRuntimeContext for TestRuntimeContext {
        fn subsystems_mut(&mut self) -> &mut Subsystems {
            &mut self.subsystems
        }

        fn project_root(&self) -> &Path {
            &self.project_root
        }

        fn report_error(&mut self, message: String) {
            self.errors.push(message);
        }
    }

    fn owner<'a>(props: &'a HashMap<String, Value>) -> RuntimeComponentOwner<'a> {
        RuntimeComponentOwner {
            scene_object_id: "earth",
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            props,
        }
    }

    #[test]
    fn runtime_behavior_has_the_reflected_component_name() {
        assert_eq!(
            <PlanetTerrainComponent as ComponentRuntimeBehavior>::CLASS_NAME,
            PLANET_TERRAIN_CLASS_NAME
        );
    }

    #[test]
    fn missing_optional_runtime_is_quiet() {
        let mut context = TestRuntimeContext {
            project_root: PathBuf::from("."),
            subsystems: Subsystems::new(),
            errors: Vec::new(),
        };
        let props = HashMap::new();

        assert!(apply_runtime_behavior_for_class(
            PLANET_TERRAIN_CLASS_NAME,
            &owner(&props),
            0,
            &serde_json::to_value(PlanetTerrainComponent::default()).unwrap(),
            &mut context,
        ));
        assert!(context.errors.is_empty());
    }

    #[test]
    fn registered_runtime_behavior_upserts_and_removes_the_planet() {
        let mut terrain = TerrainSubsystem::new(TerrainRuntimeConfig {
            worker_count: 1,
            ..TerrainRuntimeConfig::default()
        })
        .unwrap();
        terrain.init(&SubsystemContext::new()).unwrap();
        let handle = terrain.runtime_handle();

        let mut subsystems = Subsystems::new();
        subsystems.register(handle.clone());
        subsystems.register(PlanetTerrainComponentCache::default());
        subsystems.register(LiveKeySet::new());
        let mut context = TestRuntimeContext {
            project_root: PathBuf::from("."),
            subsystems,
            errors: Vec::new(),
        };
        let props = HashMap::new();
        let component = PlanetTerrainComponent::default();

        assert!(apply_runtime_behavior_for_class(
            PLANET_TERRAIN_CLASS_NAME,
            &owner(&props),
            0,
            &serde_json::to_value(&component).unwrap(),
            &mut context,
        ));
        assert!(context.errors.is_empty());
        assert_eq!(handle.counters().planets, 1);
        assert_eq!(
            context
                .subsystems
                .get_mut::<PlanetTerrainComponentCache>()
                .unwrap()
                .active_planets()
                .len(),
            1
        );

        let disabled = PlanetTerrainComponent {
            enabled: false,
            ..component
        };
        assert!(apply_runtime_behavior_for_class(
            PLANET_TERRAIN_CLASS_NAME,
            &owner(&props),
            0,
            &serde_json::to_value(disabled).unwrap(),
            &mut context,
        ));
        assert!(context.errors.is_empty());
        assert_eq!(handle.counters().planets, 0);
        assert!(
            context
                .subsystems
                .get_mut::<PlanetTerrainComponentCache>()
                .unwrap()
                .active_planets()
                .is_empty()
        );

        terrain.shutdown().unwrap();
    }
}
