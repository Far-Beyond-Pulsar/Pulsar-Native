use crate::{CellWord, ContentHash, DeterministicGenerator, EditLog, EditOp, PageId, PageKey};
use thiserror::Error;

pub const PAGE_EDGE: usize = 32;
pub const MICROBRICK_EDGE: usize = 8;
pub const MICROBRICKS_PER_AXIS: usize = PAGE_EDGE / MICROBRICK_EDGE;
pub const MICROBRICK_COUNT: usize =
    MICROBRICKS_PER_AXIS * MICROBRICKS_PER_AXIS * MICROBRICKS_PER_AXIS;
pub const CELL_COUNT: usize = PAGE_EDGE * PAGE_EDGE * PAGE_EDGE;
const MAGIC: &[u8; 8] = b"PTRNPG01";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxelPage {
    storage: PageStorage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PageStorage {
    Constant(CellWord),
    Dense(Box<[CellWord]>),
}

impl VoxelPage {
    pub fn microbrick_address(local_cell: [usize; 3]) -> Option<(usize, [usize; 3])> {
        if local_cell.iter().any(|axis| *axis >= PAGE_EDGE) {
            return None;
        }
        let brick = local_cell.map(|axis| axis / MICROBRICK_EDGE);
        let within = local_cell.map(|axis| axis % MICROBRICK_EDGE);
        let index = brick[0] + MICROBRICKS_PER_AXIS * (brick[1] + MICROBRICKS_PER_AXIS * brick[2]);
        Some((index, within))
    }

    pub fn constant(cell: CellWord) -> Self {
        Self {
            storage: PageStorage::Constant(cell),
        }
    }

    pub fn from_cells(cells: Vec<CellWord>) -> Result<Self, PageCodecError> {
        if cells.len() != CELL_COUNT {
            return Err(PageCodecError::CellCount(cells.len()));
        }
        let first = cells[0];
        if cells.iter().all(|cell| *cell == first) {
            Ok(Self::constant(first))
        } else {
            Ok(Self {
                storage: PageStorage::Dense(cells.into_boxed_slice()),
            })
        }
    }

    pub fn generate(
        key: PageKey,
        generator: &dyn DeterministicGenerator,
        edits: &EditLog,
    ) -> Result<Self, PageCodecError> {
        Self::generate_with_operations(key, generator, edits.operations())
    }

    pub(crate) fn generate_with_operations(
        key: PageKey,
        generator: &dyn DeterministicGenerator,
        operations: &[EditOp],
    ) -> Result<Self, PageCodecError> {
        if key.lod != 0 {
            return Err(PageCodecError::UnsupportedLod(key.lod));
        }
        let origin = key
            .lod0_cell_min()
            .ok_or(PageCodecError::CoordinateOverflow)?;
        let mut cells = Vec::with_capacity(CELL_COUNT);
        for z in 0..PAGE_EDGE as i64 {
            for y in 0..PAGE_EDGE as i64 {
                for x in 0..PAGE_EDGE as i64 {
                    let coordinate = [origin[0] + x, origin[1] + y, origin[2] + z];
                    let mut cell = generator.sample_cell(coordinate);
                    for operation in operations {
                        let (min, max) = operation.shape.bounds();
                        if (0..3).all(|axis| {
                            coordinate[axis] >= min[axis] && coordinate[axis] < max[axis]
                        }) {
                            cell = operation.apply(coordinate, cell);
                        }
                    }
                    cells.push(cell);
                }
            }
        }
        Self::from_cells(cells)
    }

    /// Fold only the supplied ordered edit tail into an already materialized
    /// LOD0 page. The procedural source and older edit prefix are not replayed.
    pub fn apply_edit_tail(
        &self,
        key: PageKey,
        operations: &[EditOp],
    ) -> Result<Self, PageCodecError> {
        if key.lod != 0 {
            return Err(PageCodecError::UnsupportedLod(key.lod));
        }
        let origin = key
            .lod0_cell_min()
            .ok_or(PageCodecError::CoordinateOverflow)?;
        let mut cells = Vec::with_capacity(CELL_COUNT);
        for z in 0..PAGE_EDGE as i64 {
            for y in 0..PAGE_EDGE as i64 {
                for x in 0..PAGE_EDGE as i64 {
                    let coordinate = [origin[0] + x, origin[1] + y, origin[2] + z];
                    let mut cell = self.get([x as usize, y as usize, z as usize]).unwrap();
                    for operation in operations {
                        let (min, max) = operation.shape.bounds();
                        if (0..3).all(|axis| {
                            coordinate[axis] >= min[axis] && coordinate[axis] < max[axis]
                        }) {
                            cell = operation.apply(coordinate, cell);
                        }
                    }
                    cells.push(cell);
                }
            }
        }
        Self::from_cells(cells)
    }

    /// Conservatively reduce eight LOD N children into one LOD N+1 page.
    /// Any solid fine sample keeps the coarse cell solid, preventing thin
    /// features from disappearing solely because of reduction.
    pub fn reduce_children(children: [&Self; 8]) -> Self {
        let mut cells = Vec::with_capacity(CELL_COUNT);
        for z in 0..PAGE_EDGE {
            for y in 0..PAGE_EDGE {
                for x in 0..PAGE_EDGE {
                    let mut samples = [CellWord::AIR; 8];
                    let mut sample_index = 0;
                    for dz in 0..2 {
                        for dy in 0..2 {
                            for dx in 0..2 {
                                let fine = [x * 2 + dx, y * 2 + dy, z * 2 + dz];
                                let child_xyz = fine.map(|axis| axis / PAGE_EDGE);
                                let local = fine.map(|axis| axis % PAGE_EDGE);
                                let child_index =
                                    child_xyz[0] | (child_xyz[1] << 1) | (child_xyz[2] << 2);
                                samples[sample_index] = children[child_index].get(local).unwrap();
                                sample_index += 1;
                            }
                        }
                    }
                    let selected = samples
                        .iter()
                        .filter(|sample| sample.is_solid())
                        .min_by_key(|sample| sample.density())
                        .or_else(|| samples.iter().min_by_key(|sample| sample.density()))
                        .copied()
                        .unwrap();
                    let flags = samples
                        .iter()
                        .fold(0, |flags, sample| flags | sample.flags());
                    cells.push(CellWord::new(
                        selected.density(),
                        selected.material(),
                        flags,
                    ));
                }
            }
        }
        Self::from_cells(cells).expect("LOD reduction always produces exactly one page")
    }

    pub fn cells(&self) -> impl ExactSizeIterator<Item = CellWord> + '_ {
        (0..CELL_COUNT).map(|index| self.cell_at_index(index))
    }

    pub fn constant_cell(&self) -> Option<CellWord> {
        match self.storage {
            PageStorage::Constant(cell) => Some(cell),
            PageStorage::Dense(_) => None,
        }
    }

    /// Heap bytes used by canonical cell storage, excluding the small page
    /// object itself. Uniform pages deliberately report zero.
    pub fn dense_allocation_bytes(&self) -> usize {
        match &self.storage {
            PageStorage::Constant(_) => 0,
            PageStorage::Dense(cells) => cells.len() * std::mem::size_of::<CellWord>(),
        }
    }

    pub fn get(&self, xyz: [usize; 3]) -> Option<CellWord> {
        if xyz.iter().any(|axis| *axis >= PAGE_EDGE) {
            return None;
        }
        Some(self.cell_at_index(xyz[0] + PAGE_EDGE * (xyz[1] + PAGE_EDGE * xyz[2])))
    }

    /// Stable little-endian RLE format. Halos are deliberately excluded.
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::with_capacity(16 + CELL_COUNT * 8);
        output.extend_from_slice(MAGIC);
        output.extend_from_slice(&(CELL_COUNT as u32).to_le_bytes());
        let mut index = 0;
        while index < CELL_COUNT {
            let value = self.cell_at_index(index);
            let mut run = 1_usize;
            while index + run < CELL_COUNT
                && self.cell_at_index(index + run) == value
                && run < u32::MAX as usize
            {
                run += 1;
            }
            output.extend_from_slice(&(run as u32).to_le_bytes());
            output.extend_from_slice(&value.0.to_le_bytes());
            index += run;
        }
        output
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PageCodecError> {
        if bytes.get(..8) != Some(MAGIC) {
            return Err(PageCodecError::Magic);
        }
        let count = read_u32(bytes, 8)? as usize;
        if count != CELL_COUNT {
            return Err(PageCodecError::CellCount(count));
        }
        let mut cells = Vec::with_capacity(CELL_COUNT);
        let mut cursor = 12;
        let mut previous = None;
        while cursor < bytes.len() && cells.len() < CELL_COUNT {
            let run = read_u32(bytes, cursor)? as usize;
            let value = CellWord(read_u32(bytes, cursor + 4)?);
            cursor += 8;
            if run == 0 || cells.len().saturating_add(run) > CELL_COUNT {
                return Err(PageCodecError::RunLength);
            }
            if previous == Some(value) {
                return Err(PageCodecError::NonCanonical);
            }
            cells.resize(cells.len() + run, value);
            previous = Some(value);
        }
        if cells.len() != CELL_COUNT || cursor != bytes.len() {
            return Err(PageCodecError::TruncatedOrTrailing);
        }
        Self::from_cells(cells)
    }

    pub fn page_id(&self) -> PageId {
        ContentHash::of(&self.encode())
    }

    fn cell_at_index(&self, index: usize) -> CellWord {
        match &self.storage {
            PageStorage::Constant(cell) => *cell,
            PageStorage::Dense(cells) => cells[index],
        }
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PageCodecError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(PageCodecError::TruncatedOrTrailing)?;
    Ok(u32::from_le_bytes(value.try_into().unwrap()))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PageCodecError {
    #[error("invalid terrain page magic/version")]
    Magic,
    #[error("terrain page has {0} cells instead of 32768")]
    CellCount(usize),
    #[error("invalid terrain page run length")]
    RunLength,
    #[error("terrain page is truncated or contains trailing bytes")]
    TruncatedOrTrailing,
    #[error("terrain page uses a non-canonical run encoding")]
    NonCanonical,
    #[error("materialized pages currently require LOD0, received LOD{0}")]
    UnsupportedLod(u8),
    #[error("terrain page coordinate overflow")]
    CoordinateOverflow,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditMode, EditShape, FixedSphereGenerator};

    #[test]
    fn constant_and_dense_pages_round_trip_with_stable_hashes() {
        let constant = VoxelPage::constant(CellWord::new(-4, 3, 1));
        assert_eq!(VoxelPage::decode(&constant.encode()).unwrap(), constant);
        assert_eq!(constant.encode().len(), 20);
        assert_eq!(constant.dense_allocation_bytes(), 0);

        let cells = (0..CELL_COUNT)
            .map(|index| CellWord::new(index as i16, (index % 251) as u8, 0))
            .collect();
        let dense = VoxelPage::from_cells(cells).unwrap();
        assert_eq!(VoxelPage::decode(&dense.encode()).unwrap(), dense);
        assert_eq!(dense.page_id(), ContentHash::of(&dense.encode()));
        assert_eq!(
            dense.dense_allocation_bytes(),
            CELL_COUNT * std::mem::size_of::<CellWord>()
        );
    }

    #[test]
    fn codec_rejects_corruption_versions_and_noncanonical_runs() {
        let page = VoxelPage::constant(CellWord::AIR);
        let mut wrong_version = page.encode();
        wrong_version[7] = b'2';
        assert_eq!(
            VoxelPage::decode(&wrong_version),
            Err(PageCodecError::Magic)
        );

        let mut adjacent_runs = Vec::new();
        adjacent_runs.extend_from_slice(MAGIC);
        adjacent_runs.extend_from_slice(&(CELL_COUNT as u32).to_le_bytes());
        adjacent_runs.extend_from_slice(&1_u32.to_le_bytes());
        adjacent_runs.extend_from_slice(&CellWord::AIR.0.to_le_bytes());
        adjacent_runs.extend_from_slice(&((CELL_COUNT - 1) as u32).to_le_bytes());
        adjacent_runs.extend_from_slice(&CellWord::AIR.0.to_le_bytes());
        assert_eq!(
            VoxelPage::decode(&adjacent_runs),
            Err(PageCodecError::NonCanonical)
        );

        let mut corrupt = page.encode();
        corrupt.truncate(corrupt.len() - 1);
        assert!(VoxelPage::decode(&corrupt).is_err());
    }

    #[test]
    fn lod_reduction_preserves_a_single_thin_solid_sample() {
        let air = VoxelPage::constant(CellWord::AIR);
        let mut cells = vec![CellWord::AIR; CELL_COUNT];
        cells[0] = CellWord::new(-1, 12, 4);
        let feature = VoxelPage::from_cells(cells).unwrap();
        let reduced =
            VoxelPage::reduce_children([&feature, &air, &air, &air, &air, &air, &air, &air]);
        let first = reduced.get([0, 0, 0]).unwrap();
        assert!(first.is_solid());
        assert_eq!(first.material(), 12);
        assert_eq!(first.flags(), 4);
    }

    #[test]
    fn microbrick_addressing_covers_every_page_cell_exactly() {
        let mut hits = vec![0_u8; CELL_COUNT];
        for z in 0..PAGE_EDGE {
            for y in 0..PAGE_EDGE {
                for x in 0..PAGE_EDGE {
                    let (brick, local) = VoxelPage::microbrick_address([x, y, z]).unwrap();
                    assert!(brick < MICROBRICK_COUNT);
                    assert!(local.iter().all(|axis| *axis < MICROBRICK_EDGE));
                    let reconstructed_brick = [
                        brick % MICROBRICKS_PER_AXIS,
                        (brick / MICROBRICKS_PER_AXIS) % MICROBRICKS_PER_AXIS,
                        brick / (MICROBRICKS_PER_AXIS * MICROBRICKS_PER_AXIS),
                    ];
                    let reconstructed = [
                        reconstructed_brick[0] * MICROBRICK_EDGE + local[0],
                        reconstructed_brick[1] * MICROBRICK_EDGE + local[1],
                        reconstructed_brick[2] * MICROBRICK_EDGE + local[2],
                    ];
                    assert_eq!(reconstructed, [x, y, z]);
                    let linear = x + PAGE_EDGE * (y + PAGE_EDGE * z);
                    hits[linear] += 1;
                }
            }
        }
        assert!(hits.into_iter().all(|count| count == 1));
        assert_eq!(VoxelPage::microbrick_address([PAGE_EDGE, 0, 0]), None);
    }

    #[test]
    fn incremental_edit_tail_matches_full_deterministic_replay() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 24,
            material: 2,
        };
        let first = EditOp {
            sequence: 1,
            stable_id: [1; 16],
            shape: EditShape::Sphere {
                center_cell: [8, 8, 8],
                radius_cells: 5,
            },
            mode: EditMode::Subtract,
            material: 0,
        };
        let second = EditOp {
            sequence: 2,
            stable_id: [2; 16],
            shape: EditShape::Sphere {
                center_cell: [12, 8, 8],
                radius_cells: 3,
            },
            mode: EditMode::Union,
            material: 9,
        };
        let key = PageKey::new(0, [0; 3]);
        let prefix = EditLog::from_scheduled(vec![first]).unwrap();
        let complete = EditLog::from_scheduled(vec![second, first]).unwrap();
        let prefix_page = VoxelPage::generate(key, &generator, &prefix).unwrap();
        let incremental = prefix_page.apply_edit_tail(key, &[second]).unwrap();
        let replayed = VoxelPage::generate(key, &generator, &complete).unwrap();
        assert_eq!(incremental, replayed);
        assert_eq!(incremental.page_id(), replayed.page_id());
    }
}
