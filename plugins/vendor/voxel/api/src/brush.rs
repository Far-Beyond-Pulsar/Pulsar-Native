//! Shape edits of a voxel world: the generic, persistent destruction and
//! construction journal every terrain generator understands.
//!
//! Brushes are ordered: a later brush wins where they overlap. Coordinates
//! are world metres relative to the terrain's origin, so the same journal
//! applies at every voxel size. Per-sample edits live in the payload chunks
//! instead (see `VoxelSampleEdit`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VoxelBrushShape {
    Sphere,
    /// An axis-aligned cube in the world's own cell axes.
    Cube,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VoxelBrushOp {
    /// Carve cells to air.
    Remove,
    /// Fill cells with `material`.
    Add,
    /// Recolour existing solid cells with `material`.
    Paint,
}

/// One journal entry.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct VoxelBrushEdit {
    pub center: [f64; 3],
    pub radius: f64,
    pub shape: VoxelBrushShape,
    pub op: VoxelBrushOp,
    /// Terrain material for `Add` and `Paint` (the generator's material
    /// table); ignored by `Remove`.
    #[serde(default)]
    pub material: u32,
    /// Cube only: half height along the vertical, metres; 0 is a cube of
    /// `radius`. A flat-topped box (flatten, smooth).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub height: f64,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl VoxelBrushEdit {
    /// Finite centre, positive finite radius.
    pub fn validate(&self) -> Result<(), String> {
        if !(self.height.is_finite() && self.height >= 0.0) {
            return Err("brush height must be finite and non-negative".into());
        }
        if !self.center.iter().all(|v| v.is_finite())
            || !(self.radius.is_finite() && self.radius > 0.0)
        {
            return Err("brush centre and radius must be finite, radius positive".into());
        }
        Ok(())
    }
}

/// Edits per shared chunk of a [`VoxelEditJournal`].
const JOURNAL_CHUNK: usize = 1024;

/// The edits a journal no longer lists: the world's edits after its first
/// `brushes` edits as a renderer snapshot (`helio-pass-voxel-planet`'s
/// `snapshot` format: the layers they left, not the brushes), and the
/// journal hash up to there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxelEditBase {
    pub brushes: usize,
    pub hash: u64,
    pub snapshot: std::sync::Arc<[u8]>,
}

/// A renderer's chunks of a journal's edits (`helio-pass-voxel-planet`'s
/// `KeptChunks::to_bytes`: what the edits made, so a loaded world copies
/// it instead of generating it from the terrain and every edit), for the
/// journal's first `brushes` edits, whose hash is `hash`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxelKeptChunks {
    pub brushes: usize,
    pub hash: u64,
    pub bytes: std::sync::Arc<[u8]>,
}

impl VoxelKeptChunks {
    /// `bytes` made for every edit of `journal`.
    pub fn of(journal: &VoxelEditJournal, bytes: std::sync::Arc<[u8]>) -> Self {
        let hash = journal
            .prefix_hash(journal.len())
            .expect("the journal's own length");
        Self {
            brushes: journal.len(),
            hash,
            bytes,
        }
    }
}

/// The ordered edit journal of a voxel world.
///
/// Copies share structure (edits live in shared chunks), and every edit
/// records a hash of the journal up to it, so the scene projection can hand
/// the journal to renderers every frame and they can tell in O(1) whether
/// it changed or only grew.
///
/// A journal may start from a [`VoxelEditBase`]: its oldest edits folded
/// into a snapshot of what they did ([`Self::compact`]), so a world that
/// saw millions of edits is saved and loaded by what it holds. The edits
/// after the base are listed; the base's are not and cannot be undone.
///
/// Edits belong to the terrain they were made on: `terrain` is that
/// terrain's fingerprint (form, voxel size, generator, seed and settings;
/// 0 before any). Another terrain starts with no edits ([`Self::made_on`]).
///
/// A save may keep a renderer's chunks of the journal's edits with it
/// ([`Self::keep_chunks`]); any change of the edits drops them.
/// Serialized as `{"terrain": .., "base": .., "edits": [..], "kept": ..}`.
#[derive(Clone, Default)]
pub struct VoxelEditJournal {
    base: Option<std::sync::Arc<VoxelEditBase>>,
    /// Chunks of every edit, the base's included (none past a change).
    kept: Option<std::sync::Arc<VoxelKeptChunks>>,
    /// The edits after the base.
    chunks: Vec<std::sync::Arc<Vec<(VoxelBrushEdit, u64)>>>,
    /// Every edit, the base's included.
    len: usize,
    terrain: u64,
}

/// FNV-1a over an edit, continuing `seed`.
fn edit_hash(seed: u64, edit: &VoxelBrushEdit) -> u64 {
    let mut h = seed ^ 0xcbf2_9ce4_8422_2325;
    let mut eat = |v: u64| {
        for byte in v.to_le_bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
    };
    for c in edit.center {
        eat(c.to_bits());
    }
    eat(edit.radius.to_bits());
    eat(edit.shape as u64);
    eat(edit.op as u64);
    eat(u64::from(edit.material));
    // Boxes only: cubes and balls keep the hashes they always had.
    if edit.height != 0.0 {
        eat(edit.height.to_bits());
    }
    h
}

impl VoxelEditJournal {
    /// Every edit, the base's included.
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// The snapshot of the oldest edits, if any were folded into one.
    pub fn base(&self) -> Option<&VoxelEditBase> {
        self.base.as_deref()
    }
    /// Edits in the base (none of them listed).
    pub fn base_len(&self) -> usize {
        self.base.as_ref().map_or(0, |b| b.brushes)
    }
    /// Edit `index`, when it is listed (after the base).
    pub fn get(&self, index: usize) -> Option<&VoxelBrushEdit> {
        let i = index.checked_sub(self.base_len())?;
        (index < self.len).then(|| &self.chunks[i / JOURNAL_CHUNK][i % JOURNAL_CHUNK].0)
    }
    /// Hash of the first `count` edits (0 for none), for counts from the
    /// base on.
    pub fn prefix_hash(&self, count: usize) -> Option<u64> {
        let base = self.base_len();
        match count.checked_sub(base)? {
            0 => Some(self.base.as_ref().map_or(0, |b| b.hash)),
            i if count <= self.len => {
                Some(self.chunks[(i - 1) / JOURNAL_CHUNK][(i - 1) % JOURNAL_CHUNK].1)
            }
            _ => None,
        }
    }
    pub fn push(&mut self, edit: VoxelBrushEdit) {
        let hash = edit_hash(
            self.prefix_hash(self.len)
                .expect("the journal's own length"),
            &edit,
        );
        if (self.len - self.base_len()) % JOURNAL_CHUNK == 0 {
            self.chunks
                .push(std::sync::Arc::new(Vec::with_capacity(JOURNAL_CHUNK)));
        }
        // Copies only this chunk when another journal still shares it.
        std::sync::Arc::make_mut(self.chunks.last_mut().expect("pushed above")).push((edit, hash));
        self.len += 1;
        self.kept = None;
    }
    /// Remove the newest edit (never one of the base's).
    pub fn pop(&mut self) -> Option<VoxelBrushEdit> {
        let chunk = self.chunks.last_mut()?;
        let (edit, _) = std::sync::Arc::make_mut(chunk).pop()?;
        if chunk.is_empty() {
            self.chunks.pop();
        }
        self.len -= 1;
        self.kept = None;
        Some(edit)
    }
    /// The listed edits (after the base), in order.
    pub fn listed(&self) -> impl Iterator<Item = &VoxelBrushEdit> {
        self.chunks
            .iter()
            .flat_map(|chunk| chunk.iter())
            .map(|(edit, _)| edit)
    }
    /// Edits from `start` on (from its chunk, not by walking the ones
    /// before). `start` must not be inside the base.
    pub fn iter_from(&self, start: usize) -> impl Iterator<Item = &VoxelBrushEdit> {
        let i = start
            .checked_sub(self.base_len())
            .expect("edits inside the base are not listed");
        self.chunks
            .iter()
            .skip(i / JOURNAL_CHUNK)
            .flat_map(|chunk| chunk.iter())
            .skip(i % JOURNAL_CHUNK)
            .map(|(edit, _)| edit)
    }
    /// Whether `prefix` is this journal's beginning (O(1), by hash).
    pub fn starts_with(&self, prefix: &VoxelEditJournal) -> bool {
        prefix.terrain == self.terrain
            && prefix.len <= self.len
            && self
                .prefix_hash(prefix.len)
                .is_some_and(|h| prefix.prefix_hash(prefix.len) == Some(h))
    }
    /// Fold the oldest edits into `base` (a snapshot of the world after its
    /// first `base.brushes` edits, made from this journal): they are no
    /// longer listed, and no longer undoable. Returns false (and changes
    /// nothing) when `base` does not start this journal.
    pub fn compact(&mut self, base: VoxelEditBase) -> bool {
        if base.brushes < self.base_len() || self.prefix_hash(base.brushes) != Some(base.hash) {
            return false;
        }
        let tail: Vec<(VoxelBrushEdit, u64)> = self
            .chunks
            .iter()
            .flat_map(|chunk| chunk.iter())
            .skip(base.brushes - self.base_len())
            .copied()
            .collect();
        self.chunks = tail
            .chunks(JOURNAL_CHUNK)
            .map(|chunk| std::sync::Arc::new(chunk.to_vec()))
            .collect();
        self.base = Some(std::sync::Arc::new(base));
        true
    }
    /// Keep a renderer's chunks of the edits with the journal (saved with
    /// it): whether they were made for exactly its edits (else they are not
    /// kept).
    pub fn keep_chunks(&mut self, kept: VoxelKeptChunks) -> bool {
        let fits = kept.brushes == self.len && self.prefix_hash(self.len) == Some(kept.hash);
        if fits {
            self.kept = Some(std::sync::Arc::new(kept));
        }
        fits
    }
    /// The renderer's chunks kept for the edits, until they change.
    pub fn kept_chunks(&self) -> Option<&VoxelKeptChunks> {
        self.kept.as_deref()
    }
    /// Fingerprint of the terrain the edits were made on (0: none yet).
    pub fn terrain(&self) -> u64 {
        self.terrain
    }
    /// Make the journal belong to `terrain`: edits made on another terrain
    /// are dropped (they would land on ground that is not there); a journal
    /// with no terrain yet adopts it. Returns how many edits were dropped.
    pub fn made_on(&mut self, terrain: u64) -> usize {
        if self.terrain == terrain {
            return 0;
        }
        if self.terrain == 0 {
            self.terrain = terrain;
            return 0;
        }
        let dropped = self.len;
        *self = Self {
            terrain,
            ..Self::default()
        };
        dropped
    }
}

impl PartialEq for VoxelEditJournal {
    /// Equal length and equal prefix hash (O(1)).
    fn eq(&self, other: &Self) -> bool {
        self.terrain == other.terrain
            && self.len == other.len
            && self.prefix_hash(self.len) == other.prefix_hash(other.len)
    }
}

impl std::fmt::Debug for VoxelEditJournal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoxelEditJournal")
            .field("terrain", &self.terrain)
            .field("base", &self.base_len())
            .field("len", &self.len)
            .finish()
    }
}

impl FromIterator<VoxelBrushEdit> for VoxelEditJournal {
    fn from_iter<I: IntoIterator<Item = VoxelBrushEdit>>(edits: I) -> Self {
        let mut journal = Self::default();
        edits.into_iter().for_each(|edit| journal.push(edit));
        journal
    }
}

impl Extend<VoxelBrushEdit> for VoxelEditJournal {
    fn extend<I: IntoIterator<Item = VoxelBrushEdit>>(&mut self, edits: I) {
        edits.into_iter().for_each(|edit| self.push(edit));
    }
}

/// The saved form of a [`VoxelEditBase`] (the snapshot in base64).
#[derive(Serialize, Deserialize)]
struct SavedBase {
    brushes: usize,
    hash: u64,
    snapshot: String,
}

/// The saved form of a [`VoxelKeptChunks`] (the bytes in base64).
#[derive(Serialize, Deserialize)]
struct SavedKept {
    brushes: usize,
    hash: u64,
    chunks: String,
}

/// The saved form of a [`VoxelEditJournal`].
#[derive(Serialize, Deserialize)]
struct SavedJournal<E> {
    #[serde(default)]
    terrain: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    base: Option<SavedBase>,
    #[serde(default)]
    edits: E,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kept: Option<SavedKept>,
}

impl Serialize for VoxelEditJournal {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let base = self.base().map(|b| SavedBase {
            brushes: b.brushes,
            hash: b.hash,
            snapshot: base64::encode(&b.snapshot),
        });
        let edits: Vec<&VoxelBrushEdit> = self.listed().collect();
        let kept = self.kept_chunks().map(|k| SavedKept {
            brushes: k.brushes,
            hash: k.hash,
            chunks: base64::encode(&k.bytes),
        });
        SavedJournal {
            terrain: self.terrain,
            base,
            edits,
            kept,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for VoxelEditJournal {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let saved = SavedJournal::<Vec<VoxelBrushEdit>>::deserialize(deserializer)?;
        let mut journal = Self {
            terrain: saved.terrain,
            ..Self::default()
        };
        if let Some(base) = saved.base {
            let snapshot = base64::decode(&base.snapshot)
                .ok_or_else(|| serde::de::Error::custom("voxel edit base: invalid base64"))?;
            journal.len = base.brushes;
            journal.base = Some(std::sync::Arc::new(VoxelEditBase {
                brushes: base.brushes,
                hash: base.hash,
                snapshot: snapshot.into(),
            }));
        }
        journal.extend(saved.edits);
        if let Some(kept) = saved.kept {
            let bytes = base64::decode(&kept.chunks)
                .ok_or_else(|| serde::de::Error::custom("voxel kept chunks: invalid base64"))?;
            // Chunks of other edits (a hand-edited level) are not kept.
            journal.keep_chunks(VoxelKeptChunks {
                brushes: kept.brushes,
                hash: kept.hash,
                bytes: bytes.into(),
            });
        }
        Ok(journal)
    }
}

/// Standard base64 (RFC 4648, padded) for snapshots in saved levels.
mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let n = (u32::from(chunk[0]) << 16)
                | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
                | u32::from(*chunk.get(2).unwrap_or(&0));
            for k in 0..4 {
                out.push(if k <= chunk.len() {
                    ALPHABET[(n >> (18 - 6 * k)) as usize & 63] as char
                } else {
                    '='
                });
            }
        }
        out
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        let text = text.as_bytes();
        if text.len() % 4 != 0 {
            return None;
        }
        let value = |c: u8| ALPHABET.iter().position(|&a| a == c).map(|v| v as u32);
        let mut out = Vec::with_capacity(text.len() / 4 * 3);
        for chunk in text.chunks(4) {
            let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
            if pad > 2 {
                return None;
            }
            let mut n = 0u32;
            for &c in &chunk[..4 - pad] {
                n = (n << 6) | value(c)?;
            }
            n <<= 6 * pad as u32;
            out.extend_from_slice(&n.to_be_bytes()[1..4 - pad]);
        }
        Some(out)
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;

    fn edit(x: f64) -> VoxelBrushEdit {
        VoxelBrushEdit {
            center: [x, 0.0, 0.0],
            radius: 1.0,
            shape: VoxelBrushShape::Sphere,
            op: VoxelBrushOp::Remove,
            material: 0,
            height: 0.0,
        }
    }

    #[test]
    fn journals_compare_by_prefix_and_serialize_with_their_terrain() {
        let a: VoxelEditJournal = (0..3000).map(|k| edit(k as f64)).collect();
        let mut b = a.clone();
        b.push(edit(-1.0));
        assert!(b.starts_with(&a) && !a.starts_with(&b) && a != b);
        assert_eq!(b.pop(), Some(edit(-1.0)));
        assert_eq!(a, b);
        b.pop();
        b.push(edit(12345.0));
        assert!(!b.starts_with(&a), "a changed edit is no longer a prefix");
        assert_eq!(a.get(2999), Some(&edit(2999.0)));
        assert_eq!(a.listed().count(), 3000);
        assert_eq!(a.iter_from(2998).count(), 2);

        let mut a = a;
        assert_eq!(
            a.made_on(6),
            0,
            "a journal without a terrain adopts the first"
        );
        assert_eq!((a.terrain(), a.len()), (6, 3000));
        assert_eq!(
            a.made_on(7),
            3000,
            "edits made on another terrain are dropped"
        );
        assert!(a.is_empty() && a.terrain() == 7);
        a.extend((0..3).map(|k| edit(k as f64)));
        assert_eq!(a.made_on(7), 0);
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(json["terrain"], 7);
        assert_eq!(json["edits"].as_array().unwrap().len(), 3);
        let back: VoxelEditJournal = serde_json::from_value(json).unwrap();
        assert_eq!(back, a);
        let mut other = back.clone();
        other.made_on(8);
        assert!(!other.starts_with(&back) && other != back);
    }

    #[test]
    fn a_compacted_journal_keeps_its_hashes_and_saves_its_base() {
        let full: VoxelEditJournal = (0..2500).map(|k| edit(k as f64)).collect();
        let mut j = full.clone();
        let base = VoxelEditBase {
            brushes: 2000,
            hash: full.prefix_hash(2000).unwrap(),
            snapshot: vec![0, 1, 2, 250, 251].into(),
        };
        assert!(
            !j.compact(VoxelEditBase {
                hash: 1,
                ..base.clone()
            }),
            "a base must start the journal"
        );
        assert!(j.compact(base.clone()));
        assert_eq!((j.len(), j.base_len()), (2500, 2000));
        assert_eq!(j, full, "compaction changes no edit");
        assert!(j.starts_with(&{
            let mut p = full.clone();
            for _ in 0..100 {
                p.pop();
            }
            p
        }));
        assert_eq!(j.get(1999), None);
        assert_eq!(j.get(2000), Some(&edit(2000.0)));
        assert_eq!(j.iter_from(2498).count(), 2);
        assert_eq!(j.listed().count(), 500);
        // Undo stops at the base.
        let mut undo = j.clone();
        while undo.pop().is_some() {}
        assert_eq!(undo.len(), 2000);
        // Saved and loaded with its base.
        let json = serde_json::to_value(&j).unwrap();
        assert_eq!(json["edits"].as_array().unwrap().len(), 500);
        let back: VoxelEditJournal = serde_json::from_value(json).unwrap();
        assert_eq!(back, j);
        assert_eq!(back.base(), Some(&base));
        let mut grown = back.clone();
        grown.push(edit(-5.0));
        assert!(grown.starts_with(&full) && grown.starts_with(&j));
    }

    #[test]
    fn kept_chunks_stay_with_exactly_their_edits() {
        let full: VoxelEditJournal = (0..2500).map(|k| edit(k as f64)).collect();
        let mut j = full.clone();
        let bytes: std::sync::Arc<[u8]> = vec![7, 8, 9, 200].into();
        let mut short = full.clone();
        short.pop();
        assert!(
            !j.keep_chunks(VoxelKeptChunks::of(&short, bytes.clone())),
            "chunks of other edits"
        );
        assert!(j.keep_chunks(VoxelKeptChunks::of(&full, bytes.clone())));
        assert_eq!(j.kept_chunks().map(|k| &k.bytes), Some(&bytes));
        assert_eq!(j, full, "kept chunks change no edit");
        // Compaction keeps them (the same edits); saved and loaded.
        assert!(j.compact(VoxelEditBase {
            brushes: 2000,
            hash: full.prefix_hash(2000).unwrap(),
            snapshot: vec![1, 2].into()
        }));
        assert!(j.kept_chunks().is_some());
        let back: VoxelEditJournal =
            serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert_eq!(back.kept_chunks(), j.kept_chunks());
        // Any change drops them.
        let mut grown = back.clone();
        grown.push(edit(-1.0));
        assert!(grown.kept_chunks().is_none());
        let mut undone = back.clone();
        undone.pop();
        assert!(undone.kept_chunks().is_none());
        // A saved journal whose chunks are of other edits loads without them.
        let mut json = serde_json::to_value(&back).unwrap();
        json["kept"]["brushes"] = 2499.into();
        let other: VoxelEditJournal = serde_json::from_value(json).unwrap();
        assert!(other.kept_chunks().is_none() && other == back);
    }

    #[test]
    fn saved_edits_load_bit_exact() {
        let j: VoxelEditJournal = (0..2000)
            .map(|k| edit(k as f64 * 0.1 + 1.0 / 3.0))
            .collect();
        let back: VoxelEditJournal =
            serde_json::from_str(&serde_json::to_string(&j).unwrap()).unwrap();
        assert_eq!(
            back, j,
            "every edit's bits (the journal hash) survive saving"
        );
    }

    #[test]
    fn base64_round_trips() {
        for len in 0..40 {
            let bytes: Vec<u8> = (0..len).map(|k| (k * 37 + 11) as u8).collect();
            assert_eq!(
                super::base64::decode(&super::base64::encode(&bytes)),
                Some(bytes)
            );
        }
        assert_eq!(super::base64::encode(b"Man"), "TWFu");
        assert_eq!(super::base64::encode(b"Ma"), "TWE=");
        assert_eq!(super::base64::decode("TQ=="), Some(b"M".to_vec()));
        assert_eq!(super::base64::decode("T!=="), None);
    }
}
