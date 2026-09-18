//! The terrain **shape axis**: what kind of body a set of canonical cells
//! describes.
//!
//! Everything above this module — the sparse hierarchy, page encoding, edit
//! log, snapshots, streaming, the render delta — addresses terrain as signed
//! density in canonical LOD0 cells and does not care what the cells outline.
//! Shape enters in exactly two places: the deterministic generator that says
//! which cells start solid, and the analytic ray intersection an authoring
//! front-end uses to find the surface. This module is the axis that names the
//! difference so neither of those has to be duplicated.
//!
//! A **volume** is deliberately *not* a second kind of runtime. It is
//! registered in the same body table, keyed by the same 16-byte identity, and
//! carries the same `root_lod`/`max_resident_pages` contract as a planet, so
//! there is exactly one code path from an [`crate::EditOp`] to a resident page
//! regardless of which one is being sculpted.

use crate::generator::{FixedSphereGenerator, FixedVolumeGenerator, TerrainGenerator};
use crate::{
    EditShape, MaterialId, PlanetDefinition, PlanetId, PlanetIdParseError, LOD0_CELL_SIZE_METERS,
};

/// How a terrain body's cells are laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TerrainShape {
    /// A spherical body: solid inside a radius about a centre cell.
    Planet,
    /// A finite, axis-aligned flat world: solid inside a box, below a ground
    /// plane.
    Volume,
}

/// Identity of a flat voxel volume.
///
/// Volumes live in the same identity space as planets — a volume *is* a body
/// in the runtime's table — but the newtype keeps the two from being confused
/// at API boundaries where only one of them is meaningful.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct VolumeId(pub PlanetId);

impl VolumeId {
    pub fn from_stable_name(name: &str) -> Self {
        Self(PlanetId::from_stable_name(name))
    }

    pub fn from_hex(value: &str) -> Result<Self, PlanetIdParseError> {
        PlanetId::from_hex(value).map(Self)
    }

    pub fn to_hex(self) -> String {
        self.0.to_hex()
    }

    /// The body identity this volume is registered under.
    pub const fn body_id(self) -> PlanetId {
        self.0
    }
}

/// A finite, axis-aligned flat voxel world.
///
/// `extent` is the **half**-extent from `origin` along each axis in canonical
/// LOD0 cells, so an axis spans `2 * extent + 1` cells. `origin` is the cell at
/// the centre of the world's ground plane: cells at or below `origin[1]`
/// (and inside the box) start solid, everything above starts air. That is the
/// generator's starting state only — sculpting moves the surface anywhere
/// inside the box.
///
/// ## Why the extent is `i16`
///
/// `i16` tops out at 32 767 cells, which is ±3.28 km per axis at the canonical
/// 10 cm cell size. That is far past the point where LOD streaming has to
/// carry the world anyway: see [`FlatTerrain::lod0_surface_page_count`] and
/// this module's tests for the measured budget. Widening the field would not
/// buy a bigger *resident* world, only a bigger address range, so the narrow
/// type is a deliberate statement that the extent is not the binding limit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlatTerrain {
    /// Edge length of one voxel, in meters.
    ///
    /// This must be the canonical LOD0 cell size: `PageKey`, `EditShape` and
    /// `PlanetPosition` are all *defined* in 10 cm cells, so a volume with a
    /// different voxel size would not be addressable by the shared machinery
    /// this whole design exists to reuse. The field is carried (and validated
    /// by [`VolumeDefinition::validate`]) rather than assumed, so a future
    /// coarse-cell volume is a loud error here instead of a silently wrong
    /// world.
    pub cell_size_m: f64,
    /// Half-extent from `origin` on each axis, in LOD0 cells.
    pub extent: (i16, i16, i16),
    /// Canonical LOD0 cell at the centre of the ground plane.
    pub origin: [i64; 3],
}

impl FlatTerrain {
    /// Half-extent of the flat world a "create flat world" action makes.
    ///
    /// 1024 cells is ±102.4 m, a 204.9 m square. Its LOD0 surface is 65×65
    /// pages — 528 MiB if every one of them were resident at once, which is
    /// inside the live runtime's 8192-page / 1 GiB residency cap, and far
    /// inside it once LOD refinement holds only the pages near the camera.
    pub const DEFAULT_HALF_EXTENT_CELLS: i16 = 1_024;

    /// A default flat world centred on `origin`.
    pub fn centered_on(origin: [i64; 3]) -> Self {
        Self {
            cell_size_m: LOD0_CELL_SIZE_METERS,
            extent: (
                Self::DEFAULT_HALF_EXTENT_CELLS,
                Self::DEFAULT_HALF_EXTENT_CELLS,
                Self::DEFAULT_HALF_EXTENT_CELLS,
            ),
            origin,
        }
    }

    /// Half-extent as canonical cells, widened for arithmetic.
    pub fn half_extent_cells(&self) -> [i64; 3] {
        [
            i64::from(self.extent.0),
            i64::from(self.extent.1),
            i64::from(self.extent.2),
        ]
    }

    /// Inclusive canonical LOD0 cell bounds of the volume.
    pub fn cell_bounds(&self) -> ([i64; 3], [i64; 3]) {
        let half = self.half_extent_cells();
        (
            std::array::from_fn(|axis| self.origin[axis].saturating_sub(half[axis])),
            std::array::from_fn(|axis| self.origin[axis].saturating_add(half[axis])),
        )
    }

    /// Extent along each axis in meters.
    pub fn span_m(&self) -> [f64; 3] {
        self.half_extent_cells()
            .map(|half| (2 * half + 1) as f64 * self.cell_size_m)
    }

    /// How many LOD0 pages the volume's *ground sheet* covers.
    ///
    /// This, not the cell count, is the number that decides whether a flat
    /// world needs LOD streaming: terrain is a surface, and only pages the
    /// surface passes through are ever built. Multiply by
    /// `CELL_COUNT * 4` bytes for the dense residency cost.
    pub fn lod0_surface_page_count(&self) -> u128 {
        let half = self.half_extent_cells();
        [0_usize, 2]
            .into_iter()
            .map(|axis| {
                let span = (2 * half[axis] + 1) as u128;
                span.div_ceil(crate::PAGE_EDGE_CELLS as u128)
            })
            .product()
    }
}

/// Everything the runtime needs to register a flat voxel world.
///
/// Mirrors [`PlanetDefinition`] field for field where the meaning is the same,
/// because it is registered through the same door.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeDefinition {
    pub volume_id: VolumeId,
    pub flat: FlatTerrain,
    pub material: MaterialId,
    pub root_lod: u8,
    pub max_resident_pages: usize,
}

impl VolumeDefinition {
    /// Whether every cell the volume can address fits the centered sparse
    /// hierarchy root named by `root_lod`. Same contract as
    /// [`PlanetDefinition::fits_centered_root`].
    pub fn fits_centered_root(&self) -> bool {
        if !(1..=62).contains(&self.root_lod) {
            return false;
        }
        let half_span = i128::from(crate::PAGE_EDGE_CELLS) << (self.root_lod - 1);
        let (min, max) = self.flat.cell_bounds();
        (0..3).all(|axis| {
            i128::from(min[axis]) >= -half_span && i128::from(max[axis]) < half_span
        })
    }

    /// Structural validity, independent of any runtime capacity limit.
    pub fn validate(&self) -> Result<(), VolumeDefinitionError> {
        if self.flat.cell_size_m != LOD0_CELL_SIZE_METERS {
            return Err(VolumeDefinitionError::CellSize(self.flat.cell_size_m));
        }
        let extent = self.flat.half_extent_cells();
        if extent.iter().any(|axis| *axis <= 0) {
            return Err(VolumeDefinitionError::Extent(self.flat.extent));
        }
        if self.material == 0 {
            return Err(VolumeDefinitionError::Material);
        }
        if !self.fits_centered_root() {
            return Err(VolumeDefinitionError::RootLod(self.root_lod));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum VolumeDefinitionError {
    #[error("a volume's cell size must be the canonical {LOD0_CELL_SIZE_METERS} m, got {0}")]
    CellSize(f64),
    #[error("a volume's half-extent must be positive on every axis, got {0:?}")]
    Extent((i16, i16, i16)),
    #[error("material 0 is air; a volume must be generated from a solid material")]
    Material,
    #[error("a volume does not fit the centered hierarchy root at lod {0}")]
    RootLod(u8),
}

/// A registered terrain body: one planet or one flat volume.
///
/// This is the shape axis made concrete. Code that does not care about shape
/// takes a `TerrainBodyDefinition` and uses the accessors below; only the
/// generator and the analytic hit test ever match on the variant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerrainBodyDefinition {
    Planet(PlanetDefinition),
    Volume(VolumeDefinition),
}

impl TerrainBodyDefinition {
    pub fn shape(&self) -> TerrainShape {
        match self {
            Self::Planet(_) => TerrainShape::Planet,
            Self::Volume(_) => TerrainShape::Volume,
        }
    }

    /// The identity this body is registered under. Planets and volumes share
    /// one identity space precisely so everything downstream stays one path.
    pub fn body_id(&self) -> PlanetId {
        match self {
            Self::Planet(definition) => definition.planet_id,
            Self::Volume(definition) => definition.volume_id.body_id(),
        }
    }

    pub fn material(&self) -> MaterialId {
        match self {
            Self::Planet(definition) => definition.material,
            Self::Volume(definition) => definition.material,
        }
    }

    pub fn root_lod(&self) -> u8 {
        match self {
            Self::Planet(definition) => definition.root_lod,
            Self::Volume(definition) => definition.root_lod,
        }
    }

    pub fn max_resident_pages(&self) -> usize {
        match self {
            Self::Planet(definition) => definition.max_resident_pages,
            Self::Volume(definition) => definition.max_resident_pages,
        }
    }

    pub fn fits_centered_root(&self) -> bool {
        match self {
            Self::Planet(definition) => definition.fits_centered_root(),
            Self::Volume(definition) => definition.fits_centered_root(),
        }
    }

    /// The deterministic source this body generates from.
    pub fn generator(&self) -> TerrainGenerator {
        match self {
            Self::Planet(definition) => TerrainGenerator::Sphere(FixedSphereGenerator {
                center_cell: definition.center_cell,
                radius_cells: definition.radius_cells,
                material: definition.material,
            }),
            Self::Volume(definition) => TerrainGenerator::Volume(FixedVolumeGenerator {
                origin_cell: definition.flat.origin,
                half_extent_cells: definition.flat.half_extent_cells(),
                material: definition.material,
            }),
        }
    }

    /// The planet definition, when this body is one.
    pub fn as_planet(&self) -> Option<&PlanetDefinition> {
        match self {
            Self::Planet(definition) => Some(definition),
            Self::Volume(_) => None,
        }
    }

    /// The volume definition, when this body is one.
    pub fn as_volume(&self) -> Option<&VolumeDefinition> {
        match self {
            Self::Volume(definition) => Some(definition),
            Self::Planet(_) => None,
        }
    }

    /// Scalar "height" of a world-meter point above the body's datum.
    ///
    /// A planet's datum is its centre, so this is the radius; a volume's datum
    /// is its ground plane, so this is the vertical offset. Sculpt brushes use
    /// it as the single notion of height a Flatten stroke anchors to, which is
    /// why it lives here rather than in an authoring front-end: the front-end
    /// would otherwise have to know which shape it is holding.
    pub fn altitude_m(&self, point_m: [f64; 3]) -> f64 {
        match self {
            Self::Planet(definition) => {
                let center = cell_to_meters(definition.center_cell);
                let delta = [
                    point_m[0] - center[0],
                    point_m[1] - center[1],
                    point_m[2] - center[2],
                ];
                (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt()
            }
            Self::Volume(definition) => {
                point_m[1] - cell_to_meters(definition.flat.origin)[1]
            }
        }
    }

    /// The brush primitive a *levelling* stamp should use on this body.
    ///
    /// Both primitives are valid on both shapes — this picks the one whose
    /// surface is level with respect to the body's datum, which is what a
    /// Flatten stroke means. A sphere's cap is level on a planet (it is a
    /// constant-radius surface); on a flat world only a box's top face is.
    /// `half_size_cells` is the brush radius, so either primitive's top sits
    /// exactly `half_size_cells` above `center_cell`.
    pub fn flatten_shape(&self, center_cell: [i64; 3], half_size_cells: u32) -> EditShape {
        match self {
            Self::Planet(_) => EditShape::Sphere {
                center_cell,
                radius_cells: half_size_cells,
            },
            Self::Volume(_) => EditShape::Box {
                center_cell,
                half_extent_cells: [half_size_cells; 3],
            },
        }
    }

    /// The point at `altitude_m` on the ray that leaves the datum through
    /// `point_m`'s surface direction `normal`.
    ///
    /// The inverse of [`Self::altitude_m`] for a fixed surface direction: a
    /// Flatten stamp places its primitive here so every stamp in a stroke
    /// resolves to the same level.
    pub fn point_at_altitude_m(
        &self,
        point_m: [f64; 3],
        normal: [f64; 3],
        altitude_m: f64,
    ) -> [f64; 3] {
        match self {
            Self::Planet(definition) => {
                let center = cell_to_meters(definition.center_cell);
                [
                    center[0] + normal[0] * altitude_m,
                    center[1] + normal[1] * altitude_m,
                    center[2] + normal[2] * altitude_m,
                ]
            }
            Self::Volume(definition) => [
                point_m[0],
                cell_to_meters(definition.flat.origin)[1] + altitude_m,
                point_m[2],
            ],
        }
    }
}

impl From<PlanetDefinition> for TerrainBodyDefinition {
    fn from(definition: PlanetDefinition) -> Self {
        Self::Planet(definition)
    }
}

impl From<VolumeDefinition> for TerrainBodyDefinition {
    fn from(definition: VolumeDefinition) -> Self {
        Self::Volume(definition)
    }
}

fn cell_to_meters(cell: [i64; 3]) -> [f64; 3] {
    cell.map(|axis| axis as f64 * LOD0_CELL_SIZE_METERS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CELL_COUNT, PAGE_EDGE_CELLS};

    fn volume(half_extent: i16) -> VolumeDefinition {
        VolumeDefinition {
            volume_id: VolumeId::from_stable_name("flat"),
            flat: FlatTerrain {
                cell_size_m: LOD0_CELL_SIZE_METERS,
                extent: (half_extent, half_extent, half_extent),
                origin: [0; 3],
            },
            material: 1,
            root_lod: 12,
            max_resident_pages: 4_096,
        }
    }

    #[test]
    fn a_volume_and_a_planet_answer_the_same_body_questions() {
        let planet = TerrainBodyDefinition::Planet(PlanetDefinition {
            planet_id: PlanetId::from_stable_name("earth"),
            center_cell: [0; 3],
            radius_cells: 1_000,
            material: 3,
            root_lod: 12,
            max_resident_pages: 64,
        });
        let flat = TerrainBodyDefinition::Volume(volume(1_024));
        assert_eq!(planet.shape(), TerrainShape::Planet);
        assert_eq!(flat.shape(), TerrainShape::Volume);
        assert_eq!(flat.body_id(), VolumeId::from_stable_name("flat").body_id());
        assert!(planet.fits_centered_root());
        assert!(flat.fits_centered_root());
        assert_ne!(planet.generator(), flat.generator());
    }

    #[test]
    fn a_volumes_altitude_is_measured_from_its_ground_plane() {
        let flat = TerrainBodyDefinition::Volume(VolumeDefinition {
            flat: FlatTerrain {
                origin: [0, 100, 0],
                ..volume(1_024).flat
            },
            ..volume(1_024)
        });
        // Origin cell y=100 is 10 m up at the canonical 10 cm cell size.
        assert!((flat.altitude_m([5.0, 12.0, -3.0]) - 2.0).abs() < 1e-9);
        let level = flat.point_at_altitude_m([5.0, 12.0, -3.0], [0.0, 1.0, 0.0], 2.0);
        assert!((level[1] - 12.0).abs() < 1e-9);
        assert_eq!([level[0], level[2]], [5.0, -3.0]);
    }

    #[test]
    fn a_planets_altitude_round_trips_through_its_centre() {
        let planet = TerrainBodyDefinition::Planet(PlanetDefinition {
            planet_id: PlanetId::from_stable_name("earth"),
            center_cell: [0; 3],
            radius_cells: 1_000,
            material: 1,
            root_lod: 12,
            max_resident_pages: 64,
        });
        assert!((planet.altitude_m([0.0, 100.0, 0.0]) - 100.0).abs() < 1e-9);
        assert!((planet.altitude_m([30.0, 40.0, 0.0]) - 50.0).abs() < 1e-9);
        let level = planet.point_at_altitude_m([0.0, 140.0, 0.0], [0.0, 1.0, 0.0], 100.0);
        assert!((level[1] - 100.0).abs() < 1e-9);
    }

    #[test]
    fn a_volume_rejects_a_non_canonical_cell_size_instead_of_ignoring_it() {
        let mut definition = volume(1_024);
        definition.flat.cell_size_m = 1.0;
        assert_eq!(
            definition.validate(),
            Err(VolumeDefinitionError::CellSize(1.0))
        );
    }

    #[test]
    fn a_volume_rejects_a_degenerate_extent() {
        let mut definition = volume(1_024);
        definition.flat.extent = (0, 16, 16);
        assert!(matches!(
            definition.validate(),
            Err(VolumeDefinitionError::Extent(_))
        ));
    }

    #[test]
    fn the_default_extent_spans_two_hundred_metres() {
        let flat = FlatTerrain::centered_on([0; 3]);
        assert_eq!(flat.cell_size_m, LOD0_CELL_SIZE_METERS);
        for span in flat.span_m() {
            assert!((span - 204.9).abs() < 1e-6, "got {span}");
        }
    }

    /// The design doc's §9 open question, answered as an assertion so it stays
    /// answered: how much LOD0 surface does a volume of a given extent carry,
    /// and where does that cross the live runtime's residency cap?
    ///
    /// Live config (`helio-component`'s `live_runtime_config`) allows 8192
    /// resident pages / 1 GiB of dense cells, and the streaming controller
    /// keeps only 96 pages per body actively refined.
    #[test]
    fn the_extent_budget_before_lod_streaming_is_documented_by_measurement() {
        const RESIDENT_PAGE_CAP: u128 = 8_192;
        let page_bytes = (CELL_COUNT * std::mem::size_of::<u32>()) as u128;
        assert_eq!(page_bytes, 131_072, "one dense LOD0 page is 128 KiB");
        assert_eq!(PAGE_EDGE_CELLS, 32, "one LOD0 page is a 3.2 m cube");

        // The default world: 65x65 LOD0 surface pages = 528 MiB, inside the cap.
        let default_pages = FlatTerrain::centered_on([0; 3]).lod0_surface_page_count();
        assert_eq!(default_pages, 65 * 65);
        assert!(default_pages <= RESIDENT_PAGE_CAP);

        // Break-even: a +/-144.8 m world is the largest whose whole LOD0
        // surface could be resident at once (90x90 = 8100 pages).
        let break_even = FlatTerrain {
            cell_size_m: LOD0_CELL_SIZE_METERS,
            extent: (1_448, 1_448, 1_448),
            origin: [0; 3],
        };
        assert_eq!(break_even.lod0_surface_page_count(), 91 * 91);
        assert!(break_even.lod0_surface_page_count() > RESIDENT_PAGE_CAP);
        let inside = FlatTerrain {
            extent: (1_424, 1_424, 1_424),
            ..break_even
        };
        assert_eq!(inside.lod0_surface_page_count(), 90 * 90);
        assert!(inside.lod0_surface_page_count() <= RESIDENT_PAGE_CAP);

        // The i16 ceiling is ~500x past that, so the extent type is not the
        // binding limit -- LOD streaming is, and it is already in the path.
        let widest = FlatTerrain {
            extent: (i16::MAX, i16::MAX, i16::MAX),
            ..break_even
        };
        assert_eq!(widest.lod0_surface_page_count(), 2_048 * 2_048);
        assert!(widest.lod0_surface_page_count() > RESIDENT_PAGE_CAP * 500);
    }
}
