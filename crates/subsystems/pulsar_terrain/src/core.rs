use crate::edit::EditIndex;
use crate::{
    CompactedPageRecord, DeterministicGenerator, EditError, EditLog, EditOp, HierarchyError,
    NodeState, PageCodecError, PageKey, PlanetId, SparseBrickTree, TerrainSnapshot, VoxelPage,
};
use std::collections::BTreeMap;
use thiserror::Error;

/// Owning authoritative state for one planet's sparse terrain.
///
/// GPU pages, extracted meshes, and collision data are deliberately absent;
/// those are generation-tagged consumers of this state in later milestones.
pub struct TerrainCore<G> {
    planet_id: PlanetId,
    generator: G,
    hierarchy: SparseBrickTree,
    edits: EditLog,
    edit_index: EditIndex,
    pages: BTreeMap<PageKey, VoxelPage>,
    compacted: BTreeMap<PageKey, CompactedPageRecord>,
    work: TerrainWorkCounters,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainWorkCounters {
    pub edits_appended: u64,
    pub hierarchy_overrides: u64,
    pub pages_compacted: u64,
    pub cells_generated: u64,
    pub cells_replayed: u64,
    pub edit_candidates_replayed: u64,
    pub pages_rehydrated: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainMemoryCounters {
    pub hierarchy_nodes: usize,
    pub hierarchy_encoded_bytes: usize,
    pub edit_operations: usize,
    pub edit_attachment_regions: usize,
    pub edit_attachment_references: usize,
    pub resident_pages: usize,
    pub resident_dense_bytes: usize,
    pub compacted_page_records: usize,
}

#[derive(Clone, Debug)]
pub enum PageBuildPreparation<G> {
    Current(CompactedPageRecord),
    Build(PageBuildRequest<G>),
}

/// Immutable input for one off-thread page build. Preparing this value never
/// mutates canonical terrain state; publishing requires a later generation
/// check through [`TerrainCore::commit_page_build`].
#[derive(Clone, Debug)]
pub struct PageBuildRequest<G> {
    key: PageKey,
    generator: G,
    base_page: Option<VoxelPage>,
    base_page_id: Option<crate::PageId>,
    previous_sequence: u64,
    target_sequence: u64,
    operations: Vec<EditOp>,
}

#[derive(Clone, Debug)]
pub struct PageBuildResult {
    key: PageKey,
    page: VoxelPage,
    base_page_id: Option<crate::PageId>,
    previous_sequence: u64,
    target_sequence: u64,
    replayed_operations: usize,
    reused_resident_page: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageBuildCommitOutcome {
    Committed(CompactedPageRecord),
    Duplicate(CompactedPageRecord),
    Stale { newest_sequence: u64 },
}

impl<G: DeterministicGenerator> PageBuildRequest<G> {
    pub fn key(&self) -> PageKey {
        self.key
    }

    pub fn target_sequence(&self) -> u64 {
        self.target_sequence
    }

    pub fn execute(self) -> Result<PageBuildResult, TerrainCoreError> {
        let reused_resident_page = self.base_page.is_some();
        let page = if let Some(base_page) = self.base_page {
            base_page.apply_edit_tail(self.key, &self.operations)?
        } else {
            VoxelPage::generate_with_operations(self.key, &self.generator, &self.operations)?
        };
        Ok(PageBuildResult {
            key: self.key,
            page,
            base_page_id: self.base_page_id,
            previous_sequence: self.previous_sequence,
            target_sequence: self.target_sequence,
            replayed_operations: self.operations.len(),
            reused_resident_page,
        })
    }
}

impl PageBuildResult {
    pub fn key(&self) -> PageKey {
        self.key
    }

    pub fn target_sequence(&self) -> u64 {
        self.target_sequence
    }

    pub fn page(&self) -> &VoxelPage {
        &self.page
    }
}

impl<G: DeterministicGenerator> TerrainCore<G> {
    pub fn new(planet_id: PlanetId, root_lod: u8, generator: G) -> Result<Self, TerrainCoreError> {
        let hierarchy =
            SparseBrickTree::centered(root_lod, NodeState::Procedural(generator.hash()))?;
        Ok(Self {
            planet_id,
            generator,
            hierarchy,
            edits: EditLog::default(),
            edit_index: EditIndex::new(root_lod),
            pages: BTreeMap::new(),
            compacted: BTreeMap::new(),
            work: TerrainWorkCounters::default(),
        })
    }

    pub fn hierarchy(&self) -> &SparseBrickTree {
        &self.hierarchy
    }

    pub fn planet_id(&self) -> PlanetId {
        self.planet_id
    }

    pub fn edit_log(&self) -> &EditLog {
        &self.edits
    }

    pub fn append_edit(&mut self, operation: EditOp) -> Result<(), TerrainCoreError> {
        let previous_len = self.edits.operations().len();
        self.edits.push(operation)?;
        if self.edits.operations().len() != previous_len {
            self.edit_index.insert(previous_len, operation);
            self.work.edits_appended = self.work.edits_appended.saturating_add(1);
        }
        Ok(())
    }

    pub fn prepare_page_build(
        &self,
        key: PageKey,
    ) -> Result<PageBuildPreparation<G>, TerrainCoreError>
    where
        G: Clone,
    {
        let previous_sequence = self
            .compacted
            .get(&key)
            .map_or(0, |record| record.compacted_through_sequence);
        let latest_sequence = self.edits.latest_sequence();
        if previous_sequence == latest_sequence {
            if let Some(record) = self.compacted.get(&key) {
                if self.pages.contains_key(&key) {
                    return Ok(PageBuildPreparation::Current(*record));
                }
            }
        }

        let base_page = self.pages.get(&key).cloned();
        // Dense pages are a disposable cache. If one was evicted, rebuild it
        // from the deterministic source and full relevant edit prefix.
        let replay_from_sequence = if base_page.is_some() {
            previous_sequence
        } else {
            0
        };
        let relevant = self
            .edit_index
            .operations_for_page(&self.edits, key, replay_from_sequence);
        Ok(PageBuildPreparation::Build(PageBuildRequest {
            key,
            generator: self.generator.clone(),
            base_page_id: base_page.as_ref().map(VoxelPage::page_id),
            base_page,
            previous_sequence: replay_from_sequence,
            target_sequence: latest_sequence,
            operations: relevant,
        }))
    }

    pub fn commit_page_build(
        &mut self,
        result: PageBuildResult,
    ) -> Result<PageBuildCommitOutcome, TerrainCoreError> {
        let latest_sequence = self.edits.latest_sequence();
        if result.target_sequence != latest_sequence {
            return Ok(PageBuildCommitOutcome::Stale {
                newest_sequence: latest_sequence,
            });
        }
        if let Some(current) = self.compacted.get(&result.key).copied() {
            if !self.pages.contains_key(&result.key) && result.previous_sequence == 0 {
                let rebuilt_page_id = result.page.page_id();
                if current.compacted_through_sequence == result.target_sequence {
                    if current.page_id != rebuilt_page_id {
                        return Err(TerrainCoreError::RehydratedPageMismatch(result.key));
                    }
                    self.pages.insert(result.key, result.page);
                    self.work.pages_rehydrated = self.work.pages_rehydrated.saturating_add(1);
                    self.work.cells_generated = self
                        .work
                        .cells_generated
                        .saturating_add(crate::CELL_COUNT as u64);
                    self.work.edit_candidates_replayed = self
                        .work
                        .edit_candidates_replayed
                        .saturating_add(result.replayed_operations as u64);
                    return Ok(PageBuildCommitOutcome::Duplicate(current));
                }
                // This resident cache was evicted before newer edits arrived.
                // The full replay below safely replaces the older compacted
                // record because target_sequence was checked above.
            } else {
                if current.compacted_through_sequence == result.target_sequence
                    && self.pages.contains_key(&result.key)
                {
                    return Ok(PageBuildCommitOutcome::Duplicate(current));
                }
                if current.compacted_through_sequence != result.previous_sequence
                    || self.pages.get(&result.key).map(VoxelPage::page_id) != result.base_page_id
                {
                    return Ok(PageBuildCommitOutcome::Stale {
                        newest_sequence: current.compacted_through_sequence.max(latest_sequence),
                    });
                }
            }
        } else if result.previous_sequence != 0 || result.base_page_id.is_some() {
            return Ok(PageBuildCommitOutcome::Stale {
                newest_sequence: latest_sequence,
            });
        }

        let page_id = result.page.page_id();
        let record = CompactedPageRecord {
            key: result.key,
            page_id,
            compacted_through_sequence: result.target_sequence,
        };
        let state = result
            .page
            .constant_cell()
            .map_or(NodeState::Page(page_id), |cell| {
                if cell.is_solid() {
                    NodeState::Solid(cell.material())
                } else {
                    NodeState::Air
                }
            });
        self.hierarchy.set(result.key, state)?;
        self.pages.insert(result.key, result.page);
        self.compacted.insert(result.key, record);
        self.work.pages_compacted = self.work.pages_compacted.saturating_add(1);
        self.work.edit_candidates_replayed = self
            .work
            .edit_candidates_replayed
            .saturating_add(result.replayed_operations as u64);
        if result.reused_resident_page {
            self.work.cells_replayed = self
                .work
                .cells_replayed
                .saturating_add(crate::CELL_COUNT as u64);
        } else {
            self.work.cells_generated = self
                .work
                .cells_generated
                .saturating_add(crate::CELL_COUNT as u64);
        }
        Ok(PageBuildCommitOutcome::Committed(record))
    }

    /// Fold the current ordered edit prefix into one content-addressed LOD0
    /// page and publish that page into the canonical hierarchy.
    pub fn compact_page(&mut self, key: PageKey) -> Result<CompactedPageRecord, TerrainCoreError>
    where
        G: Clone,
    {
        match self.prepare_page_build(key)? {
            PageBuildPreparation::Current(record) => Ok(record),
            PageBuildPreparation::Build(request) => {
                match self.commit_page_build(request.execute()?)? {
                    PageBuildCommitOutcome::Committed(record)
                    | PageBuildCommitOutcome::Duplicate(record) => Ok(record),
                    PageBuildCommitOutcome::Stale { .. } => {
                        unreachable!("synchronous page build cannot become stale")
                    }
                }
            }
        }
    }

    pub fn page(&self, key: PageKey) -> Option<&VoxelPage> {
        self.pages.get(&key)
    }

    pub fn resident_page_keys(&self) -> impl ExactSizeIterator<Item = PageKey> + '_ {
        self.pages.keys().copied()
    }

    /// Drop one decompressed page while retaining its authoritative compacted
    /// record and hierarchy entry. A later request rehydrates the exact bytes
    /// from the deterministic generator and ordered edit prefix.
    pub fn evict_resident_page(&mut self, key: PageKey) -> bool {
        self.pages.remove(&key).is_some()
    }

    /// Exact whole-root replacement. Resident pages remain disposable caches;
    /// the root state is immediately authoritative without iterating over them.
    pub fn set_root(&mut self, state: NodeState) -> Result<(), TerrainCoreError> {
        self.hierarchy.set_root(state)?;
        self.work.hierarchy_overrides = self.work.hierarchy_overrides.saturating_add(1);
        Ok(())
    }

    /// Attach an exact uniform or content-addressed override at any hierarchy
    /// level. Descendants are resolved lazily and never expanded here.
    pub fn set_region(&mut self, key: PageKey, state: NodeState) -> Result<(), TerrainCoreError> {
        self.hierarchy.set(key, state)?;
        self.work.hierarchy_overrides = self.work.hierarchy_overrides.saturating_add(1);
        Ok(())
    }

    pub fn snapshot(&self) -> TerrainSnapshot {
        TerrainSnapshot {
            planet_id: self.planet_id,
            generator_hash: self.generator.hash(),
            hierarchy: self.hierarchy.clone(),
            edit_tail: self.edits.clone(),
            compacted_pages: self.compacted.values().copied().collect(),
        }
    }

    pub fn resident_page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn work_counters(&self) -> TerrainWorkCounters {
        self.work
    }

    pub fn memory_counters(&self) -> TerrainMemoryCounters {
        TerrainMemoryCounters {
            hierarchy_nodes: self.hierarchy.node_count(),
            hierarchy_encoded_bytes: self.hierarchy.encode().len(),
            edit_operations: self.edits.operations().len(),
            edit_attachment_regions: self.edit_index.region_count(),
            edit_attachment_references: self.edit_index.reference_count(),
            resident_pages: self.pages.len(),
            resident_dense_bytes: self
                .pages
                .values()
                .map(VoxelPage::dense_allocation_bytes)
                .sum(),
            compacted_page_records: self.compacted.len(),
        }
    }
}

#[derive(Debug, Error)]
pub enum TerrainCoreError {
    #[error(transparent)]
    Hierarchy(#[from] HierarchyError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error(transparent)]
    Page(#[from] PageCodecError),
    #[error("rehydrated page {0:?} does not match its authoritative content hash")]
    RehydratedPageMismatch(PageKey),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditMode, EditShape, FixedSphereGenerator};

    #[test]
    fn compaction_publishes_a_hashed_page_and_snapshot() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([1; 16]), 12, generator).unwrap();
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [8; 16],
            shape: EditShape::Sphere {
                center_cell: [4, 4, 4],
                radius_cells: 3,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();
        let key = PageKey::new(0, [0, 0, 0]);
        let record = core.compact_page(key).unwrap();
        assert_eq!(record.page_id, core.page(key).unwrap().page_id());
        assert_eq!(
            core.hierarchy().resolve(key).unwrap(),
            NodeState::Page(record.page_id)
        );
        assert_eq!(core.snapshot().compacted_pages, vec![record]);

        core.set_root(NodeState::Air).unwrap();
        assert_eq!(core.hierarchy().node_count(), 1);
        assert_eq!(core.resident_page_count(), 1);
        assert_eq!(core.work_counters().edits_appended, 1);
        assert_eq!(core.work_counters().pages_compacted, 1);
        assert_eq!(
            core.work_counters().cells_generated,
            crate::CELL_COUNT as u64
        );
        assert_eq!(core.memory_counters().resident_pages, 1);

        core.append_edit(EditOp {
            sequence: 2,
            stable_id: [9; 16],
            shape: EditShape::Sphere {
                center_cell: [8, 8, 8],
                radius_cells: 2,
            },
            mode: EditMode::Paint,
            material: 11,
        })
        .unwrap();
        let updated = core.compact_page(key).unwrap();
        assert_eq!(updated.compacted_through_sequence, 2);
        assert_eq!(
            core.work_counters().cells_generated,
            crate::CELL_COUNT as u64
        );
        assert_eq!(
            core.work_counters().cells_replayed,
            crate::CELL_COUNT as u64
        );
        assert_eq!(core.work_counters().pages_compacted, 2);
    }

    #[test]
    fn uniform_compaction_and_high_level_override_remain_sparse() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([2; 16]), 24, generator).unwrap();
        let far_page = PageKey::new(0, [1_000_000, 0, 0]);
        core.compact_page(far_page).unwrap();
        assert_eq!(core.hierarchy().resolve(far_page).unwrap(), NodeState::Air);
        assert_eq!(core.page(far_page).unwrap().dense_allocation_bytes(), 0);

        let continent = PageKey::new(16, [-2, 1, 0]);
        core.set_region(continent, NodeState::Air).unwrap();
        assert_eq!(core.hierarchy().resolve(continent).unwrap(), NodeState::Air);
        assert!(core.hierarchy().node_count() <= 1 + 8 * 24 + 8 * 8);
        assert_eq!(core.work_counters().hierarchy_overrides, 1);
    }

    #[test]
    fn page_compaction_replays_only_spatially_attached_edit_candidates() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([3; 16]), 24, generator).unwrap();
        for sequence in 1..=64_u64 {
            let page = 1_000 + sequence as i64;
            core.append_edit(EditOp {
                sequence,
                stable_id: [sequence as u8; 16],
                shape: EditShape::Sphere {
                    center_cell: [page * crate::PAGE_EDGE_CELLS + 8; 3],
                    radius_cells: 1,
                },
                mode: EditMode::Subtract,
                material: 0,
            })
            .unwrap();
        }
        core.append_edit(EditOp {
            sequence: 65,
            stable_id: [65; 16],
            shape: EditShape::Sphere {
                center_cell: [8; 3],
                radius_cells: 2,
            },
            mode: EditMode::Paint,
            material: 9,
        })
        .unwrap();

        core.compact_page(PageKey::new(0, [0; 3])).unwrap();
        assert_eq!(core.work_counters().edit_candidates_replayed, 1);
        assert_eq!(core.memory_counters().edit_attachment_references, 65);
        assert_eq!(core.memory_counters().edit_attachment_regions, 65);
    }

    #[test]
    fn stale_off_thread_page_build_cannot_replace_newer_terrain() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([4; 16]), 12, generator).unwrap();
        let key = PageKey::new(0, [0; 3]);
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [1; 16],
            shape: EditShape::Sphere {
                center_cell: [4; 3],
                radius_cells: 2,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();
        let request = match core.prepare_page_build(key).unwrap() {
            PageBuildPreparation::Build(request) => request,
            PageBuildPreparation::Current(_) => panic!("first request must require a build"),
        };
        let result = request.execute().unwrap();

        core.append_edit(EditOp {
            sequence: 2,
            stable_id: [2; 16],
            shape: EditShape::Sphere {
                center_cell: [8; 3],
                radius_cells: 1,
            },
            mode: EditMode::Paint,
            material: 9,
        })
        .unwrap();

        assert_eq!(
            core.commit_page_build(result).unwrap(),
            PageBuildCommitOutcome::Stale { newest_sequence: 2 }
        );
        assert!(core.page(key).is_none());
        assert_eq!(core.work_counters().pages_compacted, 0);
        assert_eq!(core.memory_counters().compacted_page_records, 0);
    }

    #[test]
    fn duplicate_off_thread_page_build_is_idempotent() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([5; 16]), 12, generator).unwrap();
        let key = PageKey::new(0, [0; 3]);
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [3; 16],
            shape: EditShape::Sphere {
                center_cell: [4; 3],
                radius_cells: 2,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();

        let first = match core.prepare_page_build(key).unwrap() {
            PageBuildPreparation::Build(request) => request.execute().unwrap(),
            PageBuildPreparation::Current(_) => panic!("first request must require a build"),
        };
        let duplicate = match core.prepare_page_build(key).unwrap() {
            PageBuildPreparation::Build(request) => request.execute().unwrap(),
            PageBuildPreparation::Current(_) => panic!("uncommitted request cannot be current"),
        };

        let committed = match core.commit_page_build(first).unwrap() {
            PageBuildCommitOutcome::Committed(record) => record,
            outcome => panic!("unexpected first commit outcome: {outcome:?}"),
        };
        assert_eq!(
            core.commit_page_build(duplicate).unwrap(),
            PageBuildCommitOutcome::Duplicate(committed)
        );
        assert_eq!(core.work_counters().pages_compacted, 1);
        assert_eq!(core.memory_counters().compacted_page_records, 1);
    }

    #[test]
    fn evicted_dense_page_rehydrates_to_the_authoritative_hash() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([6; 16]), 12, generator).unwrap();
        let key = PageKey::new(0, [0; 3]);
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [7; 16],
            shape: EditShape::Sphere {
                center_cell: [8; 3],
                radius_cells: 3,
            },
            mode: EditMode::Paint,
            material: 12,
        })
        .unwrap();
        let original = core.compact_page(key).unwrap();
        let snapshot_before = core.snapshot();

        assert!(core.evict_resident_page(key));
        assert!(!core.evict_resident_page(key));
        assert!(core.page(key).is_none());
        assert_eq!(core.snapshot(), snapshot_before);

        let rehydrated = core.compact_page(key).unwrap();
        assert_eq!(rehydrated, original);
        assert_eq!(core.page(key).unwrap().page_id(), original.page_id);
        assert_eq!(core.work_counters().pages_rehydrated, 1);
        assert_eq!(core.work_counters().pages_compacted, 1);
    }

    #[test]
    fn evicted_page_full_replay_includes_edits_added_while_absent() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([7; 16]), 12, generator).unwrap();
        let key = PageKey::new(0, [0; 3]);
        core.compact_page(key).unwrap();
        assert!(core.evict_resident_page(key));
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [8; 16],
            shape: EditShape::Sphere {
                center_cell: [8; 3],
                radius_cells: 2,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();

        let updated = core.compact_page(key).unwrap();
        assert_eq!(updated.compacted_through_sequence, 1);
        assert_eq!(core.work_counters().pages_compacted, 2);
        assert_eq!(core.work_counters().edit_candidates_replayed, 1);
    }
}
