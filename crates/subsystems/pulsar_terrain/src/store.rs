use crate::{ContentHash, PageCodecError, PageId, VoxelPage};
use engine_fs::virtual_fs;
use std::path::PathBuf;
use thiserror::Error;

const MANIFEST_MAGIC: &[u8; 8] = b"PTRNRT01";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotRecord {
    pub generation: u64,
    pub hash: ContentHash,
    pub bytes: Vec<u8>,
}

/// Content-addressed terrain snapshots with atomic unique-name publication.
/// A pending file is never considered a root; the preceding generation remains
/// loadable until the provider's final rename succeeds.
#[derive(Clone, Debug)]
pub struct TerrainStore {
    root: PathBuf,
}

impl TerrainStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn save(&self, snapshot: &[u8]) -> Result<SnapshotRecord, TerrainStoreError> {
        let generation = self.latest_generation()?.unwrap_or(0).saturating_add(1);
        let hash = self.store_object(snapshot)?;
        virtual_fs::create_dir_all(&self.roots_dir()).map_err(TerrainStoreError::Io)?;

        let manifest = encode_manifest(generation, hash);
        let pending = self.roots_dir().join(format!("{generation:020}.pending"));
        let published = self
            .roots_dir()
            .join(format!("{generation:020}-{}.root", hash.to_hex()));
        virtual_fs::write_file(&pending, &manifest).map_err(TerrainStoreError::Io)?;
        virtual_fs::rename(&pending, &published).map_err(TerrainStoreError::Io)?;

        Ok(SnapshotRecord {
            generation,
            hash,
            bytes: snapshot.to_vec(),
        })
    }

    pub fn store_page(&self, page: &VoxelPage) -> Result<PageId, TerrainStoreError> {
        self.store_object(&page.encode())
    }

    pub fn load_page(&self, page_id: PageId) -> Result<VoxelPage, TerrainStoreError> {
        let bytes = virtual_fs::read_file(&self.objects_dir().join(page_id.to_hex()))
            .map_err(TerrainStoreError::Io)?;
        if ContentHash::of(&bytes) != page_id {
            return Err(TerrainStoreError::ObjectHash);
        }
        VoxelPage::decode(&bytes).map_err(TerrainStoreError::Page)
    }

    pub fn load_latest(&self) -> Result<Option<SnapshotRecord>, TerrainStoreError> {
        if !virtual_fs::exists(&self.roots_dir()).map_err(TerrainStoreError::Io)? {
            return Ok(None);
        }
        let mut candidates = virtual_fs::list_dir(&self.roots_dir())
            .map_err(TerrainStoreError::Io)?
            .into_iter()
            .filter(|entry| !entry.is_dir && entry.name.ends_with(".root"))
            .filter_map(|entry| {
                parse_generation(&entry.name).map(|generation| (generation, entry.name))
            })
            .collect::<Vec<_>>();
        candidates.sort_unstable_by_key(|candidate| std::cmp::Reverse(candidate.0));

        for (filename_generation, filename) in candidates {
            let manifest = match virtual_fs::read_file(&self.roots_dir().join(filename)) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let Ok((generation, hash)) = decode_manifest(&manifest) else {
                continue;
            };
            if generation != filename_generation {
                continue;
            }
            let Ok(bytes) = virtual_fs::read_file(&self.objects_dir().join(hash.to_hex())) else {
                continue;
            };
            if ContentHash::of(&bytes) != hash {
                continue;
            }
            return Ok(Some(SnapshotRecord {
                generation,
                hash,
                bytes,
            }));
        }
        Ok(None)
    }

    fn latest_generation(&self) -> Result<Option<u64>, TerrainStoreError> {
        Ok(self.load_latest()?.map(|record| record.generation))
    }

    fn store_object(&self, bytes: &[u8]) -> Result<ContentHash, TerrainStoreError> {
        let hash = ContentHash::of(bytes);
        virtual_fs::create_dir_all(&self.objects_dir()).map_err(TerrainStoreError::Io)?;
        let object_path = self.objects_dir().join(hash.to_hex());
        if !virtual_fs::exists(&object_path).map_err(TerrainStoreError::Io)? {
            let pending = self
                .objects_dir()
                .join(format!("{}.pending", hash.to_hex()));
            virtual_fs::write_file(&pending, bytes).map_err(TerrainStoreError::Io)?;
            virtual_fs::rename(&pending, &object_path).map_err(TerrainStoreError::Io)?;
        }
        Ok(hash)
    }

    fn objects_dir(&self) -> PathBuf {
        self.root.join("objects")
    }

    fn roots_dir(&self) -> PathBuf {
        self.root.join("roots")
    }

    #[cfg(test)]
    fn pending_root_path(&self, generation: u64) -> PathBuf {
        self.roots_dir().join(format!("{generation:020}.pending"))
    }
}

fn encode_manifest(generation: u64, hash: ContentHash) -> Vec<u8> {
    let mut output = Vec::with_capacity(48);
    output.extend_from_slice(MANIFEST_MAGIC);
    output.extend_from_slice(&generation.to_le_bytes());
    output.extend_from_slice(&hash.0);
    output
}

fn decode_manifest(bytes: &[u8]) -> Result<(u64, ContentHash), TerrainStoreError> {
    if bytes.len() != 48 || bytes.get(..8) != Some(MANIFEST_MAGIC) {
        return Err(TerrainStoreError::Manifest);
    }
    let generation = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let hash = ContentHash(bytes[16..48].try_into().unwrap());
    Ok((generation, hash))
}

fn parse_generation(filename: &str) -> Option<u64> {
    filename.get(..20)?.parse().ok()
}

#[derive(Debug, Error)]
pub enum TerrainStoreError {
    #[error("terrain store I/O failed: {0}")]
    Io(#[source] anyhow::Error),
    #[error("invalid terrain root manifest")]
    Manifest,
    #[error("content-addressed terrain object hash mismatch")]
    ObjectHash,
    #[error("stored terrain page is invalid: {0}")]
    Page(#[source] PageCodecError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_publish_keeps_the_previous_valid_root() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let first = store.save(b"first canonical snapshot").unwrap();

        virtual_fs::write_file(&store.pending_root_path(2), b"partial manifest").unwrap();
        let recovered = store.load_latest().unwrap().unwrap();
        assert_eq!(recovered, first);

        virtual_fs::write_file(
            &store.roots_dir().join(format!(
                "{:020}-{}.root",
                2,
                ContentHash::default().to_hex()
            )),
            b"published but incomplete manifest",
        )
        .unwrap();
        assert_eq!(store.load_latest().unwrap().unwrap(), first);

        let second = store.save(b"second canonical snapshot").unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(store.load_latest().unwrap().unwrap(), second);
    }

    #[test]
    fn pages_are_content_addressed_and_validated_on_load() {
        virtual_fs::reset_to_local();
        let temporary = tempfile::tempdir().unwrap();
        let store = TerrainStore::new(temporary.path().join("terrain"));
        let page = VoxelPage::constant(crate::CellWord::new(-1, 7, 0));
        let page_id = store.store_page(&page).unwrap();
        assert_eq!(page_id, page.page_id());
        assert_eq!(store.load_page(page_id).unwrap(), page);

        virtual_fs::write_file(&store.objects_dir().join(page_id.to_hex()), b"corrupt").unwrap();
        assert!(matches!(
            store.load_page(page_id),
            Err(TerrainStoreError::ObjectHash)
        ));
    }
}
