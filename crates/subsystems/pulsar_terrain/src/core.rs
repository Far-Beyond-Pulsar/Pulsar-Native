use crate::edit::EditIndex;
use crate::mutation::{TerrainMutation, TerrainMutationBase, TerrainOverrideIndex};
use crate::{
    CompactedPageRecord, ContentHash, DeterministicGenerator, EditError, EditLog, EditOp,
    HierarchyError, NodeState, PageCodecError, PageKey, PlanetId, SnapshotCodecError,
    SparseBrickTree, TerrainOverrideError, TerrainOverrideLog, TerrainOverrideOp,
    TerrainOverrideTarget, TerrainSnapshot, VoxelPage,
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
    overrides: TerrainOverrideLog,
    override_index: TerrainOverrideIndex,
    latest_sequence: u64,
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
    pub override_candidates_replayed: u64,
    pub pages_rehydrated: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainMemoryCounters {
    pub hierarchy_nodes: usize,
    pub hierarchy_encoded_bytes: usize,
    pub edit_operations: usize,
    pub edit_attachment_regions: usize,
    pub edit_attachment_references: usize,
    pub override_operations: usize,
    pub override_attachment_regions: usize,
    pub override_attachment_references: usize,
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
    operations: Vec<TerrainMutation>,
}

#[derive(Clone, Debug)]
pub struct PageBuildResult {
    key: PageKey,
    page: VoxelPage,
    base_page_id: Option<crate::PageId>,
    previous_sequence: u64,
    target_sequence: u64,
    replayed_edits: usize,
    replayed_overrides: usize,
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
        let replayed_edits = self
            .operations
            .iter()
            .filter(|mutation| mutation.is_edit())
            .count();
        let replayed_overrides = self.operations.len().saturating_sub(replayed_edits);
        let reset = self
            .operations
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, mutation)| mutation.page_base(self.key).map(|base| (index, base)));
        let (page, reused_resident_page) = if let Some((index, base)) = reset {
            let tail = &self.operations[index + 1..];
            match base {
                TerrainMutationBase::Constant(cell) if tail.is_empty() => {
                    (VoxelPage::constant(cell), false)
                }
                TerrainMutationBase::Constant(cell) => (
                    VoxelPage::constant(cell).apply_mutation_tail(
                        self.key,
                        &self.generator,
                        tail,
                    )?,
                    false,
                ),
                TerrainMutationBase::Procedural => (
                    VoxelPage::generate_with_mutations(self.key, &self.generator, tail)?,
                    false,
                ),
            }
        } else if let Some(base_page) = self.base_page {
            (
                base_page.apply_mutation_tail(self.key, &self.generator, &self.operations)?,
                true,
            )
        } else {
            (
                VoxelPage::generate_with_mutations(self.key, &self.generator, &self.operations)?,
                false,
            )
        };
        Ok(PageBuildResult {
            key: self.key,
            page,
            base_page_id: self.base_page_id,
            previous_sequence: self.previous_sequence,
            target_sequence: self.target_sequence,
            replayed_edits,
            replayed_overrides,
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
            overrides: TerrainOverrideLog::default(),
            override_index: TerrainOverrideIndex::from_log(
                root_lod,
                &TerrainOverrideLog::default(),
            ),
            latest_sequence: 0,
            pages: BTreeMap::new(),
            compacted: BTreeMap::new(),
            work: TerrainWorkCounters::default(),
        })
    }

    pub fn from_snapshot(
        snapshot: TerrainSnapshot,
        generator: G,
    ) -> Result<Self, TerrainCoreError> {
        let _ = snapshot.encode()?;
        if snapshot.generator_hash != generator.hash() {
            return Err(TerrainCoreError::GeneratorMismatch);
        }
        for operation in snapshot.override_tail.operations() {
            operation.validate()?;
            if matches!(&operation.state, NodeState::Procedural(hash) if *hash != generator.hash())
            {
                return Err(TerrainCoreError::GeneratorMismatch);
            }
            if let TerrainOverrideTarget::Region(key) = operation.target {
                let _ = snapshot.hierarchy.resolve(key)?;
            }
        }
        let latest_sequence = snapshot
            .edit_tail
            .latest_sequence()
            .max(snapshot.override_tail.latest_sequence());
        let mut compacted = BTreeMap::new();
        for record in snapshot.compacted_pages {
            let _ = snapshot.hierarchy.resolve(record.key)?;
            if record.compacted_through_sequence > latest_sequence {
                return Err(TerrainCoreError::CompactedBeyondMutationTail {
                    compacted: record.compacted_through_sequence,
                    latest: latest_sequence,
                });
            }
            compacted.insert(record.key, record);
        }
        let root_lod = snapshot.hierarchy.root_lod();
        let edit_index = EditIndex::from_log(root_lod, &snapshot.edit_tail);
        let override_index = TerrainOverrideIndex::from_log(root_lod, &snapshot.override_tail);
        Ok(Self {
            planet_id: snapshot.planet_id,
            generator,
            hierarchy: snapshot.hierarchy,
            edits: snapshot.edit_tail,
            edit_index,
            overrides: snapshot.override_tail,
            override_index,
            latest_sequence,
            pages: BTreeMap::new(),
            compacted,
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

    pub fn override_log(&self) -> &TerrainOverrideLog {
        &self.overrides
    }

    pub fn latest_sequence(&self) -> u64 {
        self.latest_sequence
    }

    pub fn append_edit(&mut self, operation: EditOp) -> Result<(), TerrainCoreError> {
        if let Some(existing) = self.edits.operation_with_id(operation.stable_id) {
            return if *existing == operation {
                Ok(())
            } else {
                Err(TerrainCoreError::DuplicateMutationId)
            };
        }
        if self
            .overrides
            .operation_with_id(operation.stable_id)
            .is_some()
        {
            return Err(TerrainCoreError::DuplicateMutationId);
        }
        self.validate_next_sequence(operation.sequence)?;
        let previous_len = self.edits.operations().len();
        self.edits.push(operation)?;
        if self.edits.operations().len() != previous_len {
            self.edit_index.insert(previous_len, operation);
            self.latest_sequence = operation.sequence;
            self.work.edits_appended = self.work.edits_appended.saturating_add(1);
        }
        Ok(())
    }

    pub fn append_override(
        &mut self,
        operation: TerrainOverrideOp,
    ) -> Result<(), TerrainCoreError> {
        operation.validate()?;
        if let NodeState::Procedural(hash) = &operation.state {
            if *hash != self.generator.hash() {
                return Err(TerrainCoreError::GeneratorMismatch);
            }
        }
        if let Some(existing) = self.overrides.operation_with_id(operation.stable_id) {
            return if *existing == operation {
                Ok(())
            } else {
                Err(TerrainCoreError::DuplicateMutationId)
            };
        }
        if self.edits.operation_with_id(operation.stable_id).is_some() {
            return Err(TerrainCoreError::DuplicateMutationId);
        }
        self.validate_next_sequence(operation.sequence)?;

        match operation.target {
            TerrainOverrideTarget::Root => self.hierarchy.set_root(operation.state.clone())?,
            TerrainOverrideTarget::Region(key) => {
                self.hierarchy.set(key, operation.state.clone())?
            }
        }
        let operation_index = self.overrides.operations().len();
        self.overrides.push(operation.clone())?;
        self.override_index.insert(operation_index, &operation);
        self.latest_sequence = operation.sequence;
        self.work.hierarchy_overrides = self.work.hierarchy_overrides.saturating_add(1);
        Ok(())
    }

    pub fn prepare_page_build(
        &self,
        key: PageKey,
    ) -> Result<PageBuildPreparation<G>, TerrainCoreError>
    where
        G: Clone,
    {
        let _ = self.hierarchy.resolve(key)?;
        let previous_sequence = self
            .compacted
            .get(&key)
            .map_or(0, |record| record.compacted_through_sequence);
        let latest_sequence = self.latest_sequence;
        if previous_sequence == latest_sequence {
            if let Some(record) = self.compacted.get(&key) {
                if self.pages.contains_key(&key) {
                    return Ok(PageBuildPreparation::Current(*record));
                }
            }
        }

        let base_page = self.pages.get(&key).cloned();
        // Dense pages are a disposable cache. If one was evicted, rebuild it
        // from the deterministic source and full relevant mutation prefix.
        let replay_from_sequence = if base_page.is_some() {
            previous_sequence
        } else {
            0
        };
        let operations = self.mutations_for_page(key, replay_from_sequence);
        if base_page.is_some() && operations.is_empty() {
            return Ok(PageBuildPreparation::Current(
                *self
                    .compacted
                    .get(&key)
                    .expect("every resident page has a compacted record"),
            ));
        }
        Ok(PageBuildPreparation::Build(PageBuildRequest {
            key,
            generator: self.generator.clone(),
            base_page_id: base_page.as_ref().map(VoxelPage::page_id),
            base_page,
            previous_sequence: replay_from_sequence,
            target_sequence: latest_sequence,
            operations,
        }))
    }

    pub fn commit_page_build(
        &mut self,
        result: PageBuildResult,
    ) -> Result<PageBuildCommitOutcome, TerrainCoreError> {
        let latest_sequence = self.latest_sequence;
        if result.target_sequence > latest_sequence {
            return Ok(PageBuildCommitOutcome::Stale {
                newest_sequence: latest_sequence,
            });
        }
        if let Some(newest_sequence) = self
            .mutations_for_page(result.key, result.target_sequence)
            .last()
            .map(TerrainMutation::sequence)
        {
            return Ok(PageBuildCommitOutcome::Stale { newest_sequence });
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
                        .saturating_add(result.replayed_edits as u64);
                    self.work.override_candidates_replayed = self
                        .work
                        .override_candidates_replayed
                        .saturating_add(result.replayed_overrides as u64);
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
            compacted_through_sequence: latest_sequence,
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
            .saturating_add(result.replayed_edits as u64);
        self.work.override_candidates_replayed = self
            .work
            .override_candidates_replayed
            .saturating_add(result.replayed_overrides as u64);
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

    /// Fold the current ordered mutation prefix into one content-addressed page at
    /// its requested LOD and publish it into the canonical hierarchy.
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

    pub fn evict_all_resident_pages(&mut self) -> usize {
        let count = self.pages.len();
        self.pages.clear();
        count
    }

    /// Exact whole-root replacement. Resident pages remain disposable caches;
    /// the root state is immediately authoritative without iterating over them.
    pub fn set_root(&mut self, state: NodeState) -> Result<(), TerrainCoreError> {
        let sequence = self.next_sequence()?;
        let operation = self.generated_override(sequence, TerrainOverrideTarget::Root, state);
        self.append_override(operation)
    }

    /// Attach an exact uniform or content-addressed override at any hierarchy
    /// level. Descendants are resolved lazily and never expanded here.
    pub fn set_region(&mut self, key: PageKey, state: NodeState) -> Result<(), TerrainCoreError> {
        let sequence = self.next_sequence()?;
        let operation =
            self.generated_override(sequence, TerrainOverrideTarget::Region(key), state);
        self.append_override(operation)
    }

    pub fn snapshot(&self) -> TerrainSnapshot {
        TerrainSnapshot {
            planet_id: self.planet_id,
            generator_hash: self.generator.hash(),
            hierarchy: self.hierarchy.clone(),
            edit_tail: self.edits.clone(),
            override_tail: self.overrides.clone(),
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
            override_operations: self.overrides.operations().len(),
            override_attachment_regions: self.override_index.region_count(),
            override_attachment_references: self.override_index.reference_count(),
            resident_pages: self.pages.len(),
            resident_dense_bytes: self
                .pages
                .values()
                .map(VoxelPage::dense_allocation_bytes)
                .sum(),
            compacted_page_records: self.compacted.len(),
        }
    }

    fn validate_next_sequence(&self, sequence: u64) -> Result<(), TerrainCoreError> {
        if sequence <= self.latest_sequence {
            return Err(TerrainCoreError::OutOfOrderMutation {
                latest: self.latest_sequence,
                received: sequence,
            });
        }
        Ok(())
    }

    fn next_sequence(&self) -> Result<u64, TerrainCoreError> {
        self.latest_sequence
            .checked_add(1)
            .ok_or(TerrainCoreError::SequenceOverflow)
    }

    fn generated_override(
        &self,
        sequence: u64,
        target: TerrainOverrideTarget,
        state: NodeState,
    ) -> TerrainOverrideOp {
        let mut bytes = Vec::with_capacity(96);
        bytes.extend_from_slice(b"pulsar.terrain.override.v1");
        bytes.extend_from_slice(&self.planet_id.0);
        bytes.extend_from_slice(&sequence.to_le_bytes());
        match target {
            TerrainOverrideTarget::Root => bytes.push(0),
            TerrainOverrideTarget::Region(key) => {
                bytes.push(1);
                bytes.push(key.lod);
                for axis in key.page_xyz {
                    bytes.extend_from_slice(&axis.to_le_bytes());
                }
            }
        }
        match &state {
            NodeState::Air => bytes.push(0),
            NodeState::Solid(material) => {
                bytes.push(1);
                bytes.push(*material);
            }
            NodeState::Procedural(hash) => {
                bytes.push(2);
                bytes.extend_from_slice(&hash.0);
            }
            NodeState::Branch => bytes.push(3),
            NodeState::Page(hash) => {
                bytes.push(4);
                bytes.extend_from_slice(&hash.0);
            }
        }
        let hash = ContentHash::of(&bytes);
        let mut stable_id = [0_u8; 16];
        stable_id.copy_from_slice(&hash.0[..16]);
        TerrainOverrideOp {
            sequence,
            stable_id,
            target,
            state,
        }
    }

    fn mutations_for_page(&self, key: PageKey, after_sequence: u64) -> Vec<TerrainMutation> {
        let relevant_overrides =
            self.override_index
                .operations_for_page(&self.overrides, key, after_sequence);
        let reset = relevant_overrides
            .iter()
            .enumerate()
            .rev()
            .find(|(_, operation)| operation.page_base(key).is_some());
        let replay_after = reset.map_or(after_sequence, |(_, operation)| operation.sequence);
        let relevant_edits = self
            .edit_index
            .operations_for_page(&self.edits, key, replay_after);
        let mut operations = Vec::with_capacity(
            relevant_edits
                .len()
                .saturating_add(relevant_overrides.len()),
        );
        if let Some((index, operation)) = reset {
            operations.push(TerrainMutation::Override(operation.clone()));
            operations.extend(
                relevant_overrides
                    .into_iter()
                    .skip(index + 1)
                    .map(TerrainMutation::Override),
            );
        } else {
            operations.extend(
                relevant_overrides
                    .into_iter()
                    .map(TerrainMutation::Override),
            );
        }
        operations.extend(relevant_edits.into_iter().map(TerrainMutation::Edit));
        operations.sort_unstable_by_key(TerrainMutation::sequence);
        operations
    }
}

#[derive(Debug, Error)]
pub enum TerrainCoreError {
    #[error(transparent)]
    Hierarchy(#[from] HierarchyError),
    #[error(transparent)]
    Edit(#[from] EditError),
    #[error(transparent)]
    Override(#[from] TerrainOverrideError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotCodecError),
    #[error(transparent)]
    Page(#[from] PageCodecError),
    #[error("terrain mutation sequence {received} must be greater than {latest}")]
    OutOfOrderMutation { latest: u64, received: u64 },
    #[error("terrain mutation id was already used by a different operation")]
    DuplicateMutationId,
    #[error("terrain mutation sequence counter overflowed")]
    SequenceOverflow,
    #[error("terrain procedural override does not match the active generator")]
    GeneratorMismatch,
    #[error("compacted page sequence {compacted} exceeds the canonical mutation tail {latest}")]
    CompactedBeyondMutationTail { compacted: u64, latest: u64 },
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
            sequence: 3,
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
        assert_eq!(updated.compacted_through_sequence, 3);
        assert_eq!(
            core.work_counters().cells_generated,
            (crate::CELL_COUNT * 2) as u64
        );
        assert_eq!(core.work_counters().cells_replayed, 0);
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
    fn unrelated_mutation_does_not_stale_an_off_thread_page_build() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([16; 16]), 12, generator).unwrap();
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
        let result = match core.prepare_page_build(key).unwrap() {
            PageBuildPreparation::Build(request) => request.execute().unwrap(),
            PageBuildPreparation::Current(_) => panic!("first request must require a build"),
        };

        core.append_edit(EditOp {
            sequence: 2,
            stable_id: [2; 16],
            shape: EditShape::Sphere {
                center_cell: [3_204; 3],
                radius_cells: 1,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();

        let record = match core.commit_page_build(result).unwrap() {
            PageBuildCommitOutcome::Committed(record) => record,
            outcome => panic!("unrelated edit unexpectedly invalidated the page: {outcome:?}"),
        };
        assert_eq!(record.compacted_through_sequence, 2);
        assert!(core.page(key).is_some());
    }

    #[test]
    fn unrelated_mutation_does_not_rebuild_a_resident_page() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([17; 16]), 12, generator).unwrap();
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
        let original = core.compact_page(key).unwrap();
        let compacted_before = core.work_counters().pages_compacted;

        core.append_edit(EditOp {
            sequence: 2,
            stable_id: [2; 16],
            shape: EditShape::Sphere {
                center_cell: [3_204; 3],
                radius_cells: 1,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();

        assert_eq!(core.compact_page(key).unwrap(), original);
        assert_eq!(core.work_counters().pages_compacted, compacted_before);
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

    #[test]
    fn coarse_compaction_replays_edits_attached_to_fine_descendants() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 10_000,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([9; 16]), 12, generator).unwrap();
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [1; 16],
            shape: EditShape::Sphere {
                center_cell: [16; 3],
                radius_cells: 1,
            },
            mode: EditMode::Subtract,
            material: 0,
        })
        .unwrap();

        let key = PageKey::new(2, [0; 3]);
        core.compact_page(key).unwrap();
        let edited_sample = core.page(key).unwrap().get([4; 3]).unwrap();
        assert!(!edited_sample.is_solid());
        assert_eq!(core.work_counters().edit_candidates_replayed, 1);
        assert_eq!(core.memory_counters().resident_pages, 1);
    }

    #[test]
    fn root_delete_is_authoritative_across_eviction_and_later_union() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 10_000,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([10; 16]), 12, generator).unwrap();
        let key = PageKey::new(0, [-1, 0, 0]);
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [1; 16],
            shape: EditShape::Sphere {
                center_cell: [-16, 16, 16],
                radius_cells: 4,
            },
            mode: EditMode::Paint,
            material: 9,
        })
        .unwrap();
        core.compact_page(key).unwrap();

        core.set_root(NodeState::Air).unwrap();
        assert_eq!(core.latest_sequence(), 2);
        assert!(core.evict_resident_page(key));
        let deleted = core.compact_page(key).unwrap();
        assert_eq!(deleted.compacted_through_sequence, 2);
        assert_eq!(
            core.page(key).unwrap().constant_cell(),
            Some(crate::CellWord::AIR)
        );
        assert_eq!(core.work_counters().edit_candidates_replayed, 1);
        assert_eq!(core.work_counters().override_candidates_replayed, 1);

        core.append_edit(EditOp {
            sequence: 3,
            stable_id: [3; 16],
            shape: EditShape::Sphere {
                center_cell: [-16, 16, 16],
                radius_cells: 3,
            },
            mode: EditMode::Union,
            material: 12,
        })
        .unwrap();
        let rebuilt = core.compact_page(key).unwrap();
        assert_eq!(rebuilt.compacted_through_sequence, 3);
        let sample = core.page(key).unwrap().get([16, 16, 16]).unwrap();
        assert!(sample.is_solid());
        assert_eq!(sample.material(), 12);
        assert_eq!(core.work_counters().edit_candidates_replayed, 2);
        assert_eq!(core.work_counters().override_candidates_replayed, 1);
    }

    #[test]
    fn procedural_root_reset_discards_older_materialized_edits() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([18; 16]), 12, generator).unwrap();
        let key = PageKey::new(0, [1_500, 0, 0]);
        core.append_edit(EditOp {
            sequence: 1,
            stable_id: [1; 16],
            shape: EditShape::Sphere {
                center_cell: [48_016, 16, 16],
                radius_cells: 2,
            },
            mode: EditMode::Union,
            material: 12,
        })
        .unwrap();
        core.compact_page(key).unwrap();
        assert!(core.page(key).unwrap().get([16; 3]).unwrap().is_solid());

        core.set_root(NodeState::Procedural(generator.hash()))
            .unwrap();
        assert!(core.evict_resident_page(key));
        core.compact_page(key).unwrap();
        assert_eq!(
            core.page(key).unwrap().constant_cell(),
            Some(crate::CellWord::AIR)
        );
        assert_eq!(core.work_counters().edit_candidates_replayed, 1);
        assert_eq!(core.work_counters().override_candidates_replayed, 1);
    }

    #[test]
    fn snapshot_restore_rehydrates_deleted_root_to_identical_hash() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 10_000,
            material: 5,
        };
        let key = PageKey::new(0, [0; 3]);
        let mut core = TerrainCore::new(PlanetId([11; 16]), 12, generator).unwrap();
        core.compact_page(key).unwrap();
        core.set_root(NodeState::Air).unwrap();
        core.compact_page(key).unwrap();
        let snapshot = core.snapshot();
        let canonical_hash = snapshot.content_hash().unwrap();
        let bytes = snapshot.encode().unwrap();
        let decoded = TerrainSnapshot::decode(&bytes).unwrap();
        assert_eq!(decoded.content_hash().unwrap(), canonical_hash);

        let mut restored = TerrainCore::from_snapshot(decoded, generator).unwrap();
        let record = restored.compact_page(key).unwrap();
        assert_eq!(record.compacted_through_sequence, 1);
        assert_eq!(
            restored.page(key).unwrap().constant_cell(),
            Some(crate::CellWord::AIR)
        );
        assert_eq!(restored.snapshot().content_hash().unwrap(), canonical_hash);
    }

    #[test]
    fn signed_region_override_applies_only_inside_aligned_cube() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 10_000,
            material: 6,
        };
        let mut core = TerrainCore::new(PlanetId([12; 16]), 12, generator).unwrap();
        let region = PageKey::new(2, [-1, -1, -1]);
        core.set_region(region, NodeState::Air).unwrap();

        let inside = PageKey::new(0, [-1, -1, -1]);
        let outside = PageKey::new(0, [0, -1, -1]);
        core.compact_page(inside).unwrap();
        core.compact_page(outside).unwrap();
        assert_eq!(
            core.page(inside).unwrap().constant_cell(),
            Some(crate::CellWord::AIR)
        );
        assert!(core
            .page(outside)
            .unwrap()
            .get([0, 0, 0])
            .unwrap()
            .is_solid());

        let coarse = PageKey::new(3, [-1, -1, -1]);
        core.compact_page(coarse).unwrap();
        let coarse_page = core.page(coarse).unwrap();
        assert!(coarse_page.get([15; 3]).unwrap().is_solid());
        assert!(!coarse_page.get([16; 3]).unwrap().is_solid());
    }

    #[test]
    fn override_stale_rejects_pre_delete_worker_result() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([13; 16]), 12, generator).unwrap();
        let key = PageKey::new(0, [0; 3]);
        let result = match core.prepare_page_build(key).unwrap() {
            PageBuildPreparation::Build(request) => request.execute().unwrap(),
            PageBuildPreparation::Current(_) => panic!("first build cannot be current"),
        };
        core.set_root(NodeState::Air).unwrap();
        assert_eq!(
            core.commit_page_build(result).unwrap(),
            PageBuildCommitOutcome::Stale { newest_sequence: 1 }
        );
        assert!(core.page(key).is_none());
    }

    #[test]
    fn edit_and_override_share_one_sequence_and_id_domain() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 3,
        };
        let mut core = TerrainCore::new(PlanetId([15; 16]), 12, generator).unwrap();
        core.append_override(TerrainOverrideOp {
            sequence: 1,
            stable_id: [1; 16],
            target: TerrainOverrideTarget::Root,
            state: NodeState::Air,
        })
        .unwrap();
        assert!(matches!(
            core.append_edit(EditOp {
                sequence: 1,
                stable_id: [2; 16],
                shape: EditShape::Sphere {
                    center_cell: [0; 3],
                    radius_cells: 1,
                },
                mode: EditMode::Union,
                material: 4,
            }),
            Err(TerrainCoreError::OutOfOrderMutation {
                latest: 1,
                received: 1
            })
        ));
        assert!(matches!(
            core.append_edit(EditOp {
                sequence: 2,
                stable_id: [1; 16],
                shape: EditShape::Sphere {
                    center_cell: [0; 3],
                    radius_cells: 1,
                },
                mode: EditMode::Union,
                material: 4,
            }),
            Err(TerrainCoreError::DuplicateMutationId)
        ));
    }

    #[test]
    fn randomized_mutation_replay_matches_snapshot_restore() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 10_000,
            material: 2,
        };
        let mut core = TerrainCore::new(PlanetId([14; 16]), 12, generator).unwrap();
        let mut random = 0x4d59_5df4_d0f3_3173_u64;
        for sequence in 1..=32_u64 {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            if sequence % 7 == 0 {
                let coordinate = ((random >> 32) as i32 % 4) as i64;
                core.append_override(TerrainOverrideOp {
                    sequence,
                    stable_id: [sequence as u8; 16],
                    target: TerrainOverrideTarget::Region(PageKey::new(
                        1,
                        [coordinate, -coordinate, coordinate],
                    )),
                    state: if sequence % 14 == 0 {
                        NodeState::Air
                    } else {
                        NodeState::Solid(8)
                    },
                })
                .unwrap();
            } else {
                let center = [
                    ((random >> 8) as i8 as i64) * 2,
                    ((random >> 24) as i8 as i64) * 2,
                    ((random >> 40) as i8 as i64) * 2,
                ];
                core.append_edit(EditOp {
                    sequence,
                    stable_id: [sequence as u8; 16],
                    shape: EditShape::Sphere {
                        center_cell: center,
                        radius_cells: 3,
                    },
                    mode: if sequence % 2 == 0 {
                        EditMode::Subtract
                    } else {
                        EditMode::Union
                    },
                    material: 11,
                })
                .unwrap();
            }
        }

        let keys = [
            PageKey::new(0, [-2, -1, 0]),
            PageKey::new(0, [0, 0, 0]),
            PageKey::new(2, [-1, 0, 1]),
        ];
        let expected = keys.map(|key| core.compact_page(key).unwrap().page_id);
        let bytes = core.snapshot().encode().unwrap();
        let mut restored =
            TerrainCore::from_snapshot(TerrainSnapshot::decode(&bytes).unwrap(), generator)
                .unwrap();
        let actual = keys.map(|key| restored.compact_page(key).unwrap().page_id);
        assert_eq!(actual, expected);
        assert_eq!(
            restored.snapshot().content_hash().unwrap(),
            core.snapshot().content_hash().unwrap()
        );
    }
}
