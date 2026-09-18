//! Foliage stamp: brush + terrain hit → a `FoliageComponent`-carrying `SceneObjectData`.
//!
//! Foliage painting is authoring-data stamping, not per-blade placement (design doc
//! §6): each stamp creates one scene object at the brush's terrain hit point carrying a
//! `FoliageComponent` configured from the brush, and the *existing* GPU foliage stack
//! (`helio-pass-foliage-place`) keeps deciding which blades actually render per tile from
//! that authoring data. This module never touches the GPU passes.
//!
//! # `FoliageComponent`, not `FoliageTypeComponent`/`FoliageLayerComponent`
//!
//! The design doc's §6 sketch and GitHub issue #713 both describe painting a
//! `FoliageTypeComponent` + `FoliageLayerComponent` pair directly via `AddObject`. Those
//! types are real, but they are GPU-internal ECS components
//! (`helio_pass_foliage_place::components::*`) that `FoliageComponent`'s
//! `ComponentRuntimeBehavior::sync_component` impl (helio-component's
//! `foliage_component/runtime.rs`) derives automatically, every frame, from ONE
//! editor-authored `FoliageComponent`. The editor never constructs the GPU-internal pair
//! directly -- it stamps `FoliageComponent`, exactly as a user would by hand through the
//! Properties panel's "Add Component", and the already-working `sync_component`/
//! `FoliageCache` machinery does the rest. This module corrects the design doc's sketch
//! accordingly.
//!
//! # Component data shape: nested, not flat
//!
//! `FoliageComponent` also exposes a *flat* `from_component_data`/`to_scene_props` pair
//! (`mapping.rs`), used only by its `ScenePropsProjector` impl to bridge into the legacy
//! string-keyed scene-props map. That is NOT the shape `ComponentInstance::data` must
//! carry for a scene object to actually hydrate a live component: object hydration
//! (`SceneDatabase::add_object` -> `attach_component_instance` ->
//! `hydrate_canonical_component` -> `pulsar_world_registry::hydrate_world_component_for_class`)
//! deserializes `data` with `FoliageComponent`'s own derived `Deserialize`, which is
//! NESTED by Rust field name: `{"general": {...}, "placement": {...}, "wind": {...},
//! "interaction": {...}, "rendering": {...}}`. Building the flat shape here would silently
//! fail to hydrate a live component (it would round-trip through the properties panel's
//! JSON storage but never reach the World). This module always builds the component value
//! itself and serializes it with `serde_json::to_value`, never a hand-written flat map, so
//! it can never drift from whichever shape `FoliageComponent`'s derive actually produces.

use engine_backend::services::terrain_edit::TerrainHit;
use helio_component::FoliageComponent;

use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};
use crate::level_editor::state::terrain::FoliageBrush;

/// How far below the hit point the placement altitude band reaches, in
/// meters. Matches `PlacementFoliageProps::default()`'s `altitude_min`
/// (-20.0), just re-anchored -- see [`recenter_altitude_band`].
const DEFAULT_ALTITUDE_BELOW_M: f32 = 20.0;

/// How far above the hit point the placement altitude band reaches, in
/// meters. Matches `PlacementFoliageProps::default()`'s `altitude_max`
/// (50.0), re-anchored the same way.
const DEFAULT_ALTITUDE_ABOVE_M: f32 = 50.0;

/// Build the `FoliageComponent` one brush stamp should carry.
///
/// Brush knobs with a direct component equivalent flow straight across:
/// `density` -> `general.density`, `slope_limit` -> `placement.slope_min/
/// max_degrees`. `radius_m` becomes the layer's half-extent
/// (`placement.layer_extent`) -- the brush's footprint is what should scope
/// how far this stamp's foliage grows, in the absence of any richer
/// brush-shape concept.
///
/// `type_id` has no live equivalent in `FoliageComponent` beyond
/// `general.density_layer` (a numeric slice index into a density-texture
/// array, not a string identity -- see this module's top doc on the design
/// doc's `FoliageTypeComponent` mismatch). A numeric `type_id` sets that
/// slice; anything else leaves the default slice (0). Multi-type palettes
/// are explicitly out of scope for this milestone (design doc §6 / issue
/// #713), so this is a deliberately narrow bridge, not a real type system.
pub fn build_component(brush: &FoliageBrush, hit_position_m: [f32; 3]) -> FoliageComponent {
    let mut foliage = FoliageComponent::default();

    // General
    foliage.general.enabled = true;
    foliage.general.density = brush.density.max(0.0);
    if let Ok(layer) = brush.type_id.parse::<u64>() {
        foliage.general.density_layer = layer;
    }

    // Placement
    let (slope_lo, slope_hi) = brush.slope_limit;
    foliage.placement.slope_min_degrees = slope_lo.min(slope_hi).clamp(0.0, 90.0);
    foliage.placement.slope_max_degrees = slope_hi.max(slope_lo).clamp(0.0, 90.0);
    foliage.placement.layer_extent = brush.radius_m.max(1.0);
    let (height_lo, height_hi) = brush.height_range;
    foliage.placement.height_min = height_lo.min(height_hi).max(0.0);
    foliage.placement.height_max = height_hi.max(height_lo).max(0.0);
    let (width_lo, width_hi) = brush.width_range;
    foliage.placement.width_min = width_lo.min(width_hi).max(0.0);
    foliage.placement.width_max = width_hi.max(width_lo).max(0.0);
    recenter_altitude_band(&mut foliage, hit_position_m[1]);

    // Rendering
    foliage.rendering.two_sided = brush.two_sided;
    foliage.rendering.casts_shadow = brush.casts_shadow;
    foliage.rendering.roughness = brush.roughness.clamp(0.0, 1.0);
    foliage.rendering.metallic = brush.metallic.clamp(0.0, 1.0);
    foliage.rendering.lod_distance_0 = brush.lod_distance.max(0.0);

    // Wind
    foliage.wind.wind_enabled = brush.wind_enabled;
    foliage.wind.trunk_sway = brush.trunk_sway.max(0.0);
    foliage.wind.branch_flutter = brush.branch_flutter.max(0.0);
    foliage.wind.leaf_jitter = brush.leaf_jitter.max(0.0);
    foliage.wind.wind_speed = brush.wind_speed.max(0.0);

    // Interaction
    foliage.interaction.interactor_radius = brush.interactor_radius.max(0.0);

    foliage
}

/// Recenter the component's world-altitude acceptance band on the hit point.
///
/// `PlacementFoliageProps::altitude_min/max` are literal world-Y bounds, not
/// an offset from the owning object: `runtime.rs`'s `sync_component` passes
/// them straight into `FoliageTypeComponent::altitude_range` and
/// `FoliageLayerComponent::bounds_min/max[1]`. The component's own defaults
/// (-20.0 / 50.0) implicitly assume the terrain sits near world Y = 0, which
/// holds for a freshly created flat world but not for a planet's surface,
/// whose hit points sit near the planet's radius. Left at the defaults, a
/// foliage stamp painted on a planet would frequently self-exclude: the
/// object is created, but its own altitude band immediately rejects it, so
/// nothing renders and there is no visible feedback that anything is wrong.
/// Recentering on the hit keeps the same vertical window size the component
/// ships with, just anchored to where the brush actually stamped -- this
/// applies uniformly to planets and flat volumes alike, with no branch on
/// target kind (matches `sculpt.rs`'s "the body answers" pattern).
fn recenter_altitude_band(foliage: &mut FoliageComponent, hit_y: f32) {
    foliage.placement.altitude_min = hit_y - DEFAULT_ALTITUDE_BELOW_M;
    foliage.placement.altitude_max = hit_y + DEFAULT_ALTITUDE_ABOVE_M;
}

/// Build the scene object one foliage stamp should add.
///
/// A fresh `Empty` object per stamp, named after the brush's `type_id` and
/// positioned at the hit, carrying exactly one `FoliageComponent` instance.
///
/// # Why create a new object per stamp, instead of updating a nearby one
///
/// Two choices were on the table (design doc leaves it open). Creating a new
/// object per (coalesced) stamp was chosen because:
///
/// - It mirrors the sculpt brush's own model: each committed stamp is one
///   discrete, independently undoable unit (`TerrainUndoDomain` records one
///   op per stamp; here, `execute_command` records one `AddObject` per
///   stamp), rather than an accumulating mutation of shared state.
/// - "Update a nearby one" needs a spatial nearest-neighbor query over scene
///   objects that does not exist anywhere in `SceneDatabase` today; building
///   one would be new infrastructure well beyond "wire the brush to a
///   stamp", and would still need its own answer for *which* object counts
///   as "nearby" relative to `radius_m`.
/// - Coalescing (`sculpt::should_stamp`, reused as-is) already bounds stamp
///   density along a drag the same way it bounds sculpt stamps, so dragging
///   does not flood the scene the way stamping on every pointer-move would.
///
/// The known trade-off: overlapping stamps each contribute their own
/// `FoliageLayerComponent`-equivalent layer, so densities in the overlap
/// region are not deduplicated -- two overlapping stamps can render denser
/// grass in their shared area than either alone. That is a rendering-side
/// concern in the existing (untouched) GPU foliage stack, not something this
/// milestone's authoring change introduces control over; it is called out
/// here rather than silently accepted.
pub fn stamp_object_data(brush: &FoliageBrush, hit: &TerrainHit) -> SceneObjectData {
    let foliage = build_component(brush, hit.position_m);
    let data = serde_json::to_value(&foliage)
        .expect("FoliageComponent always serializes: plain numeric/bool/string leaves");

    SceneObjectData {
        id: String::new(),
        name: format!("Foliage ({})", brush.type_id),
        object_type: ObjectType::Empty,
        transform: Transform {
            position: hit.position_m,
            ..Transform::default()
        },
        visible: true,
        locked: false,
        parent: None,
        children: Vec::new(),
        scene_path: String::new(),
        props: Default::default(),
        component_instances: Some(serde_json::json!([
            {
                "class_name": "FoliageComponent",
                "enabled": true,
                "data": data,
            }
        ])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::services::terrain_edit::{PlanetId, TerrainTarget};

    fn brush() -> FoliageBrush {
        FoliageBrush {
            type_id: "default_grass".to_string(),
            density: 512.0,
            radius_m: 12.0,
            slope_limit: (5.0, 40.0),
            ..FoliageBrush::default()
        }
    }

    fn hit(position_m: [f32; 3]) -> TerrainHit {
        TerrainHit {
            target: TerrainTarget::Planet(PlanetId::from_stable_name("test")),
            position_m,
            cell: [0, 0, 0],
            normal: [0.0, 1.0, 0.0],
            material: 1,
            distance_m: 10.0,
        }
    }

    #[test]
    fn brush_density_and_slope_limit_flow_into_the_component() {
        let foliage = build_component(&brush(), [0.0, 100.0, 0.0]);
        assert_eq!(foliage.general.density, 512.0);
        assert_eq!(foliage.placement.slope_min_degrees, 5.0);
        assert_eq!(foliage.placement.slope_max_degrees, 40.0);
        assert_eq!(foliage.placement.layer_extent, 12.0);
    }

    #[test]
    fn an_inverted_slope_limit_is_not_carried_through_inverted() {
        let mut inverted = brush();
        inverted.slope_limit = (40.0, 5.0);
        let foliage = build_component(&inverted, [0.0, 100.0, 0.0]);
        assert_eq!(foliage.placement.slope_min_degrees, 5.0);
        assert_eq!(foliage.placement.slope_max_degrees, 40.0);
    }

    #[test]
    fn a_numeric_type_id_selects_the_density_layer_slice() {
        let mut numbered = brush();
        numbered.type_id = "3".to_string();
        let foliage = build_component(&numbered, [0.0, 100.0, 0.0]);
        assert_eq!(foliage.general.density_layer, 3);
    }

    #[test]
    fn a_non_numeric_type_id_leaves_the_default_density_layer() {
        let foliage = build_component(&brush(), [0.0, 100.0, 0.0]);
        assert_eq!(foliage.general.density_layer, 0);
    }

    /// The whole point of this module: a planet-altitude hit must not land
    /// outside its own component's altitude acceptance band, or the stamped
    /// foliage would exist but never render (see `recenter_altitude_band`'s
    /// doc).
    #[test]
    fn the_altitude_band_is_recentered_on_a_planet_scale_hit_point() {
        let foliage = build_component(&brush(), [0.0, 6_378_100.0, 0.0]);
        assert!(foliage.placement.altitude_min <= 6_378_100.0);
        assert!(foliage.placement.altitude_max >= 6_378_100.0);
        assert_eq!(foliage.placement.altitude_min, 6_378_100.0 - 20.0);
        assert_eq!(foliage.placement.altitude_max, 6_378_100.0 + 50.0);
    }

    #[test]
    fn stamp_object_data_carries_exactly_one_foliage_component_at_the_hit_point() {
        let data = stamp_object_data(&brush(), &hit([12.0, 34.0, -5.0]));
        assert_eq!(data.transform.position, [12.0, 34.0, -5.0]);
        assert_eq!(data.object_type, ObjectType::Empty);

        let instances = data
            .component_instances
            .as_ref()
            .and_then(|v| v.as_array())
            .expect("component_instances must be a JSON array");
        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0]["class_name"], "FoliageComponent");
        assert_eq!(instances[0]["enabled"], true);

        // The decisive round-trip: `data` must be the NESTED shape
        // `FoliageComponent`'s own `Deserialize` expects (see module doc),
        // not the flat `to_scene_props` shape. If this ever regresses to
        // flat, `from_value` fails and this test catches it immediately.
        let component: FoliageComponent =
            serde_json::from_value(instances[0]["data"].clone())
                .expect("stamped component data must deserialize as nested FoliageComponent");
        assert_eq!(component.general.density, 512.0);
        assert_eq!(component.placement.slope_min_degrees, 5.0);
    }

    // ── Undo: the scene-command snapshot path, not `TerrainUndoDomain` ─────
    //
    // Milestone 4's whole undo requirement (issue #713): a painted foliage
    // object must undo/redo through the *existing* `SceneCommand`/
    // `execute_command` snapshot-undo path, the same one every other
    // `AddObject` already uses -- NOT through `TerrainUndoDomain`, which is
    // specifically for voxel mutations. These tests exercise the exact call
    // a foliage stamp makes (`execute_command` with the object this module
    // builds) and additionally assert `TerrainUndoDomain` never sees it, so
    // a regression that accidentally routes foliage through terrain-stroke
    // undo would fail loudly here instead of only showing up as a UX
    // surprise later.

    use crate::level_editor::core::commands::{execute_command, SceneCommand};
    use crate::level_editor::state::LevelEditorState;

    #[test]
    fn a_painted_foliage_object_undoes_and_redoes_through_scene_command_undo() {
        let mut state = LevelEditorState::new();
        assert!(!state.scene.can_undo());

        let data = stamp_object_data(&brush(), &hit([5.0, 0.0, 5.0]));
        let result = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data,
                parent_id: None,
            },
        );
        assert!(result.changed, "the foliage AddObject must not be a no-op");
        assert_eq!(state.scene.database.get_all_objects().len(), 1);

        // It went through the ordinary undo-tracked command path.
        assert!(state.scene.can_undo());
        // And specifically NOT through the terrain-stroke history: foliage
        // painting mutates no voxels, so nothing should ever land here.
        assert!(
            !state.editor.terrain_undo.can_undo(),
            "foliage painting must never open a TerrainUndoDomain stroke"
        );

        assert!(state.scene.undo());
        assert!(state.scene.database.get_all_objects().is_empty());

        assert!(state.scene.redo());
        let objects = state.scene.database.get_all_objects();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].name, "Foliage (default_grass)");
    }
}
