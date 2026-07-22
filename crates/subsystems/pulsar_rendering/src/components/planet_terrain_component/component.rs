use engine_class_derive::engine_class;
use pulsar_terrain::{PlanetDefinition, PlanetId, PlanetIdParseError, TerrainRuntimeError};
use thiserror::Error;

pub const PLANET_TERRAIN_CLASS_NAME: &str = "PlanetTerrainComponent";

/// Scene-owned planetary terrain definition. Canonical data and worker state
/// remain in the `pulsar_terrain` runtime supplied to the component context.
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
        let definition = PlanetDefinition {
            planet_id,
            center_cell: [self.center_cell_x, self.center_cell_y, self.center_cell_z],
            radius_cells: self.radius_cells,
            material,
            root_lod,
            max_resident_pages,
        };
        if !definition.fits_centered_root() {
            return Err(ComponentError::PlanetOutsideRoot);
        }
        Ok(definition)
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
    #[error("planet bounds must fit inside the centered sparse hierarchy root")]
    PlanetOutsideRoot,
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use pulsar_reflection::EngineClass;

    #[test]
    fn component_round_trips_with_stable_name() {
        let component = PlanetTerrainComponent::default();
        let json = serde_json::to_value(&component).unwrap();
        let decoded: PlanetTerrainComponent = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.radius_cells, component.radius_cells);
        assert_eq!(decoded.root_lod, component.root_lod);
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
    fn component_rejects_a_planet_that_does_not_fit_its_root() {
        let component = PlanetTerrainComponent {
            radius_cells: 10_000,
            root_lod: 4,
            ..PlanetTerrainComponent::default()
        };
        assert!(matches!(
            component.definition("too-large"),
            Err(ComponentError::PlanetOutsideRoot)
        ));
    }
}
