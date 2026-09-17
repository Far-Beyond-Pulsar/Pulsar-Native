//! Scene `PlanetTerrainComponent`s → canonical `PlanetDefinition`s.
//!
//! A planet exists at runtime because some scene object carries a
//! `PlanetTerrainComponent`. Feeding those components into the terrain runtime
//! used to be the job of the generic world-component dispatch in
//! `HelioRenderer`; the SceneDB nativization work removed that path, leaving
//! `HelioInner::planet_terrain` permanently `None` and no planet in the editor
//! at all.
//!
//! Rather than block the terrain seam on that being restored, this module
//! reads the components straight out of the editor's own scene database and
//! posts them through [`TerrainEditApi::sync_scene_planets`]. The source-key
//! format matches `PlanetTerrainComponent`'s own
//! `ComponentRuntimeBehavior::sync_component` exactly (`"{object_id}:{index}"`)
//! so that when the generic dispatch returns, both paths address the same
//! planet instead of registering a duplicate.

use engine_backend::services::terrain_edit::{PlanetDefinition, TerrainEditApi};
use helio_component::PlanetTerrainComponent;

use crate::level_editor::core::scene_database::SceneDatabase;

/// The component class this module looks for.
const PLANET_TERRAIN_CLASS: &str = "PlanetTerrainComponent";

/// Collect every enabled planet the scene defines, paired with the stable
/// source key of the component that authored it.
///
/// Malformed components (bad radius, out-of-range LOD, a planet that does not
/// fit its hierarchy root) are logged and skipped rather than failing the whole
/// sync -- one bad component must not take the rest of the scene's terrain
/// down with it.
pub fn collect_scene_planets(database: &SceneDatabase) -> Vec<(String, PlanetDefinition)> {
    let mut planets = Vec::new();
    for object in database.get_all_objects() {
        for (index, instance) in database.get_components(&object.id).into_iter().enumerate() {
            if !instance.enabled || instance.class_name != PLANET_TERRAIN_CLASS {
                continue;
            }
            let component: PlanetTerrainComponent = match serde_json::from_value(instance.data) {
                Ok(component) => component,
                Err(error) => {
                    tracing::warn!(
                        object = %object.id,
                        %error,
                        "skipping a PlanetTerrainComponent that could not be decoded"
                    );
                    continue;
                }
            };
            if !component.enabled {
                continue;
            }
            let source_key = format!("{}:{index}", object.id);
            match component.definition(&source_key) {
                Ok(definition) => planets.push((source_key, definition)),
                Err(error) => tracing::warn!(
                    object = %object.id,
                    %error,
                    "skipping an invalid PlanetTerrainComponent"
                ),
            }
        }
    }
    planets
}

/// Push the scene's planets at the terrain runtime.
///
/// Latest-wins and idempotent: the render thread upserts what it is given and
/// retires any source key that has disappeared, so calling this whenever the
/// scene revision changes is enough to keep the runtime in step.
pub fn sync_scene_planets(database: &SceneDatabase, api: &TerrainEditApi) {
    api.sync_scene_planets(collect_scene_planets(database));
}
