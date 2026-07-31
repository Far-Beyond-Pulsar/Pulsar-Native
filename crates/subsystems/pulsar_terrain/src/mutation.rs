use crate::{CellWord, ContentHash, DeterministicGenerator, EditOp, NodeState, PageKey};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const OVERRIDE_LOG_MAGIC: &[u8; 8] = b"PTOVRD01";
const OVERRIDE_RECORD_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainOverrideTarget {
    Root,
    Region(PageKey),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainOverrideOp {
    pub sequence: u64,
    pub stable_id: [u8; 16],
    pub target: TerrainOverrideTarget,
    pub state: NodeState,
}

impl TerrainOverrideOp {
    pub(crate) fn validate(&self) -> Result<(), TerrainOverrideError> {
        match &self.state {
            NodeState::Air | NodeState::Solid(_) | NodeState::Procedural(_) => Ok(()),
            NodeState::Branch | NodeState::Page(_) => Err(TerrainOverrideError::NonUniformState),
        }
    }

    fn applies_to_cell(&self, cell_xyz: [i64; 3]) -> bool {
        let TerrainOverrideTarget::Region(key) = self.target else {
            return true;
        };
        let Some(min) = key.lod0_cell_min() else {
            return false;
        };
        let Some(span) = key.lod0_cell_span() else {
            return false;
        };
        (0..3).all(|axis| {
            cell_xyz[axis] >= min[axis] && cell_xyz[axis] < min[axis].saturating_add(span)
        })
    }

    pub(crate) fn page_base(&self, key: PageKey) -> Option<TerrainMutationBase> {
        let covers_page = match self.target {
            TerrainOverrideTarget::Root => true,
            TerrainOverrideTarget::Region(region) => {
                let region_min = region.lod0_cell_min()?;
                let region_span = region.lod0_cell_span()?;
                let page_min = key.lod0_cell_min()?;
                let page_span = key.lod0_cell_span()?;
                (0..3).all(|axis| {
                    page_min[axis] >= region_min[axis]
                        && page_min[axis].saturating_add(page_span)
                            <= region_min[axis].saturating_add(region_span)
                })
            }
        };
        if !covers_page {
            return None;
        }
        match &self.state {
            NodeState::Air => Some(TerrainMutationBase::Constant(CellWord::AIR)),
            NodeState::Solid(material) => Some(TerrainMutationBase::Constant(CellWord::new(
                i16::MIN,
                *material,
                0,
            ))),
            NodeState::Procedural(_) => Some(TerrainMutationBase::Procedural),
            NodeState::Branch | NodeState::Page(_) => {
                unreachable!("override state is validated before publication")
            }
        }
    }

    fn apply<G: DeterministicGenerator>(
        &self,
        cell_xyz: [i64; 3],
        cell: CellWord,
        generator: &G,
    ) -> CellWord {
        if !self.applies_to_cell(cell_xyz) {
            return cell;
        }
        match &self.state {
            NodeState::Air => CellWord::AIR,
            NodeState::Solid(material) => CellWord::new(i16::MIN, *material, 0),
            NodeState::Procedural(_) => generator.sample_cell(cell_xyz),
            NodeState::Branch | NodeState::Page(_) => {
                unreachable!("override state is validated before publication")
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerrainMutation {
    Edit(EditOp),
    Override(TerrainOverrideOp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerrainMutationBase {
    Constant(CellWord),
    Procedural,
}

impl TerrainMutation {
    pub(crate) fn sequence(&self) -> u64 {
        match self {
            Self::Edit(operation) => operation.sequence,
            Self::Override(operation) => operation.sequence,
        }
    }

    pub(crate) fn is_edit(&self) -> bool {
        matches!(self, Self::Edit(_))
    }

    pub(crate) fn apply<G: DeterministicGenerator>(
        &self,
        cell_xyz: [i64; 3],
        cell: CellWord,
        generator: &G,
    ) -> CellWord {
        match self {
            Self::Edit(operation) => {
                let (min, max) = operation.shape.bounds();
                if (0..3).all(|axis| cell_xyz[axis] >= min[axis] && cell_xyz[axis] < max[axis]) {
                    operation.apply(cell_xyz, cell)
                } else {
                    cell
                }
            }
            Self::Override(operation) => operation.apply(cell_xyz, cell, generator),
        }
    }

    pub(crate) fn page_base(&self, key: PageKey) -> Option<TerrainMutationBase> {
        match self {
            Self::Edit(_) => None,
            Self::Override(operation) => operation.page_base(key),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainOverrideLog {
    operations: Vec<TerrainOverrideOp>,
    ids: BTreeMap<[u8; 16], usize>,
}

impl TerrainOverrideLog {
    pub fn push(&mut self, operation: TerrainOverrideOp) -> Result<(), TerrainOverrideError> {
        operation.validate()?;
        if let Some(existing) = self.operation_with_id(operation.stable_id) {
            return if *existing == operation {
                Ok(())
            } else {
                Err(TerrainOverrideError::DuplicateId)
            };
        }
        if let Some(latest) = self.operations.last().map(|operation| operation.sequence) {
            if operation.sequence <= latest {
                return Err(TerrainOverrideError::OutOfOrder {
                    latest,
                    received: operation.sequence,
                });
            }
        }
        self.ids.insert(operation.stable_id, self.operations.len());
        self.operations.push(operation);
        Ok(())
    }

    pub fn operations(&self) -> &[TerrainOverrideOp] {
        &self.operations
    }

    pub fn latest_sequence(&self) -> u64 {
        self.operations
            .last()
            .map_or(0, |operation| operation.sequence)
    }

    pub(crate) fn operation_with_id(&self, stable_id: [u8; 16]) -> Option<&TerrainOverrideOp> {
        self.ids
            .get(&stable_id)
            .and_then(|index| self.operations.get(*index))
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(12 + self.operations.len() * OVERRIDE_RECORD_BYTES);
        output.extend_from_slice(OVERRIDE_LOG_MAGIC);
        output.extend_from_slice(&(self.operations.len() as u32).to_le_bytes());
        for operation in &self.operations {
            output.extend_from_slice(&operation.sequence.to_le_bytes());
            output.extend_from_slice(&operation.stable_id);
            match operation.target {
                TerrainOverrideTarget::Root => {
                    output.push(0);
                    output.push(state_tag(&operation.state));
                    output.push(0);
                    output.extend_from_slice(&[0; 5]);
                    output.extend_from_slice(&[0; 24]);
                }
                TerrainOverrideTarget::Region(key) => {
                    output.push(1);
                    output.push(state_tag(&operation.state));
                    output.push(key.lod);
                    output.extend_from_slice(&[0; 5]);
                    for axis in key.page_xyz {
                        output.extend_from_slice(&axis.to_le_bytes());
                    }
                }
            }
            let mut payload = [0_u8; 32];
            match &operation.state {
                NodeState::Air => {}
                NodeState::Solid(material) => payload[0] = *material,
                NodeState::Procedural(hash) => payload = hash.0,
                NodeState::Branch | NodeState::Page(_) => {
                    unreachable!("override state is validated before encoding")
                }
            }
            output.extend_from_slice(&payload);
        }
        output
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, TerrainOverrideError> {
        if bytes.len() < 12 || bytes.get(..8) != Some(OVERRIDE_LOG_MAGIC) {
            return Err(TerrainOverrideError::Codec);
        }
        let count = u32::from_le_bytes(
            bytes[8..12]
                .try_into()
                .map_err(|_| TerrainOverrideError::Codec)?,
        ) as usize;
        let expected = 12_usize
            .checked_add(
                count
                    .checked_mul(OVERRIDE_RECORD_BYTES)
                    .ok_or(TerrainOverrideError::Codec)?,
            )
            .ok_or(TerrainOverrideError::Codec)?;
        if bytes.len() != expected {
            return Err(TerrainOverrideError::Codec);
        }

        let mut log = Self::default();
        let mut cursor = 12;
        for _ in 0..count {
            let sequence = read_u64(bytes, cursor)?;
            let stable_id = read_array(bytes, cursor + 8)?;
            let target_tag = bytes[cursor + 24];
            let state_tag = bytes[cursor + 25];
            let lod = bytes[cursor + 26];
            if bytes.get(cursor + 27..cursor + 32) != Some(&[0; 5]) {
                return Err(TerrainOverrideError::Codec);
            }
            let page_xyz = [
                read_i64(bytes, cursor + 32)?,
                read_i64(bytes, cursor + 40)?,
                read_i64(bytes, cursor + 48)?,
            ];
            let payload: [u8; 32] = read_array(bytes, cursor + 56)?;
            let target = match target_tag {
                0 if lod == 0 && page_xyz == [0; 3] => TerrainOverrideTarget::Root,
                1 => TerrainOverrideTarget::Region(PageKey::new(lod, page_xyz)),
                _ => return Err(TerrainOverrideError::Codec),
            };
            let state = match state_tag {
                0 if payload == [0; 32] => NodeState::Air,
                1 if payload[1..] == [0; 31] => NodeState::Solid(payload[0]),
                2 => NodeState::Procedural(ContentHash(payload)),
                _ => return Err(TerrainOverrideError::Codec),
            };
            log.push(TerrainOverrideOp {
                sequence,
                stable_id,
                target,
                state,
            })?;
            cursor += OVERRIDE_RECORD_BYTES;
        }
        Ok(log)
    }
}

fn state_tag(state: &NodeState) -> u8 {
    match state {
        NodeState::Air => 0,
        NodeState::Solid(_) => 1,
        NodeState::Procedural(_) => 2,
        NodeState::Branch | NodeState::Page(_) => {
            unreachable!("override state is validated before encoding")
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TerrainOverrideIndex {
    root_lod: u8,
    root: Vec<usize>,
    regions: BTreeMap<PageKey, Vec<usize>>,
    occupied_subtrees: BTreeSet<PageKey>,
}

impl TerrainOverrideIndex {
    pub(crate) fn from_log(root_lod: u8, log: &TerrainOverrideLog) -> Self {
        let mut index = Self {
            root_lod,
            root: Vec::new(),
            regions: BTreeMap::new(),
            occupied_subtrees: BTreeSet::new(),
        };
        for (operation_index, operation) in log.operations().iter().enumerate() {
            index.insert(operation_index, operation);
        }
        index
    }

    pub(crate) fn insert(&mut self, operation_index: usize, operation: &TerrainOverrideOp) {
        match operation.target {
            TerrainOverrideTarget::Root => self.root.push(operation_index),
            TerrainOverrideTarget::Region(key) => {
                self.regions.entry(key).or_default().push(operation_index);
                let mut ancestor = Some(key);
                while let Some(node) = ancestor.filter(|node| node.lod < self.root_lod) {
                    self.occupied_subtrees.insert(node);
                    ancestor = node.parent();
                }
            }
        }
    }

    pub(crate) fn operations_for_page(
        &self,
        log: &TerrainOverrideLog,
        key: PageKey,
        after_sequence: u64,
    ) -> Vec<TerrainOverrideOp> {
        let mut indices = Vec::new();
        Self::extend_after(log, &self.root, after_sequence, &mut indices);
        let mut ancestor = Some(key);
        while let Some(node) = ancestor.filter(|node| node.lod < self.root_lod) {
            if let Some(attached) = self.regions.get(&node) {
                Self::extend_after(log, attached, after_sequence, &mut indices);
            }
            ancestor = node.parent();
        }
        self.collect_descendant_indices(log, key, after_sequence, &mut indices);
        indices.sort_unstable();
        indices.dedup();
        indices
            .into_iter()
            .filter_map(|index| log.operations().get(index).cloned())
            .collect()
    }

    fn collect_descendant_indices(
        &self,
        log: &TerrainOverrideLog,
        parent: PageKey,
        after_sequence: u64,
        indices: &mut Vec<usize>,
    ) {
        let Some(child_lod) = parent.lod.checked_sub(1) else {
            return;
        };
        for z in 0..2_i64 {
            for y in 0..2_i64 {
                for x in 0..2_i64 {
                    let offsets = [x, y, z];
                    let mut page_xyz = [0_i64; 3];
                    let mut valid = true;
                    for axis in 0..3 {
                        let Some(coordinate) = parent.page_xyz[axis]
                            .checked_mul(2)
                            .and_then(|coordinate| coordinate.checked_add(offsets[axis]))
                        else {
                            valid = false;
                            break;
                        };
                        page_xyz[axis] = coordinate;
                    }
                    if !valid {
                        continue;
                    }
                    let child = PageKey::new(child_lod, page_xyz);
                    if !self.occupied_subtrees.contains(&child) {
                        continue;
                    }
                    if let Some(attached) = self.regions.get(&child) {
                        Self::extend_after(log, attached, after_sequence, indices);
                    }
                    self.collect_descendant_indices(log, child, after_sequence, indices);
                }
            }
        }
    }

    fn extend_after(
        log: &TerrainOverrideLog,
        attached: &[usize],
        after_sequence: u64,
        indices: &mut Vec<usize>,
    ) {
        let first = attached.partition_point(|index| {
            log.operations()
                .get(*index)
                .is_some_and(|operation| operation.sequence <= after_sequence)
        });
        indices.extend_from_slice(&attached[first..]);
    }

    pub(crate) fn region_count(&self) -> usize {
        self.regions.len() + usize::from(!self.root.is_empty())
    }

    pub(crate) fn reference_count(&self) -> usize {
        self.root.len() + self.regions.values().map(Vec::len).sum::<usize>()
    }
}

fn read_array<const N: usize>(
    bytes: &[u8],
    offset: usize,
) -> Result<[u8; N], TerrainOverrideError> {
    bytes
        .get(offset..offset + N)
        .ok_or(TerrainOverrideError::Codec)?
        .try_into()
        .map_err(|_| TerrainOverrideError::Codec)
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, TerrainOverrideError> {
    Ok(u64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, TerrainOverrideError> {
    Ok(i64::from_le_bytes(read_array(bytes, offset)?))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TerrainOverrideError {
    #[error("terrain override sequence {received} must be greater than {latest}")]
    OutOfOrder { latest: u64, received: u64 },
    #[error("terrain override id was already applied at a different sequence")]
    DuplicateId,
    #[error("terrain overrides support only Air, Solid, or Procedural uniform states")]
    NonUniformState,
    #[error("invalid canonical terrain override-log encoding")]
    Codec,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_log_round_trip_is_canonical_across_signed_regions() {
        let mut log = TerrainOverrideLog::default();
        log.push(TerrainOverrideOp {
            sequence: 4,
            stable_id: [4; 16],
            target: TerrainOverrideTarget::Root,
            state: NodeState::Air,
        })
        .unwrap();
        log.push(TerrainOverrideOp {
            sequence: 9,
            stable_id: [9; 16],
            target: TerrainOverrideTarget::Region(PageKey::new(7, [-2, 3, -4])),
            state: NodeState::Solid(17),
        })
        .unwrap();
        let encoded = log.encode();
        assert_eq!(TerrainOverrideLog::decode(&encoded).unwrap(), log);

        let mut noncanonical = encoded;
        noncanonical[12 + 27] = 1;
        assert_eq!(
            TerrainOverrideLog::decode(&noncanonical),
            Err(TerrainOverrideError::Codec)
        );
    }

    #[test]
    fn index_finds_ancestor_and_descendant_overrides_without_global_replay() {
        let mut log = TerrainOverrideLog::default();
        for (sequence, target) in [
            (1, TerrainOverrideTarget::Root),
            (
                2,
                TerrainOverrideTarget::Region(PageKey::new(4, [-1, 0, 0])),
            ),
            (
                3,
                TerrainOverrideTarget::Region(PageKey::new(0, [-3, 2, 1])),
            ),
        ] {
            log.push(TerrainOverrideOp {
                sequence,
                stable_id: [sequence as u8; 16],
                target,
                state: NodeState::Air,
            })
            .unwrap();
        }
        let index = TerrainOverrideIndex::from_log(12, &log);
        let sequences = index
            .operations_for_page(&log, PageKey::new(5, [-1, 0, 0]), 0)
            .into_iter()
            .map(|operation| operation.sequence)
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![1, 2, 3]);
    }
}
