use crate::{CellWord, ContentHash, MaterialId, PageKey};
use std::collections::BTreeMap;
use thiserror::Error;

const EDIT_LOG_MAGIC: &[u8; 8] = b"PTEDIT01";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    Union,
    Subtract,
    Replace,
    Paint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditShape {
    Sphere {
        center_cell: [i64; 3],
        radius_cells: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditOp {
    pub sequence: u64,
    pub stable_id: [u8; 16],
    pub shape: EditShape,
    pub mode: EditMode,
    pub material: MaterialId,
}

impl EditOp {
    pub fn apply(self, cell_xyz: [i64; 3], cell: CellWord) -> CellWord {
        let shape_distance = self.shape.signed_distance(cell_xyz);
        if shape_distance > 0 && matches!(self.mode, EditMode::Replace | EditMode::Paint) {
            return cell;
        }
        let old_distance = i32::from(cell.density());
        let density = match self.mode {
            EditMode::Union => old_distance.min(shape_distance),
            EditMode::Subtract => old_distance.max(-shape_distance),
            EditMode::Replace => shape_distance,
            EditMode::Paint => old_distance,
        }
        .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;

        let affects_material = shape_distance <= 0
            && matches!(
                self.mode,
                EditMode::Union | EditMode::Replace | EditMode::Paint
            );
        let material = if affects_material && density <= 0 {
            self.material
        } else if density > 0 {
            0
        } else {
            cell.material()
        };
        CellWord::new(density, material, cell.flags())
    }
}

impl EditShape {
    pub fn bounds(self) -> ([i64; 3], [i64; 3]) {
        match self {
            Self::Sphere {
                center_cell,
                radius_cells,
            } => {
                let radius = i64::from(radius_cells);
                (
                    center_cell.map(|axis| axis.saturating_sub(radius)),
                    center_cell.map(|axis| axis.saturating_add(radius).saturating_add(1)),
                )
            }
        }
    }

    /// Conservative count of LOD0 pages intersected by the shape's integer
    /// cell AABB. This is used for scheduling and edit-amplification budgets;
    /// it does not materialize or visit any page.
    pub fn affected_lod0_page_count(self) -> u128 {
        let (min, max) = self.bounds();
        (0..3)
            .map(|axis| {
                let first = min[axis].div_euclid(crate::PAGE_EDGE_CELLS);
                let last = max[axis]
                    .saturating_sub(1)
                    .div_euclid(crate::PAGE_EDGE_CELLS);
                u128::try_from(last.saturating_sub(first).saturating_add(1)).unwrap_or(u128::MAX)
            })
            .fold(1_u128, u128::saturating_mul)
    }

    /// Smallest canonical hierarchy region that contains the edit AABB.
    ///
    /// Edits that cross the centered root split (or extend beyond the root)
    /// attach to the root. This keeps insertion bounded without enumerating
    /// intersected pages; exact shape bounds are still checked during replay.
    fn covering_attachment(self, root_lod: u8) -> EditAttachment {
        let (min_cell, max_cell) = self.bounds();
        let min_page = min_cell.map(|axis| axis.div_euclid(crate::PAGE_EDGE_CELLS));
        let max_page =
            max_cell.map(|axis| axis.saturating_sub(1).div_euclid(crate::PAGE_EDGE_CELLS));
        let half = 1_i64 << (root_lod - 1);
        let root_min = -half;
        let root_max = half - 1;
        if (0..3).any(|axis| min_page[axis] < root_min || max_page[axis] > root_max) {
            return EditAttachment::Root;
        }

        let relative_min = min_page.map(|axis| (axis - root_min) as u64);
        let relative_max = max_page.map(|axis| (axis - root_min) as u64);
        let differing = (relative_min[0] ^ relative_max[0])
            | (relative_min[1] ^ relative_max[1])
            | (relative_min[2] ^ relative_max[2]);
        if differing == 0 {
            return EditAttachment::Region(PageKey::new(0, min_page));
        }

        let lod = (u64::BITS - differing.leading_zeros()) as u8;
        if lod >= root_lod {
            EditAttachment::Root
        } else {
            let scale = 1_i64 << lod;
            EditAttachment::Region(PageKey::new(
                lod,
                min_page.map(|axis| axis.div_euclid(scale)),
            ))
        }
    }

    fn signed_distance(self, cell_xyz: [i64; 3]) -> i32 {
        match self {
            Self::Sphere {
                center_cell,
                radius_cells,
            } => {
                let delta = [
                    i128::from(cell_xyz[0]) - i128::from(center_cell[0]),
                    i128::from(cell_xyz[1]) - i128::from(center_cell[1]),
                    i128::from(cell_xyz[2]) - i128::from(center_cell[2]),
                ];
                let squared = delta
                    .iter()
                    .map(|axis| axis.saturating_mul(*axis) as u128)
                    .sum::<u128>();
                let distance = integer_sqrt(squared).min(i32::MAX as u128) as i32;
                distance.saturating_sub(radius_cells.min(i32::MAX as u32) as i32)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditAttachment {
    Root,
    Region(PageKey),
}

/// Derived sparse spatial index for the canonical edit log.
///
/// Every edit contributes exactly one reference, either at the root or at its
/// smallest covering hierarchy region. Page replay walks only the page's
/// ancestor chain, so work is proportional to relevant candidates rather than
/// the total logical volume or the total global edit count.
#[derive(Clone, Debug)]
pub(crate) struct EditIndex {
    root_lod: u8,
    root: Vec<usize>,
    regions: BTreeMap<PageKey, Vec<usize>>,
}

impl EditIndex {
    pub(crate) fn new(root_lod: u8) -> Self {
        Self {
            root_lod,
            root: Vec::new(),
            regions: BTreeMap::new(),
        }
    }

    pub(crate) fn insert(&mut self, operation_index: usize, operation: EditOp) {
        match operation.shape.covering_attachment(self.root_lod) {
            EditAttachment::Root => self.root.push(operation_index),
            EditAttachment::Region(key) => {
                self.regions.entry(key).or_default().push(operation_index);
            }
        }
    }

    pub(crate) fn operations_for_page(
        &self,
        log: &EditLog,
        key: PageKey,
        after_sequence: u64,
    ) -> Vec<EditOp> {
        let mut indices = self.root.clone();
        let mut ancestor = Some(key);
        while let Some(key) = ancestor.filter(|key| key.lod < self.root_lod) {
            if let Some(attached) = self.regions.get(&key) {
                indices.extend_from_slice(attached);
            }
            ancestor = key.parent();
        }
        indices.sort_unstable();
        indices
            .into_iter()
            .filter_map(|index| log.operations().get(index).copied())
            .filter(|operation| operation.sequence > after_sequence)
            .collect()
    }

    pub(crate) fn region_count(&self) -> usize {
        self.regions.len() + usize::from(!self.root.is_empty())
    }

    pub(crate) fn reference_count(&self) -> usize {
        self.root.len() + self.regions.values().map(Vec::len).sum::<usize>()
    }
}

fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut low = 1_u128;
    let mut high = 1_u128 << (128 - value.leading_zeros()).div_ceil(2);
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if middle <= value / middle {
            low = middle;
        } else {
            high = middle;
        }
    }
    low
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("edit sequence {received} must be greater than {latest}")]
    OutOfOrder { latest: u64, received: u64 },
    #[error("edit id was already applied at a different sequence")]
    DuplicateId,
    #[error("invalid canonical edit-log encoding")]
    Codec,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditLog {
    operations: Vec<EditOp>,
}

impl EditLog {
    /// Canonicalize operations delivered by independent workers or network
    /// scheduling. Authoritative sequence numbers, not arrival order, define
    /// replay order.
    pub fn from_scheduled(mut operations: Vec<EditOp>) -> Result<Self, EditError> {
        operations.sort_unstable_by_key(|operation| (operation.sequence, operation.stable_id));
        let mut log = Self::default();
        for operation in operations {
            log.push(operation)?;
        }
        Ok(log)
    }

    pub fn push(&mut self, operation: EditOp) -> Result<(), EditError> {
        if let Some(existing) = self
            .operations
            .iter()
            .find(|existing| existing.stable_id == operation.stable_id)
        {
            return if *existing == operation {
                Ok(())
            } else {
                Err(EditError::DuplicateId)
            };
        }
        if let Some(latest) = self.operations.last().map(|op| op.sequence) {
            if operation.sequence <= latest {
                return Err(EditError::OutOfOrder {
                    latest,
                    received: operation.sequence,
                });
            }
        }
        self.operations.push(operation);
        Ok(())
    }

    pub fn operations(&self) -> &[EditOp] {
        &self.operations
    }

    pub fn latest_sequence(&self) -> u64 {
        self.operations
            .last()
            .map_or(0, |operation| operation.sequence)
    }

    pub fn apply(&self, cell_xyz: [i64; 3], mut cell: CellWord) -> CellWord {
        for operation in &self.operations {
            let (min, max) = operation.shape.bounds();
            if (0..3).all(|axis| cell_xyz[axis] >= min[axis] && cell_xyz[axis] < max[axis]) {
                cell = operation.apply(cell_xyz, cell);
            }
        }
        cell
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(12 + self.operations.len() * 56);
        output.extend_from_slice(EDIT_LOG_MAGIC);
        output.extend_from_slice(&(self.operations.len() as u32).to_le_bytes());
        for operation in &self.operations {
            output.extend_from_slice(&operation.sequence.to_le_bytes());
            output.extend_from_slice(&operation.stable_id);
            output.push(match operation.mode {
                EditMode::Union => 0,
                EditMode::Subtract => 1,
                EditMode::Replace => 2,
                EditMode::Paint => 3,
            });
            output.push(operation.material);
            match operation.shape {
                EditShape::Sphere {
                    center_cell,
                    radius_cells,
                } => {
                    output.push(0);
                    output.push(0);
                    for axis in center_cell {
                        output.extend_from_slice(&axis.to_le_bytes());
                    }
                    output.extend_from_slice(&radius_cells.to_le_bytes());
                }
            }
        }
        output
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EditError> {
        if bytes.get(..8) != Some(EDIT_LOG_MAGIC) || bytes.len() < 12 {
            return Err(EditError::Codec);
        }
        let count = read_u32(bytes, 8)? as usize;
        let expected = 12_usize
            .checked_add(count.checked_mul(56).ok_or(EditError::Codec)?)
            .ok_or(EditError::Codec)?;
        if bytes.len() != expected {
            return Err(EditError::Codec);
        }
        let mut log = Self::default();
        let mut cursor = 12;
        for _ in 0..count {
            let sequence = read_u64(bytes, cursor)?;
            let stable_id = bytes
                .get(cursor + 8..cursor + 24)
                .ok_or(EditError::Codec)?
                .try_into()
                .map_err(|_| EditError::Codec)?;
            let mode = match *bytes.get(cursor + 24).ok_or(EditError::Codec)? {
                0 => EditMode::Union,
                1 => EditMode::Subtract,
                2 => EditMode::Replace,
                3 => EditMode::Paint,
                _ => return Err(EditError::Codec),
            };
            let material = *bytes.get(cursor + 25).ok_or(EditError::Codec)?;
            if bytes.get(cursor + 26..cursor + 28) != Some(&[0, 0]) {
                return Err(EditError::Codec);
            }
            let mut center_cell = [0_i64; 3];
            for (axis, value) in center_cell.iter_mut().enumerate() {
                *value = read_i64(bytes, cursor + 28 + axis * 8)?;
            }
            let radius_cells = read_u32(bytes, cursor + 52)?;
            log.push(EditOp {
                sequence,
                stable_id,
                shape: EditShape::Sphere {
                    center_cell,
                    radius_cells,
                },
                mode,
                material,
            })?;
            cursor += 56;
        }
        Ok(log)
    }

    pub fn content_hash(&self) -> ContentHash {
        ContentHash::of(&self.encode())
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, EditError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(EditError::Codec)?
            .try_into()
            .map_err(|_| EditError::Codec)?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, EditError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(EditError::Codec)?
            .try_into()
            .map_err(|_| EditError::Codec)?,
    ))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, EditError> {
    Ok(i64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(EditError::Codec)?
            .try_into()
            .map_err(|_| EditError::Codec)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_edit_log_round_trips_and_rejects_reordering() {
        let first = EditOp {
            sequence: 4,
            stable_id: [4; 16],
            shape: EditShape::Sphere {
                center_cell: [-9, 2, 17],
                radius_cells: 6,
            },
            mode: EditMode::Subtract,
            material: 0,
        };
        let second = EditOp {
            sequence: 9,
            stable_id: [9; 16],
            shape: EditShape::Sphere {
                center_cell: [3, -7, 1],
                radius_cells: 2,
            },
            mode: EditMode::Paint,
            material: 8,
        };
        let mut log = EditLog::default();
        log.push(first).unwrap();
        log.push(second).unwrap();
        let decoded = EditLog::decode(&log.encode()).unwrap();
        assert_eq!(decoded.operations(), log.operations());
        assert_eq!(decoded.content_hash(), log.content_hash());
        assert_eq!(
            log.push(EditOp {
                sequence: 2,
                stable_id: [2; 16],
                ..first
            }),
            Err(EditError::OutOfOrder {
                latest: 9,
                received: 2
            })
        );
    }

    #[test]
    fn worker_arrival_order_does_not_change_canonical_replay() {
        let operations = (1..=32)
            .map(|sequence| EditOp {
                sequence,
                stable_id: [sequence as u8; 16],
                shape: EditShape::Sphere {
                    center_cell: [sequence as i64 - 16, -(sequence as i64), 3],
                    radius_cells: (sequence % 7 + 1) as u32,
                },
                mode: if sequence % 2 == 0 {
                    EditMode::Union
                } else {
                    EditMode::Subtract
                },
                material: sequence as u8,
            })
            .collect::<Vec<_>>();
        let forward = EditLog::from_scheduled(operations.clone()).unwrap();
        let reverse = EditLog::from_scheduled(operations.into_iter().rev().collect()).unwrap();
        assert_eq!(forward, reverse);
        assert_eq!(forward.content_hash(), reverse.content_hash());
        for coordinate in [[-20, 0, 3], [0, -10, 3], [20, -30, 3]] {
            assert_eq!(
                forward.apply(coordinate, CellWord::AIR),
                reverse.apply(coordinate, CellWord::AIR)
            );
        }
    }

    #[test]
    fn edit_page_amplification_is_bounded_without_visiting_pages() {
        let one_cell = EditShape::Sphere {
            center_cell: [0; 3],
            radius_cells: 0,
        };
        assert_eq!(one_cell.affected_lod0_page_count(), 1);

        let across_negative_boundary = EditShape::Sphere {
            center_cell: [0; 3],
            radius_cells: 1,
        };
        assert_eq!(across_negative_boundary.affected_lod0_page_count(), 8);
    }

    #[test]
    fn edits_attach_once_and_page_queries_visit_only_ancestors() {
        let operations = [
            EditOp {
                sequence: 1,
                stable_id: [1; 16],
                shape: EditShape::Sphere {
                    center_cell: [3_208, 3_208, 3_208],
                    radius_cells: 1,
                },
                mode: EditMode::Subtract,
                material: 0,
            },
            EditOp {
                sequence: 2,
                stable_id: [2; 16],
                shape: EditShape::Sphere {
                    center_cell: [8, 8, 8],
                    radius_cells: 1,
                },
                mode: EditMode::Paint,
                material: 4,
            },
            EditOp {
                sequence: 3,
                stable_id: [3; 16],
                shape: EditShape::Sphere {
                    center_cell: [0; 3],
                    radius_cells: 1,
                },
                mode: EditMode::Union,
                material: 7,
            },
        ];
        let mut log = EditLog::default();
        let mut index = EditIndex::new(12);
        for operation in operations {
            let operation_index = log.operations().len();
            log.push(operation).unwrap();
            index.insert(operation_index, operation);
        }

        let local = index.operations_for_page(&log, PageKey::new(0, [0; 3]), 0);
        assert_eq!(
            local
                .iter()
                .map(|operation| operation.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        let far = index.operations_for_page(&log, PageKey::new(0, [100; 3]), 1);
        assert_eq!(
            far.iter()
                .map(|operation| operation.sequence)
                .collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(index.region_count(), 3);
        assert_eq!(index.reference_count(), operations.len());
    }
}
