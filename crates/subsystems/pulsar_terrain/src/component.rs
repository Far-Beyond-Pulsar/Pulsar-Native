use crate::{PlanetId, PlanetIdParseError, TerrainRuntimeHandle};
use engine_class_derive::{engine_class, register_runtime_behavior};
use pulsar_reflection::{ComponentRuntimeBehavior, ComponentRuntimeContext, RuntimeComponentOwner};
use serde_json::Value;
use thiserror::Error;

pub const PLANET_TERRAIN_CLASS_NAME: &str = "PlanetTerrainComponent";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanetDefinition {
    pub planet_id: PlanetId,
    pub center_cell: [i64; 3],
    pub radius_cells: u64,
    pub material: u8,
    pub root_lod: u8,
    pub max_resident_pages: usize,
}

/// Runtime-owned planetary terrain definition. Legacy editor terrain
/// components remain separate while this production contract is validated.
#[engine_class(category = "Terrain", clone, debug, serialize, deserialize)]
pub struct PlanetTerrainComponent {
    #[property]
    pub enabled: bool,
    /// Optional 32-character hexadecimal ID. An empty value derives a stable
    /// ID from the owning scene object and component index.
    #[property]
    pub planet_id: String,
    #[property]
    pub center_cell_x: i64,
    #[property]
    pub center_cell_y: i64,
    #[property]
    pub center_cell_z: i64,
    /// Canonical radius in 10 cm LOD0 cells.
    #[property]
    pub radius_cells: u64,
    #[property]
    pub material: u64,
    #[property]
    pub root_lod: u64,
    #[property]
    pub max_resident_pages: u64,
}

impl Default for PlanetTerrainComponent {
    fn default() -> Self {
        Self {
            enabled: true,
            planet_id: String::new(),
            center_cell_x: 0,
            center_cell_y: 0,
            center_cell_z: 0,
            radius_cells: 63_710_000,
            material: 1,
            root_lod: 22,
            max_resident_pages: 8_192,
        }
    }
}

impl PlanetTerrainComponent {
    pub fn definition(&self, stable_owner_key: &str) -> Result<PlanetDefinition, ComponentError> {
        if self.radius_cells == 0 {
            return Err(ComponentError::ZeroRadius);
        }
        let material = u8::try_from(self.material)
            .ok()
            .filter(|material| *material != 0)
            .ok_or(ComponentError::Material(self.material))?;
        let root_lod = u8::try_from(self.root_lod)
            .ok()
            .filter(|lod| (1..=62).contains(lod))
            .ok_or(ComponentError::RootLod(self.root_lod))?;
        let max_resident_pages = usize::try_from(self.max_resident_pages)
            .ok()
            .filter(|pages| *pages != 0)
            .ok_or(ComponentError::ResidentPages(self.max_resident_pages))?;
        let explicit_id = self.planet_id.trim();
        let planet_id = if explicit_id.is_empty() {
            PlanetId::from_stable_name(stable_owner_key)
        } else {
            PlanetId::from_hex(explicit_id)?
        };
        Ok(PlanetDefinition {
            planet_id,
            center_cell: [self.center_cell_x, self.center_cell_y, self.center_cell_z],
            radius_cells: self.radius_cells,
            material,
            root_lod,
            max_resident_pages,
        })
    }
}

#[register_runtime_behavior]
impl ComponentRuntimeBehavior for PlanetTerrainComponent {
    const CLASS_NAME: &'static str = PLANET_TERRAIN_CLASS_NAME;

    fn sync_component(
        owner: &RuntimeComponentOwner,
        component_index: usize,
        component_data: &Value,
        context: &mut dyn ComponentRuntimeContext,
    ) {
        let component = match serde_json::from_value::<Self>(component_data.clone()) {
            Ok(component) => component,
            Err(error) => {
                context.report_error(format!(
                    "{PLANET_TERRAIN_CLASS_NAME} on '{}' is invalid: {error}",
                    owner.scene_object_id
                ));
                return;
            }
        };
        let source_key = format!("{}:{component_index}", owner.scene_object_id);
        let result = if component.enabled {
            component.definition(&source_key).and_then(|definition| {
                context
                    .subsystems_mut()
                    .get_mut::<TerrainRuntimeHandle>()
                    .ok_or(ComponentError::RuntimeUnavailable)?
                    .upsert_component(source_key, definition)
                    .map(|_| ())
                    .map_err(ComponentError::Runtime)
            })
        } else {
            context
                .subsystems_mut()
                .get_mut::<TerrainRuntimeHandle>()
                .ok_or(ComponentError::RuntimeUnavailable)
                .and_then(|runtime| {
                    runtime
                        .remove_component(&source_key)
                        .map_err(ComponentError::Runtime)
                })
        };
        if let Err(error) = result {
            context.report_error(format!(
                "{PLANET_TERRAIN_CLASS_NAME} on '{}': {error}",
                owner.scene_object_id
            ));
        }
    }
}

#[derive(Debug, Error)]
pub enum ComponentError {
    #[error(transparent)]
    PlanetId(#[from] PlanetIdParseError),
    #[error("radius_cells must be greater than zero")]
    ZeroRadius,
    #[error("material must be in 1..=255, got {0}")]
    Material(u64),
    #[error("root_lod must be in 1..=62, got {0}")]
    RootLod(u64),
    #[error("max_resident_pages must fit usize and be greater than zero, got {0}")]
    ResidentPages(u64),
    #[error("TerrainRuntimeHandle is not registered in the component context")]
    RuntimeUnavailable,
    #[error(transparent)]
    Runtime(#[from] crate::TerrainRuntimeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerrainRuntimeConfig, TerrainSubsystem};
    use engine_subsystems::{Subsystem, SubsystemContext};
    use pulsar_reflection::{
        apply_runtime_behavior_for_class, ComponentRuntimeContext, EngineClass,
        RuntimeComponentOwner, Subsystems,
    };
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

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

    #[test]
    fn component_round_trips_with_stable_runtime_name() {
        let component = PlanetTerrainComponent::default();
        let json = serde_json::to_value(&component).unwrap();
        let decoded: PlanetTerrainComponent = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.radius_cells, component.radius_cells);
        assert_eq!(decoded.root_lod, component.root_lod);
        assert_eq!(
            <PlanetTerrainComponent as ComponentRuntimeBehavior>::CLASS_NAME,
            PLANET_TERRAIN_CLASS_NAME
        );
        assert_eq!(
            PlanetTerrainComponent::class_name(),
            PLANET_TERRAIN_CLASS_NAME
        );
        assert_eq!(component.get_properties().len(), 9);
    }

    #[test]
    fn empty_component_id_is_stable_and_explicit_id_round_trips() {
        let component = PlanetTerrainComponent::default();
        let first = component.definition("earth:0").unwrap();
        let second = component.definition("earth:0").unwrap();
        assert_eq!(first.planet_id, second.planet_id);

        let explicit = PlanetTerrainComponent {
            planet_id: first.planet_id.to_hex(),
            ..PlanetTerrainComponent::default()
        };
        assert_eq!(
            explicit.definition("different").unwrap().planet_id,
            first.planet_id
        );
    }

    #[test]
    fn registered_runtime_behavior_upserts_and_removes_the_planet() {
        let config = TerrainRuntimeConfig {
            worker_count: 1,
            ..TerrainRuntimeConfig::default()
        };
        let mut terrain = TerrainSubsystem::new(config).unwrap();
        terrain.init(&SubsystemContext::new()).unwrap();
        let handle = terrain.runtime_handle();

        let mut subsystems = Subsystems::new();
        subsystems.register(handle.clone());
        let mut context = TestRuntimeContext {
            project_root: PathBuf::from("."),
            subsystems,
            errors: Vec::new(),
        };
        let props = HashMap::new();
        let owner = RuntimeComponentOwner {
            scene_object_id: "earth",
            position: [0.0; 3],
            rotation: [0.0; 3],
            scale: [1.0; 3],
            props: &props,
        };

        let component = PlanetTerrainComponent::default();
        assert!(apply_runtime_behavior_for_class(
            PLANET_TERRAIN_CLASS_NAME,
            &owner,
            0,
            &serde_json::to_value(&component).unwrap(),
            &mut context,
        ));
        assert!(context.errors.is_empty());
        assert_eq!(handle.counters().planets, 1);

        let disabled = PlanetTerrainComponent {
            enabled: false,
            ..component
        };
        assert!(apply_runtime_behavior_for_class(
            PLANET_TERRAIN_CLASS_NAME,
            &owner,
            0,
            &serde_json::to_value(disabled).unwrap(),
            &mut context,
        ));
        assert!(context.errors.is_empty());
        assert_eq!(handle.counters().planets, 0);

        terrain.shutdown().unwrap();
    }
}
