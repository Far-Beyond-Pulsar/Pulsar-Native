use crate::{ContentHash, EditLog, PageId, PageKey, PlanetId, SparseBrickTree};
use thiserror::Error;

const SNAPSHOT_MAGIC: &[u8; 8] = b"PTSNAP01";
const PAGE_RECORD_BYTES: usize = 72;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactedPageRecord {
    pub key: PageKey,
    pub page_id: PageId,
    pub compacted_through_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainSnapshot {
    pub planet_id: PlanetId,
    pub generator_hash: ContentHash,
    pub hierarchy: SparseBrickTree,
    pub edit_tail: EditLog,
    pub compacted_pages: Vec<CompactedPageRecord>,
}

impl TerrainSnapshot {
    pub fn encode(&self) -> Result<Vec<u8>, SnapshotCodecError> {
        let mut compacted_pages = self.compacted_pages.clone();
        compacted_pages.sort_unstable_by_key(|record| record.key);
        if compacted_pages
            .windows(2)
            .any(|pair| pair[0].key == pair[1].key)
        {
            return Err(SnapshotCodecError::DuplicatePageKey);
        }
        let hierarchy = self.hierarchy.encode();
        let edits = self.edit_tail.encode();
        let mut output = Vec::with_capacity(
            80 + hierarchy.len() + edits.len() + compacted_pages.len() * PAGE_RECORD_BYTES,
        );
        output.extend_from_slice(SNAPSHOT_MAGIC);
        output.extend_from_slice(&self.planet_id.0);
        output.extend_from_slice(&self.generator_hash.0);
        output.extend_from_slice(&(hierarchy.len() as u64).to_le_bytes());
        output.extend_from_slice(&(edits.len() as u64).to_le_bytes());
        output.extend_from_slice(&(compacted_pages.len() as u32).to_le_bytes());
        output.extend_from_slice(&[0; 4]);
        output.extend_from_slice(&hierarchy);
        output.extend_from_slice(&edits);
        for record in compacted_pages {
            output.push(record.key.lod);
            output.extend_from_slice(&[0; 7]);
            for axis in record.key.page_xyz {
                output.extend_from_slice(&axis.to_le_bytes());
            }
            output.extend_from_slice(&record.page_id.0);
            output.extend_from_slice(&record.compacted_through_sequence.to_le_bytes());
        }
        Ok(output)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotCodecError> {
        if bytes.len() < 80
            || bytes.get(..8) != Some(SNAPSHOT_MAGIC)
            || bytes.get(76..80) != Some(&[0; 4])
        {
            return Err(SnapshotCodecError::Codec);
        }
        let planet_id = PlanetId(read_array(bytes, 8)?);
        let generator_hash = ContentHash(read_array(bytes, 24)?);
        let hierarchy_len = read_u64(bytes, 56)? as usize;
        let edits_len = read_u64(bytes, 64)? as usize;
        let page_count = read_u32(bytes, 72)? as usize;
        let hierarchy_end = 80_usize
            .checked_add(hierarchy_len)
            .ok_or(SnapshotCodecError::Codec)?;
        let edits_end = hierarchy_end
            .checked_add(edits_len)
            .ok_or(SnapshotCodecError::Codec)?;
        let expected_end = edits_end
            .checked_add(
                page_count
                    .checked_mul(PAGE_RECORD_BYTES)
                    .ok_or(SnapshotCodecError::Codec)?,
            )
            .ok_or(SnapshotCodecError::Codec)?;
        if expected_end != bytes.len() {
            return Err(SnapshotCodecError::Codec);
        }
        let hierarchy = SparseBrickTree::decode(&bytes[80..hierarchy_end])
            .map_err(|_| SnapshotCodecError::Codec)?;
        let edit_tail = EditLog::decode(&bytes[hierarchy_end..edits_end])
            .map_err(|_| SnapshotCodecError::Codec)?;
        let mut compacted_pages = Vec::with_capacity(page_count);
        let mut cursor = edits_end;
        for _ in 0..page_count {
            let lod = bytes[cursor];
            if bytes.get(cursor + 1..cursor + 8) != Some(&[0; 7]) {
                return Err(SnapshotCodecError::Codec);
            }
            let page_xyz = [
                read_i64(bytes, cursor + 8)?,
                read_i64(bytes, cursor + 16)?,
                read_i64(bytes, cursor + 24)?,
            ];
            compacted_pages.push(CompactedPageRecord {
                key: PageKey::new(lod, page_xyz),
                page_id: ContentHash(read_array(bytes, cursor + 32)?),
                compacted_through_sequence: read_u64(bytes, cursor + 64)?,
            });
            cursor += PAGE_RECORD_BYTES;
        }
        if compacted_pages
            .windows(2)
            .any(|pair| pair[0].key >= pair[1].key)
        {
            return Err(SnapshotCodecError::DuplicatePageKey);
        }
        Ok(Self {
            planet_id,
            generator_hash,
            hierarchy,
            edit_tail,
            compacted_pages,
        })
    }

    pub fn content_hash(&self) -> Result<ContentHash, SnapshotCodecError> {
        Ok(ContentHash::of(&self.encode()?))
    }
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], SnapshotCodecError> {
    bytes
        .get(offset..offset + N)
        .ok_or(SnapshotCodecError::Codec)?
        .try_into()
        .map_err(|_| SnapshotCodecError::Codec)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, SnapshotCodecError> {
    Ok(u32::from_le_bytes(read_array(bytes, offset)?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, SnapshotCodecError> {
    Ok(u64::from_le_bytes(read_array(bytes, offset)?))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, SnapshotCodecError> {
    Ok(i64::from_le_bytes(read_array(bytes, offset)?))
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnapshotCodecError {
    #[error("invalid canonical terrain snapshot encoding")]
    Codec,
    #[error("terrain snapshot contains duplicate or unsorted page keys")]
    DuplicatePageKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EditMode, EditOp, EditShape, NodeState};

    #[test]
    fn snapshot_round_trip_is_canonical_across_page_insertion_order() {
        let generator_hash = ContentHash::of(b"generator");
        let hierarchy =
            SparseBrickTree::centered(16, NodeState::Procedural(generator_hash)).unwrap();
        let mut edits = EditLog::default();
        edits
            .push(EditOp {
                sequence: 3,
                stable_id: [3; 16],
                shape: EditShape::Sphere {
                    center_cell: [1, 2, 3],
                    radius_cells: 5,
                },
                mode: EditMode::Subtract,
                material: 0,
            })
            .unwrap();
        let pages = vec![
            CompactedPageRecord {
                key: PageKey::new(0, [4, -1, 2]),
                page_id: ContentHash::of(b"b"),
                compacted_through_sequence: 3,
            },
            CompactedPageRecord {
                key: PageKey::new(0, [-2, 7, 1]),
                page_id: ContentHash::of(b"a"),
                compacted_through_sequence: 3,
            },
        ];
        let snapshot = TerrainSnapshot {
            planet_id: PlanetId([7; 16]),
            generator_hash,
            hierarchy,
            edit_tail: edits,
            compacted_pages: pages,
        };
        let decoded = TerrainSnapshot::decode(&snapshot.encode().unwrap()).unwrap();
        assert_eq!(
            decoded.content_hash().unwrap(),
            snapshot.content_hash().unwrap()
        );
        assert_eq!(decoded.compacted_pages[0].key.page_xyz, [-2, 7, 1]);
    }
}
