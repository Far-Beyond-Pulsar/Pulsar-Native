//! Canonical page dependencies required to extract one smooth terrain surface.
//!
//! Pulsar owns these pages because they are ordinary canonical samples. Helio
//! may cache them for GPU extraction, but it must never synthesize a separate
//! halo or transition slab that canonical edits and persistence cannot see.

use crate::{PageKey, TerrainTransitionFace, PAGE_EDGE_CELLS, TERRAIN_TRANSITION_FACE_MASK};
use std::collections::BTreeSet;
use thiserror::Error;

const TRANSITION_FACE_SAMPLE_EDGE: i64 = PAGE_EDGE_CELLS * 2 + 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum TerrainSurfaceSamplingError {
    #[error("terrain surface transition mask {0:#010b} contains unsupported face bits")]
    TransitionMask(u8),
    #[error("LOD0 has no finer neighbor and cannot own a transition surface")]
    FinestLodTransition,
    #[error("terrain surface sampling dependency address overflows")]
    CoordinateOverflow,
}

/// Return the exact canonical page identities needed by the regular 34^3
/// extraction halo and every enabled fine-side 67x67x3 transition slab.
pub fn terrain_surface_required_pages(
    page: PageKey,
    transition_mask: u8,
) -> Result<BTreeSet<PageKey>, TerrainSurfaceSamplingError> {
    if transition_mask & !TERRAIN_TRANSITION_FACE_MASK != 0 {
        return Err(TerrainSurfaceSamplingError::TransitionMask(transition_mask));
    }
    if page.lod == 0 && transition_mask != 0 {
        return Err(TerrainSurfaceSamplingError::FinestLodTransition);
    }

    let mut pages = BTreeSet::new();
    let page_min = page
        .lod0_cell_min()
        .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
    let coarse_scale = 1_i64
        .checked_shl(u32::from(page.lod))
        .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
    let page_span = PAGE_EDGE_CELLS
        .checked_mul(coarse_scale)
        .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
    let regular_min = checked_axis_offset(page_min, [-coarse_scale; 3])?;
    let regular_max = checked_axis_offset(page_min, [page_span; 3])?;
    insert_page_box(page.lod, regular_min, regular_max, &mut pages)?;

    if transition_mask == 0 {
        return Ok(pages);
    }

    let fine_lod = page.lod - 1;
    let fine_scale = coarse_scale / 2;
    for face in TerrainTransitionFace::ALL {
        if transition_mask & face.bit() == 0 {
            continue;
        }
        let basis = transition_face_integer_basis(face);
        let mut minimum = [i64::MAX; 3];
        let mut maximum = [i64::MIN; 3];
        for u in [-1_i64, TRANSITION_FACE_SAMPLE_EDGE] {
            for v in [-1_i64, TRANSITION_FACE_SAMPLE_EDGE] {
                for outward in [-1_i64, 1] {
                    let mut position = page_min;
                    for axis in 0..3 {
                        let origin_offset = i64::from(basis.origin[axis])
                            .checked_mul(page_span)
                            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
                        let u_offset = i64::from(basis.u_axis[axis])
                            .checked_mul(u)
                            .and_then(|value| value.checked_mul(fine_scale))
                            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
                        let v_offset = i64::from(basis.v_axis[axis])
                            .checked_mul(v)
                            .and_then(|value| value.checked_mul(fine_scale))
                            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
                        let outward_offset = i64::from(basis.outward[axis])
                            .checked_mul(outward)
                            .and_then(|value| value.checked_mul(fine_scale))
                            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
                        position[axis] = position[axis]
                            .checked_add(origin_offset)
                            .and_then(|value| value.checked_add(u_offset))
                            .and_then(|value| value.checked_add(v_offset))
                            .and_then(|value| value.checked_add(outward_offset))
                            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?;
                        minimum[axis] = minimum[axis].min(position[axis]);
                        maximum[axis] = maximum[axis].max(position[axis]);
                    }
                }
            }
        }
        insert_page_box(fine_lod, minimum, maximum, &mut pages)?;
    }
    Ok(pages)
}

/// Union the extraction dependencies for a complete non-overlapping surface
/// frontier. Surface-owning pages are removed from the result so callers can
/// track sampling-only residency independently.
pub(crate) fn terrain_frontier_sampling_support(
    pages: impl IntoIterator<Item = PageKey>,
    transition_masks: &std::collections::BTreeMap<PageKey, u8>,
) -> Result<BTreeSet<PageKey>, TerrainSurfaceSamplingError> {
    let surfaces = pages.into_iter().collect::<BTreeSet<_>>();
    let mut support = BTreeSet::new();
    for page in &surfaces {
        support.extend(terrain_surface_required_pages(
            *page,
            transition_masks.get(page).copied().unwrap_or(0),
        )?);
    }
    support.retain(|page| !surfaces.contains(page));
    Ok(support)
}

fn checked_axis_offset(
    position: [i64; 3],
    offset: [i64; 3],
) -> Result<[i64; 3], TerrainSurfaceSamplingError> {
    Ok([
        position[0]
            .checked_add(offset[0])
            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?,
        position[1]
            .checked_add(offset[1])
            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?,
        position[2]
            .checked_add(offset[2])
            .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?,
    ])
}

fn insert_page_box(
    lod: u8,
    minimum: [i64; 3],
    maximum: [i64; 3],
    output: &mut BTreeSet<PageKey>,
) -> Result<(), TerrainSurfaceSamplingError> {
    let minimum_page = PageKey::address_lod0_cell(lod, minimum)
        .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?
        .0
        .page_xyz;
    let maximum_page = PageKey::address_lod0_cell(lod, maximum)
        .ok_or(TerrainSurfaceSamplingError::CoordinateOverflow)?
        .0
        .page_xyz;
    for z in minimum_page[2]..=maximum_page[2] {
        for y in minimum_page[1]..=maximum_page[1] {
            for x in minimum_page[0]..=maximum_page[0] {
                output.insert(PageKey::new(lod, [x, y, z]));
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct IntegerFaceBasis {
    origin: [i8; 3],
    u_axis: [i8; 3],
    v_axis: [i8; 3],
    outward: [i8; 3],
}

const fn transition_face_integer_basis(face: TerrainTransitionFace) -> IntegerFaceBasis {
    match face {
        TerrainTransitionFace::NegativeX => IntegerFaceBasis {
            origin: [0, 0, 1],
            u_axis: [0, 1, 0],
            v_axis: [0, 0, -1],
            outward: [-1, 0, 0],
        },
        TerrainTransitionFace::PositiveX => IntegerFaceBasis {
            origin: [1, 0, 0],
            u_axis: [0, 1, 0],
            v_axis: [0, 0, 1],
            outward: [1, 0, 0],
        },
        TerrainTransitionFace::NegativeY => IntegerFaceBasis {
            origin: [1, 0, 0],
            u_axis: [0, 0, 1],
            v_axis: [-1, 0, 0],
            outward: [0, -1, 0],
        },
        TerrainTransitionFace::PositiveY => IntegerFaceBasis {
            origin: [0, 1, 0],
            u_axis: [0, 0, 1],
            v_axis: [1, 0, 0],
            outward: [0, 1, 0],
        },
        TerrainTransitionFace::NegativeZ => IntegerFaceBasis {
            origin: [0, 1, 0],
            u_axis: [1, 0, 0],
            v_axis: [0, -1, 0],
            outward: [0, 0, -1],
        },
        TerrainTransitionFace::PositiveZ => IntegerFaceBasis {
            origin: [0, 0, 1],
            u_axis: [1, 0, 0],
            v_axis: [0, 1, 0],
            outward: [0, 0, 1],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_halo_is_exact_and_signed_boundary_safe() {
        let page = PageKey::new(3, [-3, 4, -5]);
        let required = terrain_surface_required_pages(page, 0).unwrap();
        assert_eq!(required.len(), 27);
        assert!(required.contains(&PageKey::new(3, [-4, 3, -6])));
        assert!(required.contains(&page));
        assert!(required.contains(&PageKey::new(3, [-2, 5, -4])));
    }

    #[test]
    fn transition_support_includes_fine_pages_and_rejects_invalid_requests() {
        let page = PageKey::new(2, [-1, 0, 1]);
        let required = terrain_surface_required_pages(
            page,
            TerrainTransitionFace::PositiveX.bit() | TerrainTransitionFace::NegativeZ.bit(),
        )
        .unwrap();
        assert!(required.iter().any(|key| key.lod == 1));
        assert!(required.iter().any(|key| key.lod == 2));
        assert!(matches!(
            terrain_surface_required_pages(PageKey::new(0, [0; 3]), 1),
            Err(TerrainSurfaceSamplingError::FinestLodTransition)
        ));
        assert!(matches!(
            terrain_surface_required_pages(page, 0b1000_0000),
            Err(TerrainSurfaceSamplingError::TransitionMask(0b1000_0000))
        ));
        assert!(matches!(
            terrain_surface_required_pages(PageKey::new(62, [i64::MAX, 0, 0]), 0),
            Err(TerrainSurfaceSamplingError::CoordinateOverflow)
        ));
    }
}
