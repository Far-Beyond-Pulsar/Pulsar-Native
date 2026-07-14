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

    /// Fold the current ordered edit prefix into one content-addressed LOD0
    /// page and publish that page into the canonical hierarchy.
    pub fn compact_page(&mut self, key: PageKey) -> Result<CompactedPageRecord, TerrainCoreError> {
        let previous_sequence = self
            .compacted
            .get(&key)
            .map_or(0, |record| record.compacted_through_sequence);
        let latest_sequence = self.edits.latest_sequence();
        if previous_sequence == latest_sequence {
            if let Some(record) = self.compacted.get(&key) {
                return Ok(*record);
            }
        }

        let relevant = self
            .edit_index
            .operations_for_page(&self.edits, key, previous_sequence);
        self.work.edit_candidates_replayed = self
            .work
            .edit_candidates_replayed
            .saturating_add(relevant.len() as u64);
        let page = if let Some(previous) = self.pages.get(&key) {
            self.work.cells_replayed = self
                .work
                .cells_replayed
                .saturating_add(crate::CELL_COUNT as u64);
            previous.apply_edit_tail(key, &relevant)?
        } else {
            self.work.cells_generated = self
                .work
                .cells_generated
                .saturating_add(crate::CELL_COUNT as u64);
            VoxelPage::generate_with_operations(key, &self.generator, &relevant)?
        };
        let page_id = page.page_id();
        let record = CompactedPageRecord {
            key,
            page_id,
            compacted_through_sequence: latest_sequence,
        };
        let state = page
            .constant_cell()
            .map_or(NodeState::Page(page_id), |cell| {
                if cell.is_solid() {
                    NodeState::Solid(cell.material())
                } else {
                    NodeState::Air
                }
            });
        self.hierarchy.set(key, state)?;
        self.pages.insert(key, page);
        self.compacted.insert(key, record);
        self.work.pages_compacted = self.work.pages_compacted.saturating_add(1);
        Ok(record)
    }

    pub fn page(&self, key: PageKey) -> Option<&VoxelPage> {
        self.pages.get(&key)
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
}
