//! Projection bridge from resolved authored portal topology to GPU rows.
//!
//! The resolver owns authored/runtime topology. This module owns the derived
//! frame representation consumed by the existing SceneDB portal buffers. A
//! view row represents one resolved source/target projection, rather than
//! mutating the authored `PortalRow` row. That distinction matters when
//! one indexed sublevel is instanced more than once: the same authored portal
//! can then legitimately produce several projection rows.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use glam::Mat4;
use pulsar_scenedb::Entity;

use crate::components::{PortalChain, PortalProjectionCountsRow, PortalViewRow};
use crate::resolver::{
    PortalOccurrence, PortalProjection, ResolutionError, ResolvedPortalChain, SubLevelResolver,
};
use helio_core::CoordinateSpaceAllocator;
use helio_mesh_api::SubLevelIndex;

/// The coordinate-space slot reserved for root/world coordinates.
pub const IDENTITY_COORDINATE_SPACE_SLOT: u32 = 0;

/// Runtime limits for one portal projection build.
///
/// `max_chain_depth` is the recursion-depth control. It is ordinary runtime
/// data and is intentionally not represented by a const-generic or fixed
/// array type. `max_chains` is an optional runtime work/allocation budget
/// for highly connected portal graphs; it does not reduce chain depth.
/// `None` means grow with the scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortalProjectionConfig {
    pub max_chain_depth: usize,
    pub max_chains: Option<usize>,
    pub max_coordinate_spaces: Option<usize>,
}

impl Default for PortalProjectionConfig {
    fn default() -> Self {
        Self {
            max_chain_depth: 3,
            max_chains: None,
            max_coordinate_spaces: None,
        }
    }
}

/// A stable identity for one runtime portal occurrence.
///
/// The authored entity alone is not sufficient because an indexed sublevel
/// can be placed repeatedly, and each actor path creates a distinct runtime
/// occurrence.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RuntimePortalKey {
    pub entity: Entity,
    pub sublevel_index: SubLevelIndex,
    pub actor_path: Vec<Entity>,
}

impl RuntimePortalKey {
    pub fn from_occurrence(occurrence: &PortalOccurrence) -> Self {
        Self {
            entity: occurrence.entity,
            sublevel_index: occurrence.context.sublevel_index,
            actor_path: occurrence.context.actor_path.clone(),
        }
    }

    fn cmp_path(&self, other: &Self) -> Ordering {
        self.actor_path
            .iter()
            .map(|entity| entity.bits())
            .cmp(other.actor_path.iter().map(|entity| entity.bits()))
    }
}

impl Ord for RuntimePortalKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sublevel_index
            .cmp(&other.sublevel_index)
            .then_with(|| self.cmp_path(other))
            .then_with(|| self.entity.bits().cmp(&other.entity.bits()))
    }
}

impl PartialOrd for RuntimePortalKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Stable identity for a derived view row: a source occurrence displaying a
/// particular target occurrence.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PortalProjectionKey {
    pub source: RuntimePortalKey,
    pub target: RuntimePortalKey,
}

impl PortalProjectionKey {
    pub fn from_projection(projection: &PortalProjection) -> Self {
        Self {
            source: RuntimePortalKey::from_occurrence(&projection.source),
            target: RuntimePortalKey::from_occurrence(&projection.target),
        }
    }
}

impl Ord for PortalProjectionKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.source
            .cmp(&other.source)
            .then_with(|| self.target.cmp(&other.target))
    }
}

impl PartialOrd for PortalProjectionKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Errors raised while converting a valid resolver result into the fixed GPU
/// projection ABI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    Resolution(ResolutionError),
    CoordinateSpaceCapacity {
        requested: usize,
        maximum: usize,
    },
    /// The host's coordinate-space table has no free slot.
    CoordinateSpacesFull,
    ChainCapacity {
        requested: usize,
        maximum: usize,
    },
    SceneRowCount {
        kind: &'static str,
        expected: usize,
        actual: usize,
    },
    NonDenseSceneRow {
        kind: &'static str,
        dense_index: usize,
        entity_index: u32,
    },
}

impl fmt::Display for ProjectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resolution(error) => error.fmt(f),
            Self::CoordinateSpaceCapacity { requested, maximum } => write!(
                f,
                "portal projection needs {requested} coordinate-space slots; maximum is {maximum}"
            ),
            Self::CoordinateSpacesFull => write!(f, "the coordinate-space table is full"),
            Self::ChainCapacity { requested, maximum } => write!(
                f,
                "portal projection produced {requested} chains; maximum is {maximum}"
            ),
            Self::SceneRowCount {
                kind,
                expected,
                actual,
            } => write!(
                f,
                "{kind} projection row list has {actual} entries; expected {expected}"
            ),
            Self::NonDenseSceneRow {
                kind,
                dense_index,
                entity_index,
            } => write!(
                f,
                "{kind} projection row {dense_index} is backed by SceneDB entity index {entity_index}"
            ),
        }
    }
}

impl std::error::Error for ProjectionError {}

impl From<ResolutionError> for ProjectionError {
    fn from(error: ResolutionError) -> Self {
        Self::Resolution(error)
    }
}

/// Dense, deterministic derived rows for one resolved portal frame.
#[derive(Clone, Debug, PartialEq)]
pub struct PortalProjectionFrame {
    /// Full coordinate-space table. Slot zero is always identity.
    pub coordinate_spaces: Vec<Mat4>,
    /// Dense view rows. Their indices are the IDs stored in chain rows.
    pub portal_views: Vec<PortalViewRow>,
    /// Dense chain rows. Each `portals[]` entry indexes `portal_views`.
    pub portal_chains: Vec<PortalChain>,
    /// Stable dense IDs for all resolved runtime occurrences.
    pub occurrence_indices: BTreeMap<RuntimePortalKey, u32>,
    /// Stable dense IDs for source/target projection rows.
    pub projection_indices: BTreeMap<PortalProjectionKey, u32>,
    /// All view rows generated by a runtime occurrence, useful when one
    /// source has multiple cross-sublevel target placements.
    pub view_indices_by_occurrence: BTreeMap<RuntimePortalKey, Vec<u32>>,
    /// Explicit active counts for the SceneDB-facing derived count row.
    pub counts: PortalProjectionCountsRow,
}

impl PortalProjectionFrame {
    /// Coordinate spaces excluding slot zero, suitable for the existing
    /// `Renderer::set_coordinate_spaces` API, which reserves slot zero itself.
    pub fn renderer_coordinate_spaces(&self) -> &[Mat4] {
        &self.coordinate_spaces[1..]
    }

    pub fn portal_view_index(&self, key: &PortalProjectionKey) -> Option<u32> {
        self.projection_indices.get(key).copied()
    }
}

/// Converts resolver output into the existing SceneDB/GPU projection seam.
#[derive(Clone, Copy, Debug)]
pub struct PortalProjectionBridge {
    config: PortalProjectionConfig,
}

impl Default for PortalProjectionBridge {
    fn default() -> Self {
        Self {
            config: PortalProjectionConfig::default(),
        }
    }
}

impl PortalProjectionBridge {
    pub fn new(max_chain_depth: usize) -> Result<Self, ResolutionError> {
        if max_chain_depth == 0 {
            return Err(ResolutionError::InvalidRecursionDepth {
                requested: max_chain_depth,
                maximum: usize::MAX,
            });
        }
        Ok(Self {
            config: PortalProjectionConfig {
                max_chain_depth,
                ..Self::default().config
            },
        })
    }

    pub fn with_config(config: PortalProjectionConfig) -> Result<Self, ResolutionError> {
        if config.max_chain_depth == 0 {
            return Err(ResolutionError::InvalidRecursionDepth {
                requested: config.max_chain_depth,
                maximum: usize::MAX,
            });
        }
        Ok(Self { config })
    }

    pub fn max_chain_depth(&self) -> usize {
        self.config.max_chain_depth
    }

    pub fn config(&self) -> PortalProjectionConfig {
        self.config
    }

    /// Build a projection frame with its own coordinate-space table
    /// (`PortalProjectionFrame::coordinate_spaces`), numbered from slot 1.
    /// Use [`Self::build_into`] when the scene's other spaces (rigid frames)
    /// share the table.
    pub fn build(
        &self,
        resolver: &SubLevelResolver,
    ) -> Result<PortalProjectionFrame, ProjectionError> {
        self.build_local(resolver, None, None)
    }

    /// Build a projection frame after pruning recursive branches against the
    /// current camera view. The visibility test happens before dense chain
    /// rows are allocated, so an uncapped depth request does not first
    /// serialize invisible paths into CPU memory.
    pub fn build_visible(
        &self,
        resolver: &SubLevelResolver,
        view_projection: Mat4,
    ) -> Result<PortalProjectionFrame, ProjectionError> {
        self.build_local(resolver, Some(view_projection), None)
    }

    /// Build a visible projection frame with the camera position available
    /// for conservative portal back-face rejection during recursion.
    pub fn build_visible_from_camera(
        &self,
        resolver: &SubLevelResolver,
        view_projection: Mat4,
        camera_position: glam::Vec3,
    ) -> Result<PortalProjectionFrame, ProjectionError> {
        self.build_local(resolver, Some(view_projection), Some(camera_position))
    }

    /// [`Self::build`], taking its coordinate-space slots from `spaces` (the
    /// host's one table, shared with the scene's rigid frames) instead of
    /// numbering its own. The returned frame's `coordinate_spaces` holds only
    /// the world; the matrices live in the host's table.
    pub fn build_into(
        &self,
        resolver: &SubLevelResolver,
        spaces: &mut dyn CoordinateSpaceAllocator,
    ) -> Result<PortalProjectionFrame, ProjectionError> {
        self.build_with_visibility(resolver, None, None, spaces)
    }

    /// [`Self::build_visible_from_camera`] with the host's table.
    pub fn build_visible_from_camera_into(
        &self,
        resolver: &SubLevelResolver,
        view_projection: Mat4,
        camera_position: glam::Vec3,
        spaces: &mut dyn CoordinateSpaceAllocator,
    ) -> Result<PortalProjectionFrame, ProjectionError> {
        self.build_with_visibility(
            resolver,
            Some(view_projection),
            Some(camera_position),
            spaces,
        )
    }

    /// Build with a table of this frame's own (the `build*` methods).
    fn build_local(
        &self,
        resolver: &SubLevelResolver,
        view_projection: Option<Mat4>,
        camera_position: Option<glam::Vec3>,
    ) -> Result<PortalProjectionFrame, ProjectionError> {
        let mut local = LocalSpaces {
            matrices: vec![Mat4::IDENTITY],
            slots: HashMap::from([(matrix_key(Mat4::IDENTITY), IDENTITY_COORDINATE_SPACE_SLOT)]),
            maximum: self.config.max_coordinate_spaces,
        };
        let mut frame = self
            .build_with_visibility(resolver, view_projection, camera_position, &mut local)
            .map_err(|error| match (error, self.config.max_coordinate_spaces) {
                (ProjectionError::CoordinateSpacesFull, Some(maximum)) => {
                    ProjectionError::CoordinateSpaceCapacity {
                        requested: local.matrices.len() + 1,
                        maximum,
                    }
                }
                (error, _) => error,
            })?;
        frame.coordinate_spaces = local.matrices;
        Ok(frame)
    }

    fn build_with_visibility(
        &self,
        resolver: &SubLevelResolver,
        view_projection: Option<Mat4>,
        camera_position: Option<glam::Vec3>,
        spaces: &mut dyn CoordinateSpaceAllocator,
    ) -> Result<PortalProjectionFrame, ProjectionError> {
        let occurrences = resolver.resolve_portals()?;
        let mut all_occurrences = BTreeMap::<RuntimePortalKey, PortalOccurrence>::new();
        for occurrence in occurrences {
            all_occurrences.insert(RuntimePortalKey::from_occurrence(&occurrence), occurrence);
        }

        let mut projections = resolver.resolve_projections()?;
        let direct_projection_keys: BTreeSet<_> = projections
            .iter()
            .map(PortalProjectionKey::from_projection)
            .collect();
        let mut chains = match (view_projection, camera_position) {
            (Some(view_projection), Some(camera_position)) => resolver
                .resolve_chains_visible_from_camera(
                    self.max_chain_depth(),
                    self.config.max_chains,
                    view_projection,
                    camera_position,
                )?,
            (Some(view_projection), None) => resolver.resolve_chains_visible(
                self.max_chain_depth(),
                self.config.max_chains,
                view_projection,
            )?,
            (None, None) => {
                resolver.resolve_chains_bounded(self.max_chain_depth(), self.config.max_chains)?
            }
            (None, Some(_)) => unreachable!("camera position requires a view projection"),
        };
        for projection in &projections {
            all_occurrences
                .entry(RuntimePortalKey::from_occurrence(&projection.source))
                .or_insert_with(|| projection.source.clone());
            all_occurrences
                .entry(RuntimePortalKey::from_occurrence(&projection.target))
                .or_insert_with(|| projection.target.clone());
        }
        for chain in &chains {
            for projection in &chain.projections {
                all_occurrences
                    .entry(RuntimePortalKey::from_occurrence(&projection.source))
                    .or_insert_with(|| projection.source.clone());
                all_occurrences
                    .entry(RuntimePortalKey::from_occurrence(&projection.target))
                    .or_insert_with(|| projection.target.clone());
            }
        }

        // Chains contain the same first-hop projection data as the direct
        // projection list, so make the row set the union of both sources and
        // sort it by stable runtime keys before assigning any GPU index.
        for chain in &chains {
            projections.extend(chain.projections.iter().cloned());
        }
        projections.sort_by_key(PortalProjectionKey::from_projection);
        projections.dedup_by(|left, right| {
            PortalProjectionKey::from_projection(left)
                == PortalProjectionKey::from_projection(right)
        });

        let mut projection_indices = BTreeMap::new();
        for (index, projection) in projections.iter().enumerate() {
            projection_indices.insert(
                PortalProjectionKey::from_projection(projection),
                index as u32,
            );
        }

        let mut highest_slot = IDENTITY_COORDINATE_SPACE_SLOT;
        let mut portal_views = Vec::with_capacity(projections.len());
        let mut view_indices_by_occurrence = BTreeMap::<RuntimePortalKey, Vec<u32>>::new();
        for (index, projection) in projections.iter().enumerate() {
            let coordinate_space = spaces
                .space_for(projection.target_to_source)
                .ok_or(ProjectionError::CoordinateSpacesFull)?;
            highest_slot = highest_slot.max(coordinate_space);
            let mut view = projection.to_portal_view(coordinate_space);
            if !direct_projection_keys.contains(&PortalProjectionKey::from_projection(projection)) {
                view._pad = PortalViewRow::FLAG_MASK_HIDDEN;
            }
            portal_views.push(view);
            view_indices_by_occurrence
                .entry(RuntimePortalKey::from_occurrence(&projection.source))
                .or_default()
                .push(index as u32);
        }

        chains.sort_by_key(chain_sort_key);
        chains.dedup_by(|left, right| chain_sort_key(left) == chain_sort_key(right));
        if let Some(maximum) = self.config.max_chains {
            if chains.len() > maximum {
                return Err(ProjectionError::ChainCapacity {
                    requested: chains.len(),
                    maximum,
                });
            }
        }
        let mut portal_chains = Vec::with_capacity(chains.len());
        for chain in chains {
            let portals = chain
                .projections
                .iter()
                .map(|projection| {
                    *projection_indices
                        .get(&PortalProjectionKey::from_projection(projection))
                        .expect("chain projection was included in projection rows")
                })
                .collect();
            portal_chains.push(PortalChain { portals });
        }

        let occurrence_indices = all_occurrences
            .keys()
            .enumerate()
            .map(|(index, key)| (key.clone(), index as u32))
            .collect();
        let counts = PortalProjectionCountsRow {
            portal_view_count: portal_views.len() as u32,
            portal_chain_count: portal_chains.len() as u32,
            coordinate_space_count: highest_slot + 1,
            _pad: 0,
        };

        Ok(PortalProjectionFrame {
            coordinate_spaces: vec![Mat4::IDENTITY],
            portal_views,
            portal_chains,
            occurrence_indices,
            projection_indices,
            view_indices_by_occurrence,
            counts,
        })
    }
}

fn matrix_key(matrix: Mat4) -> [u32; 16] {
    matrix.to_cols_array().map(f32::to_bits)
}

/// The table a `build*` call numbers for itself: slot 0 is the world, then
/// each distinct matrix in order of first use.
struct LocalSpaces {
    matrices: Vec<Mat4>,
    slots: HashMap<[u32; 16], u32>,
    maximum: Option<usize>,
}

impl CoordinateSpaceAllocator for LocalSpaces {
    fn space_for(&mut self, matrix: Mat4) -> Option<u32> {
        let key = matrix_key(matrix);
        if let Some(&slot) = self.slots.get(&key) {
            return Some(slot);
        }
        if self
            .maximum
            .is_some_and(|maximum| self.matrices.len() >= maximum)
        {
            return None;
        }
        let slot = self.matrices.len() as u32;
        self.matrices.push(matrix);
        self.slots.insert(key, slot);
        Some(slot)
    }
}

fn chain_sort_key(chain: &ResolvedPortalChain) -> Vec<PortalProjectionKey> {
    chain
        .projections
        .iter()
        .map(PortalProjectionKey::from_projection)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::PortalRow;
    use crate::resolver::SubLevelContents;
    use helio_mesh_api::SubLevelActorRow;

    #[test]
    fn dense_indexing_and_identity_slot_are_deterministic() {
        let mut world = pulsar_scenedb::World::new();
        let a_entity = world.spawn();
        let b_entity = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (
                        a_entity,
                        PortalRow::new(Some(b_entity), Mat4::IDENTITY, [1.0, 1.0]),
                    ),
                    (
                        b_entity,
                        PortalRow::new(Some(a_entity), Mat4::IDENTITY, [1.0, 1.0]),
                    ),
                ],
            ),
        );
        let frame = PortalProjectionBridge::new(1)
            .unwrap()
            .build(&resolver)
            .unwrap();

        assert_eq!(frame.coordinate_spaces[0], Mat4::IDENTITY);
        // Even identity-authored portal poses get a distinct render-space
        // turn: the view map includes the standard 180-degree portal-plane
        // flip, so target contents land beyond the source opening.
        assert_eq!(frame.coordinate_spaces.len(), 2);
        assert_eq!(frame.portal_views.len(), 2);
        assert_eq!(frame.portal_chains.len(), 2);
        assert_eq!(frame.counts.portal_view_count, 2);
        assert_eq!(frame.counts.portal_chain_count, 2);
        assert!(frame.projection_indices.values().copied().eq(0..2));
    }

    #[test]
    fn same_sublevel_projection_uses_one_dense_pair() {
        let mut world = pulsar_scenedb::World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (
                        a,
                        PortalRow::new(
                            Some(b),
                            Mat4::from_translation(glam::vec3(2.0, 0.0, 0.0)),
                            [1.0, 1.0],
                        ),
                    ),
                    (
                        b,
                        PortalRow::new(
                            Some(a),
                            Mat4::from_translation(glam::vec3(-2.0, 0.0, 0.0)),
                            [1.0, 1.0],
                        ),
                    ),
                ],
            ),
        );
        let frame = PortalProjectionBridge::new(1)
            .unwrap()
            .build(&resolver)
            .unwrap();
        assert_eq!(frame.portal_views.len(), 2);
        assert_eq!(frame.coordinate_spaces.len(), 3);
    }

    #[test]
    fn a_host_allocator_shares_its_table_with_the_projection() {
        // The host owns one table. Three rigid frames already hold slots 1..=3;
        // the projection asks it for its spaces instead of numbering its own.
        struct Host {
            matrices: Vec<Mat4>,
        }
        impl CoordinateSpaceAllocator for Host {
            fn space_for(&mut self, matrix: Mat4) -> Option<u32> {
                if let Some(slot) = self.matrices.iter().position(|m| *m == matrix) {
                    return Some(slot as u32);
                }
                self.matrices.push(matrix);
                Some(self.matrices.len() as u32 - 1)
            }
        }
        let frame_matrix = |x: f32| Mat4::from_translation(glam::vec3(x, 100.0, 0.0));
        let mut host = Host {
            matrices: vec![
                Mat4::IDENTITY,
                frame_matrix(1.0),
                frame_matrix(2.0),
                frame_matrix(3.0),
            ],
        };

        let mut world = pulsar_scenedb::World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (
                        a,
                        PortalRow::new(
                            Some(b),
                            Mat4::from_translation(glam::vec3(2.0, 0.0, 0.0)),
                            [1.0, 1.0],
                        ),
                    ),
                    (
                        b,
                        PortalRow::new(
                            Some(a),
                            Mat4::from_translation(glam::vec3(-2.0, 0.0, 0.0)),
                            [1.0, 1.0],
                        ),
                    ),
                ],
            ),
        );
        let frame = PortalProjectionBridge::new(1)
            .unwrap()
            .build_into(&resolver, &mut host)
            .unwrap();

        assert_eq!(
            host.matrices.len(),
            6,
            "world, 3 frames, 2 projection spaces"
        );
        assert_eq!(
            host.matrices[1],
            frame_matrix(1.0),
            "the frames are untouched"
        );
        assert_eq!(
            frame.coordinate_spaces,
            vec![Mat4::IDENTITY],
            "the host owns the matrices"
        );
        assert!(
            frame
                .portal_views
                .iter()
                .all(|view| view.coordinate_space >= 4),
            "no view uses a slot a frame holds"
        );
        assert_eq!(frame.counts.coordinate_space_count, 6);
    }

    #[test]
    fn disabled_entry_portal_does_not_create_a_recursive_opening() {
        let mut world = pulsar_scenedb::World::new();
        let source = world.spawn();
        let target = world.spawn();
        let mut target_component = PortalRow::new(
            Some(source),
            Mat4::from_translation(glam::vec3(-2.0, 0.0, 0.0)),
            [1.0, 1.0],
        );
        target_component.flags &= !PortalRow::FLAG_ENABLED;

        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (
                        source,
                        PortalRow::new(
                            Some(target),
                            Mat4::from_translation(glam::vec3(2.0, 0.0, 0.0)),
                            [1.0, 1.0],
                        ),
                    ),
                    (target, target_component),
                ],
            ),
        );

        let frame = PortalProjectionBridge::new(3)
            .unwrap()
            .build(&resolver)
            .unwrap();
        assert!(frame
            .portal_views
            .iter()
            .all(|view| view._pad != PortalViewRow::FLAG_MASK_HIDDEN));
        assert!(frame.portal_views.iter().any(|view| view._pad == 0));
    }

    #[test]
    fn cross_sublevel_projection_has_a_canonical_target_occurrence() {
        let mut world = pulsar_scenedb::World::new();
        let source = world.spawn();
        let target = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [(
                    source,
                    PortalRow::new(Some(target), Mat4::IDENTITY, [1.0, 1.0]),
                )],
            ),
        );
        resolver.insert_sublevel(
            1,
            SubLevelContents::new(
                [],
                [(
                    target,
                    PortalRow::new(
                        Some(source),
                        Mat4::from_translation(glam::vec3(3.0, 0.0, 0.0)),
                        [1.0, 1.0],
                    ),
                )],
            ),
        );
        let frame = PortalProjectionBridge::new(1)
            .unwrap()
            .build(&resolver)
            .unwrap();
        assert_eq!(frame.occurrence_indices.len(), 2);
        assert_eq!(frame.portal_views.len(), 1);
        assert_eq!(frame.counts.coordinate_space_count, 2);
    }

    #[test]
    fn nested_actor_projection_is_keyed_by_actor_path() {
        let mut world = pulsar_scenedb::World::new();
        let actor_entity = world.spawn();
        let a = world.spawn();
        let b = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [(
                    actor_entity,
                    SubLevelActorRow::new(1, Mat4::from_translation(glam::vec3(10.0, 0.0, 0.0))),
                )],
                [],
            ),
        );
        resolver.insert_sublevel(
            1,
            SubLevelContents::new(
                [],
                [
                    (a, PortalRow::new(Some(b), Mat4::IDENTITY, [1.0, 1.0])),
                    (
                        b,
                        PortalRow::new(
                            Some(a),
                            Mat4::from_translation(glam::vec3(0.0, 0.0, 2.0)),
                            [1.0, 1.0],
                        ),
                    ),
                ],
            ),
        );
        let frame = PortalProjectionBridge::new(1)
            .unwrap()
            .build(&resolver)
            .unwrap();
        assert!(frame
            .occurrence_indices
            .keys()
            .any(|key| key.actor_path == vec![actor_entity]));
        assert_eq!(frame.portal_views.len(), 2);
    }

    #[test]
    fn chain_rows_reference_dense_view_rows() {
        let mut world = pulsar_scenedb::World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (a, PortalRow::new(Some(b), Mat4::IDENTITY, [1.0, 1.0])),
                    (b, PortalRow::new(Some(a), Mat4::IDENTITY, [1.0, 1.0])),
                ],
            ),
        );
        let frame = PortalProjectionBridge::new(2)
            .unwrap()
            .build(&resolver)
            .unwrap();
        assert_eq!(frame.portal_chains.len(), 4);
        assert!(frame.portal_chains.iter().all(|chain| {
            (chain.portals.len() == 1 || chain.portals.len() == 2)
                && chain
                    .portals
                    .iter()
                    .all(|id| (*id as usize) < frame.portal_views.len())
        }));
    }

    #[test]
    fn chain_rows_accept_runtime_depth_above_legacy_default() {
        let mut world = pulsar_scenedb::World::new();
        let a = world.spawn();
        let b = world.spawn();
        let mut resolver = SubLevelResolver::new();
        resolver.insert_sublevel(
            0,
            SubLevelContents::new(
                [],
                [
                    (a, PortalRow::new(Some(b), Mat4::IDENTITY, [1.0, 1.0])),
                    (b, PortalRow::new(Some(a), Mat4::IDENTITY, [1.0, 1.0])),
                ],
            ),
        );

        let frame = PortalProjectionBridge::new(4)
            .unwrap()
            .build(&resolver)
            .unwrap();
        assert!(frame
            .portal_chains
            .iter()
            .any(|chain| chain.portals.len() == 4));
    }
}
