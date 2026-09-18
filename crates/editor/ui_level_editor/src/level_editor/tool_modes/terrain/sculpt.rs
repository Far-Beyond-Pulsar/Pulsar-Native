//! Brush math: a terrain hit plus brush settings become one `EditOp` stamp.
//!
//! A brush is a primitive placed relative to the surface and combined with the
//! terrain field by one of four [`EditMode`]s. Everything a sculpt brush
//! expresses is therefore *where the primitive is placed and how big it is*:
//!
//! | Brush | Mode | Placement |
//! |---|---|---|
//! | Raise | `Union` | sunk below the surface so it protrudes by `strength` |
//! | Lower | `Subtract` | raised above the surface so it bites in by `strength` |
//! | Flatten | `Replace` | anchored to the altitude sampled at stroke start |
//! | Paint | `Paint` | centred on the hit; only the material channel moves |
//!
//! # Nothing here knows whether it is sculpting a planet or a flat world
//!
//! Everything shape-dependent is asked of the [`TerrainBodyDefinition`]:
//! "height" (`altitude_m`), "the point at that height" (`point_at_altitude_m`),
//! and "the primitive that levels this body" (`flatten_shape`). That is the
//! whole difference, and it lives in `pulsar_terrain` where the shape axis is
//! defined — this module has no branch on target kind at all.

use engine_backend::services::terrain_edit::{
    meters_to_cell, meters_to_radius_cells, EditMode, EditOp, EditShape, TerrainBodyDefinition,
    TerrainHit, LOD0_CELL_SIZE_METERS,
};

use crate::level_editor::state::terrain::{SculptBrush, SculptMode};

/// How far the brush must travel before the next stamp is emitted, as a
/// fraction of the brush radius. A drag that does not clear this keeps the
/// pointer event but commits nothing, which is what keeps a slow drag from
/// flooding the mutation log with near-identical ops (design doc §5.3).
const COALESCE_FRACTION: f32 = 0.25;

/// How much of the radius a fully soft brush gives up.
///
/// `EditShape::Sphere` has a hard boundary — there is no per-cell falloff
/// weight in the canonical edit format — so falloff can only be approximated
/// by depositing less per stamp. Real feathering needs a new `EditShape`
/// variant carrying a falloff curve, which is out of scope here.
const FALLOFF_RADIUS_GIVEUP: f32 = 0.4;

/// Whether the brush has moved far enough from its last stamp to stamp again.
pub fn should_stamp(last_center_m: Option<[f32; 3]>, center_m: [f32; 3], radius_m: f32) -> bool {
    let Some(last) = last_center_m else {
        return true;
    };
    let delta = [
        center_m[0] - last[0],
        center_m[1] - last[1],
        center_m[2] - last[2],
    ];
    let travelled_squared = delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2];
    let threshold = radius_m * COALESCE_FRACTION;
    travelled_squared >= threshold * threshold
}

/// Height of a world point above the body's datum, in meters.
///
/// For a planet that is the distance from its centre; for a flat world it is
/// the offset from its ground plane. The body answers, so a Flatten stroke
/// means the same thing on both.
pub fn altitude_m(definition: &TerrainBodyDefinition, point_m: [f32; 3]) -> f64 {
    definition.altitude_m([
        f64::from(point_m[0]),
        f64::from(point_m[1]),
        f64::from(point_m[2]),
    ])
}

/// One brush stamp, ready to hand to `TerrainEditApi::apply_edit`.
///
/// `op`'s `sequence` and `stable_id` are placeholders: the edit seam owns
/// mutation ordering and rewrites both when it commits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SculptStamp {
    pub op: EditOp,
    /// Where the stamp's primitive was centred, in world meters. Coalescing
    /// compares against the *hit* point rather than this, so it is carried
    /// separately for the brush cursor and for diagnostics.
    pub sphere_center_m: [f64; 3],
}

/// Build the stamp for one brush application.
///
/// `anchor_altitude_m` is the altitude captured when the stroke opened; it is
/// only consulted by [`SculptMode::Flatten`], which anchors every stamp in a
/// stroke to it so a flatten drag converges on one level rather than chasing
/// the surface it is already modifying.
pub fn build_stamp(
    hit: &TerrainHit,
    definition: &TerrainBodyDefinition,
    brush: &SculptBrush,
    anchor_altitude_m: Option<f64>,
) -> SculptStamp {
    let radius_m = effective_radius_m(brush);
    let radius_cells = meters_to_radius_cells(radius_m);
    // A stamp can never displace more than its own radius: pushing further
    // would detach the sphere from the surface and leave a floating blob.
    let strength_m = f64::from(brush.strength.clamp(0.0, radius_m));
    let radius_m = f64::from(radius_m);

    let hit_point = [
        f64::from(hit.position_m[0]),
        f64::from(hit.position_m[1]),
        f64::from(hit.position_m[2]),
    ];
    let normal = [
        f64::from(hit.normal[0]),
        f64::from(hit.normal[1]),
        f64::from(hit.normal[2]),
    ];
    let (mode, sphere_center) = match brush.mode {
        // Sink the sphere so exactly `strength` metres of it stand proud of
        // the surface, then union it in.
        SculptMode::Raise => (
            EditMode::Union,
            offset_along(hit_point, normal, -(radius_m - strength_m)),
        ),
        // Mirror image: lift it so it bites `strength` metres down.
        SculptMode::Lower => (
            EditMode::Subtract,
            offset_along(hit_point, normal, radius_m - strength_m),
        ),
        // Put the primitive's outer surface at the stroke's anchor altitude,
        // so every stamp in the stroke resolves to the same level.
        SculptMode::Flatten => {
            let anchor = anchor_altitude_m.unwrap_or_else(|| altitude_m(definition, hit.position_m));
            (
                EditMode::Replace,
                definition.point_at_altitude_m(hit_point, normal, anchor - radius_m),
            )
        }
        // Material-only: leave the density field where it is.
        SculptMode::Paint => (EditMode::Paint, hit_point),
    };

    let center_cell = meters_to_cell(sphere_center);
    // Only levelling needs a primitive whose top is flat with respect to the
    // body's datum; every other brush is a round stroke on either shape.
    let shape = if matches!(brush.mode, SculptMode::Flatten) {
        definition.flatten_shape(center_cell, radius_cells)
    } else {
        EditShape::Sphere {
            center_cell,
            radius_cells,
        }
    };

    SculptStamp {
        op: EditOp {
            sequence: 0,
            stable_id: [0; 16],
            shape,
            mode,
            material: brush_material(brush),
        },
        sphere_center_m: sphere_center,
    }
}

/// The brush radius actually stamped, after falloff.
pub fn effective_radius_m(brush: &SculptBrush) -> f32 {
    let falloff = brush.falloff.clamp(0.0, 1.0);
    (brush.radius_m * (1.0 - FALLOFF_RADIUS_GIVEUP * falloff))
        .max(LOD0_CELL_SIZE_METERS as f32)
}

/// Material written by a stamp.
///
/// `EditOp::material` is a `u8`; the brush stores a `u32` so the UI can offer
/// a wider palette later. Material 0 means air, so a brush configured with it
/// would paint holes — clamp up to 1 instead of silently erasing.
fn brush_material(brush: &SculptBrush) -> u8 {
    u8::try_from(brush.material).unwrap_or(u8::MAX).max(1)
}

fn offset_along(point: [f64; 3], direction: [f64; 3], distance: f64) -> [f64; 3] {
    [
        point[0] + direction[0] * distance,
        point[1] + direction[1] * distance,
        point[2] + direction[2] * distance,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine_backend::services::terrain_edit::{
        FlatTerrain, PlanetDefinition, PlanetId, TerrainTarget, VolumeDefinition, VolumeId,
    };

    fn definition() -> TerrainBodyDefinition {
        TerrainBodyDefinition::Planet(PlanetDefinition {
            planet_id: PlanetId::from_stable_name("test"),
            center_cell: [0; 3],
            // 100 m radius.
            radius_cells: 1_000,
            material: 1,
            root_lod: 12,
            max_resident_pages: 64,
        })
    }

    /// A flat world whose ground plane sits at y = 0.
    fn flat_definition() -> TerrainBodyDefinition {
        TerrainBodyDefinition::Volume(VolumeDefinition {
            volume_id: VolumeId::from_stable_name("flat"),
            flat: FlatTerrain::centered_on([0; 3]),
            material: 1,
            root_lod: 12,
            max_resident_pages: 4_096,
        })
    }

    /// A hit on the ground plane of the flat world, 20 m from its centre.
    fn flat_hit() -> TerrainHit {
        TerrainHit {
            target: TerrainTarget::Volume(VolumeId::from_stable_name("flat")),
            position_m: [20.0, 0.0, -5.0],
            cell: [200, 0, -50],
            normal: [0.0, 1.0, 0.0],
            material: 1,
            distance_m: 30.0,
        }
    }

    /// A hit on the +Y pole of the test planet.
    fn hit() -> TerrainHit {
        TerrainHit {
            target: TerrainTarget::Planet(PlanetId::from_stable_name("test")),
            position_m: [0.0, 100.0, 0.0],
            cell: [0, 1_000, 0],
            normal: [0.0, 1.0, 0.0],
            material: 1,
            distance_m: 50.0,
        }
    }

    fn brush(mode: SculptMode) -> SculptBrush {
        SculptBrush {
            mode,
            radius_m: 8.0,
            falloff: 0.0,
            strength: 2.0,
            material: 3,
        }
    }

    #[test]
    fn raise_sinks_the_sphere_so_it_protrudes_by_the_brush_strength() {
        let stamp = build_stamp(&hit(), &definition(), &brush(SculptMode::Raise), None);
        assert_eq!(stamp.op.mode, EditMode::Union);
        // radius 8, strength 2 => centre 6 m below the surface, top at 102 m.
        assert!((stamp.sphere_center_m[1] - 94.0).abs() < 1e-6, "{stamp:?}");
    }

    #[test]
    fn lower_lifts_the_sphere_so_it_bites_in_by_the_brush_strength() {
        let stamp = build_stamp(&hit(), &definition(), &brush(SculptMode::Lower), None);
        assert_eq!(stamp.op.mode, EditMode::Subtract);
        assert!((stamp.sphere_center_m[1] - 106.0).abs() < 1e-6, "{stamp:?}");
    }

    #[test]
    fn flatten_anchors_every_stamp_to_the_stroke_start_altitude() {
        let anchor = Some(100.0_f64);
        let first = build_stamp(&hit(), &definition(), &brush(SculptMode::Flatten), anchor);
        // A later stamp in the same stroke, after the surface has risen.
        let raised = TerrainHit {
            position_m: [0.0, 140.0, 0.0],
            ..hit()
        };
        let second = build_stamp(&raised, &definition(), &brush(SculptMode::Flatten), anchor);
        assert_eq!(first.op.mode, EditMode::Replace);
        assert_eq!(
            first.sphere_center_m, second.sphere_center_m,
            "flatten must not chase the surface it is levelling"
        );
        // Sphere top sits at the anchor altitude: centre = 100 - 8.
        assert!((first.sphere_center_m[1] - 92.0).abs() < 1e-6, "{first:?}");
    }

    #[test]
    fn flatten_without_an_anchor_falls_back_to_the_current_altitude() {
        let stamp = build_stamp(&hit(), &definition(), &brush(SculptMode::Flatten), None);
        assert!((stamp.sphere_center_m[1] - 92.0).abs() < 1e-6, "{stamp:?}");
    }

    #[test]
    fn paint_centres_on_the_hit_and_only_carries_material() {
        let stamp = build_stamp(&hit(), &definition(), &brush(SculptMode::Paint), None);
        assert_eq!(stamp.op.mode, EditMode::Paint);
        assert_eq!(stamp.sphere_center_m, [0.0, 100.0, 0.0]);
        assert_eq!(stamp.op.material, 3);
    }

    #[test]
    fn a_strength_larger_than_the_radius_cannot_detach_the_stamp() {
        let mut brush = brush(SculptMode::Raise);
        brush.strength = 1_000.0;
        let stamp = build_stamp(&hit(), &definition(), &brush, None);
        // Clamped to the radius: the sphere centre lands exactly on the hit.
        assert!((stamp.sphere_center_m[1] - 100.0).abs() < 1e-6, "{stamp:?}");
    }

    #[test]
    fn falloff_shrinks_the_stamped_radius_without_ever_reaching_zero() {
        let mut soft = brush(SculptMode::Raise);
        soft.falloff = 1.0;
        let hard = brush(SculptMode::Raise);
        assert!(effective_radius_m(&soft) < effective_radius_m(&hard));
        assert!(effective_radius_m(&soft) > 0.0);

        let mut degenerate = brush(SculptMode::Raise);
        degenerate.radius_m = 0.0;
        degenerate.falloff = 1.0;
        assert!(effective_radius_m(&degenerate) > 0.0);
    }

    #[test]
    fn a_brush_never_paints_air_by_accident() {
        let mut brush = brush(SculptMode::Paint);
        brush.material = 0;
        assert_eq!(build_stamp(&hit(), &definition(), &brush, None).op.material, 1);
        brush.material = 9_999;
        assert_eq!(
            build_stamp(&hit(), &definition(), &brush, None).op.material,
            u8::MAX
        );
    }

    #[test]
    fn the_first_stamp_of_a_stroke_always_commits() {
        assert!(should_stamp(None, [0.0, 0.0, 0.0], 8.0));
    }

    #[test]
    fn a_drag_shorter_than_a_quarter_radius_coalesces_away() {
        let last = Some([0.0, 0.0, 0.0]);
        assert!(!should_stamp(last, [1.0, 0.0, 0.0], 8.0));
        assert!(should_stamp(last, [3.0, 0.0, 0.0], 8.0));
    }

    #[test]
    fn a_smaller_brush_stamps_more_often_over_the_same_travel() {
        let last = Some([0.0, 0.0, 0.0]);
        let moved = [1.0, 0.0, 0.0];
        assert!(!should_stamp(last, moved, 8.0));
        assert!(should_stamp(last, moved, 2.0));
    }

    #[test]
    fn altitude_is_measured_from_the_planet_centre() {
        assert!((altitude_m(&definition(), [0.0, 100.0, 0.0]) - 100.0).abs() < 1e-9);
        assert!((altitude_m(&definition(), [30.0, 40.0, 0.0]) - 50.0).abs() < 1e-9);
    }

    // ── The same pipeline, against a flat volume ───────────────────────────
    //
    // These are deliberately the *same* assertions as the planet cases above,
    // run through the same `build_stamp` with only the body swapped. That is
    // the milestone's acceptance bar: no branch on target kind anywhere above
    // `TerrainBodyDefinition`.

    #[test]
    fn raise_on_a_flat_world_sinks_the_stamp_the_same_way_it_does_on_a_planet() {
        let stamp = build_stamp(
            &flat_hit(),
            &flat_definition(),
            &brush(SculptMode::Raise),
            None,
        );
        assert_eq!(stamp.op.mode, EditMode::Union);
        // radius 8, strength 2 => centre 6 m below the ground plane.
        assert!((stamp.sphere_center_m[1] + 6.0).abs() < 1e-6, "{stamp:?}");
        assert!(matches!(stamp.op.shape, EditShape::Sphere { .. }));
    }

    #[test]
    fn lower_on_a_flat_world_lifts_the_stamp_by_the_brush_strength() {
        let stamp = build_stamp(
            &flat_hit(),
            &flat_definition(),
            &brush(SculptMode::Lower),
            None,
        );
        assert_eq!(stamp.op.mode, EditMode::Subtract);
        assert!((stamp.sphere_center_m[1] - 6.0).abs() < 1e-6, "{stamp:?}");
    }

    #[test]
    fn a_flat_worlds_altitude_is_its_height_above_the_ground_plane() {
        assert!((altitude_m(&flat_definition(), [20.0, 3.0, -5.0]) - 3.0).abs() < 1e-9);
        assert!((altitude_m(&flat_definition(), [999.0, -2.0, 0.0]) + 2.0).abs() < 1e-9);
    }

    /// A sphere's cap is level on a planet but domed on a flat world, so a
    /// flat world levels with a box instead. The brush code does not choose —
    /// the body does.
    #[test]
    fn flatten_uses_a_box_on_a_flat_world_and_a_sphere_on_a_planet() {
        let anchor = Some(4.0_f64);
        let flat = build_stamp(
            &flat_hit(),
            &flat_definition(),
            &brush(SculptMode::Flatten),
            anchor,
        );
        assert_eq!(flat.op.mode, EditMode::Replace);
        match flat.op.shape {
            EditShape::Box {
                half_extent_cells, ..
            } => assert_eq!(half_extent_cells, [80; 3], "8 m brush at 10 cm cells"),
            other => panic!("a flat world must level with a box, got {other:?}"),
        }
        // Box top face at the anchor: centre is 8 m (the radius) below it.
        assert!((flat.sphere_center_m[1] + 4.0).abs() < 1e-6, "{flat:?}");
        // The stamp stays under the cursor rather than moving to a datum.
        assert_eq!([flat.sphere_center_m[0], flat.sphere_center_m[2]], [20.0, -5.0]);

        let planet = build_stamp(&hit(), &definition(), &brush(SculptMode::Flatten), anchor);
        assert!(matches!(planet.op.shape, EditShape::Sphere { .. }));
    }

    #[test]
    fn flatten_on_a_flat_world_does_not_chase_the_surface_it_is_levelling() {
        let anchor = Some(0.0_f64);
        let first = build_stamp(
            &flat_hit(),
            &flat_definition(),
            &brush(SculptMode::Flatten),
            anchor,
        );
        let raised = TerrainHit {
            position_m: [20.0, 12.0, -5.0],
            ..flat_hit()
        };
        let second = build_stamp(
            &raised,
            &flat_definition(),
            &brush(SculptMode::Flatten),
            anchor,
        );
        assert_eq!(first.sphere_center_m[1], second.sphere_center_m[1]);
    }

    #[test]
    fn paint_on_a_flat_world_centres_on_the_hit_like_it_does_on_a_planet() {
        let stamp = build_stamp(
            &flat_hit(),
            &flat_definition(),
            &brush(SculptMode::Paint),
            None,
        );
        assert_eq!(stamp.op.mode, EditMode::Paint);
        assert_eq!(stamp.sphere_center_m, [20.0, 0.0, -5.0]);
        assert_eq!(stamp.op.material, 3);
    }
}
