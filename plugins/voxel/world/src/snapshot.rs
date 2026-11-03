//! Edit snapshots: a planet's edits as their layers, loaded without
//! replaying the brushes that made them.
//!
//! A host's brush journal grows with every stroke and replays them all on
//! load. A snapshot stores what the strokes left instead (see
//! `planet::Edits`): the large analytic brushes, the baked bricks of every
//! level, the recent brushes, and the history's length and hash, so a
//! journal's tail after the brushes a snapshot covers can be applied on top.
//! Little-endian:
//!
//! * a 16-byte header: magic `HVPS`, format version, and a fingerprint of
//!   the grid (form, radius, voxel size). Baked edits do not depend on the
//!   terrain under them, so a snapshot loads onto any terrain of that grid
//!   (a generator update keeps a world's edits), never onto another grid;
//! * brush count, history hash, sealed brush count and hash, edit bounds;
//! * the large brushes, then the recent ones, as brush records (`record`);
//! * the baked bricks: key, then runs of equal cells (large brushes baked
//!   at their bake level too);
//! * a checksum of everything before it.
use crate::edit_store::{Brick, BrickKey, CellEdit, EditStore, BRICK_CELLS};
use crate::edits::Brush;
use crate::grid::Grid;
use crate::planet::{Edits, Planet, PlanetRecipe};
use crate::record;

const MAGIC: [u8; 4] = *b"HVPS";
/// The format (only this one reads).
const VERSION: u16 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    /// Not a snapshot, or cut short.
    NotASnapshot,
    UnsupportedVersion(u16),
    /// Written for a different grid.
    GridMismatch {
        snapshot: u64,
        planet: u64,
    },
    /// Failed its checksum or holds invalid data.
    Corrupt(&'static str),
    /// Its brushes could not be applied to the planet.
    Rejected(String),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotASnapshot => write!(f, "not a planet edit snapshot"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported snapshot version {v}"),
            Self::GridMismatch { snapshot, planet } => {
                write!(
                    f,
                    "snapshot grid {snapshot:016x} does not match planet grid {planet:016x}"
                )
            }
            Self::Corrupt(reason) => write!(f, "snapshot is corrupt: {reason}"),
            Self::Rejected(reason) => write!(f, "snapshot was rejected: {reason}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

/// The history a snapshot holds: its brush count and hash (see
/// [`crate::edits::history_hash`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Covers {
    brushes: usize,
    hash: u64,
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SnapshotError> {
        let out = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or(SnapshotError::NotASnapshot)?;
        self.at += n;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, SnapshotError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, SnapshotError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32, SnapshotError> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f64(&mut self) -> Result<f64, SnapshotError> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn brushes(&mut self) -> Result<Vec<Brush>, SnapshotError> {
        let n = self.u32()? as usize;
        (0..n)
            .map(|index| {
                record::decode(index, self.take(record::RECORD_BYTES)?)
                    .map_err(|_| SnapshotError::Corrupt("brush record"))
            })
            .collect()
    }
}

/// Fingerprint of a grid: its form, size and voxel size.
pub fn grid_fingerprint(grid: &Grid) -> u64 {
    let form = format!("{:?}", grid.shape());
    let mut bytes = form.into_bytes();
    for v in [
        grid.radius().to_bits(),
        grid.voxel_size().to_bits(),
        grid.cells() as u64,
    ] {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    record::fnv64(&bytes)
}

/// Header and checksum checks; the reader starts after the header.
fn open<'a>(bytes: &'a [u8], grid: &Grid) -> Result<Reader<'a>, SnapshotError> {
    if bytes.len() < 24 || bytes[..4] != MAGIC {
        return Err(SnapshotError::NotASnapshot);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(SnapshotError::UnsupportedVersion(version));
    }
    let snapshot = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let planet = grid_fingerprint(grid);
    if snapshot != planet {
        return Err(SnapshotError::GridMismatch { snapshot, planet });
    }
    let (body, sum) = bytes.split_at(bytes.len() - 8);
    if u64::from_le_bytes(sum.try_into().unwrap()) != record::fnv64(body) {
        return Err(SnapshotError::Corrupt("checksum"));
    }
    Ok(Reader {
        bytes: body,
        at: 16,
    })
}

impl Planet {
    /// Snapshot of the edits (see the module doc).
    pub fn snapshot(&self) -> Vec<u8> {
        let edits = self.edits();
        let mut out = vec![0u8; 16];
        out[..4].copy_from_slice(&MAGIC);
        out[4..6].copy_from_slice(&VERSION.to_le_bytes());
        out[8..16].copy_from_slice(&grid_fingerprint(self.grid()).to_le_bytes());
        let crate::planet::BakedState {
            sealed,
            hash: sealed_hash,
            ..
        } = edits.baked_state();
        let (top, bottom) = self.edit_bounds();
        for v in [
            edits.len() as u64,
            edits.hash(),
            sealed as u64,
            sealed_hash,
            top.to_bits(),
            bottom.to_bits(),
        ] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for log in [&edits.large, &edits.recent] {
            out.extend_from_slice(&(log.len() as u32).to_le_bytes());
            for (index, brush) in log.brushes().enumerate() {
                out.extend_from_slice(&record::encode(index as u32, brush));
            }
        }
        let mut bricks: Vec<_> = edits.baked.bricks().collect();
        bricks.sort_unstable_by_key(|(key, _)| *key);
        out.extend_from_slice(&(bricks.len() as u64).to_le_bytes());
        for (key, brick) in bricks {
            out.push(key.face);
            out.push(key.level);
            for v in [key.bi, key.bj, key.bk] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            let runs: Vec<(u16, u16)> = brick
                .cells
                .chunk_by(|a, b| a == b)
                .map(|run| (run.len() as u16, run[0].0))
                .collect();
            out.extend_from_slice(&(runs.len() as u16).to_le_bytes());
            for (len, value) in runs {
                out.extend_from_slice(&len.to_le_bytes());
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        let sum = record::fnv64(&out);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// A planet from its recipe and an edit snapshot.
    pub fn from_snapshot(recipe: PlanetRecipe, bytes: &[u8]) -> Result<Self, SnapshotError> {
        // Built first: the planet names the concrete generator version its
        // recipe resolves to, which the snapshot was written with.
        let mut planet = Planet::new(recipe).map_err(SnapshotError::Rejected)?;
        let grid = *planet.grid();
        let mut r = open(bytes, &grid)?;
        let covers = Covers {
            brushes: r.u64()? as usize,
            hash: r.u64()?,
        };
        let sealed = (r.u64()? as usize, r.u64()?);
        let bounds = (r.f64()?, r.f64()?);
        let large = r.brushes()?;
        let recent = r.brushes()?;
        let mut baked = EditStore::default();
        for _ in 0..r.u64()? {
            let key = BrickKey {
                face: r.u8()?,
                level: r.u8()?,
                bi: r.i32()?,
                bj: r.i32()?,
                bk: r.i32()?,
            };
            let mut brick = Brick::default();
            let mut at = 0;
            for _ in 0..r.u16()? {
                let (len, value) = (r.u16()? as usize, r.u16()?);
                let run = brick
                    .cells
                    .get_mut(at..at + len)
                    .ok_or(SnapshotError::Corrupt("brick runs"))?;
                run.fill(CellEdit(value));
                at += len;
            }
            if at != BRICK_CELLS || key.face > 5 {
                return Err(SnapshotError::Corrupt("brick"));
            }
            baked.set(key, Some(brick));
        }
        if r.at != r.bytes.len() {
            return Err(SnapshotError::Corrupt("trailing data"));
        }
        let edits = Edits::from_parts(planet.grid(), &large, baked, sealed, &recent)
            .map_err(SnapshotError::Rejected)?;
        if (edits.len(), edits.hash()) != (covers.brushes, covers.hash) {
            return Err(SnapshotError::Corrupt("history"));
        }
        planet.restore_edits(edits, bounds);
        Ok(planet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edits::{BrushOp, BrushShape};
    use crate::grid::Cell;

    /// A planet with sealed (baked and large) and recent edits.
    fn edited() -> (Planet, Vec<Brush>) {
        let mut p = Planet::new(PlanetRecipe::default()).unwrap();
        let g = *p.grid();
        let (face, i0, j0) = (2u8, g.cells() / 2 + 8, g.cells() / 2 - 24);
        let top = p.column_top(face, i0 + 20, j0 + 20, 0);
        let mut applied = Vec::new();
        for n in 0..700 {
            let cell = Cell::new(
                face,
                i0 + (n * 7 % 40),
                j0 + (n * 13 % 40),
                top - 12 + (n % 20),
            );
            let brush = Brush {
                center: g.cell_center(cell).to_array(),
                radius: if n % 90 == 11 {
                    4.0
                } else {
                    0.1 + f64::from(n % 6) * 0.1
                },
                shape: if n % 2 == 0 {
                    BrushShape::Sphere
                } else {
                    BrushShape::Cube
                },
                op: [BrushOp::Remove, BrushOp::Add, BrushOp::Paint][n as usize % 3],
                material: 1 + (n as u32 % 9),
                height: 0.0,
            };
            p.apply(brush).unwrap();
            applied.push(brush);
        }
        assert!(p.edits().sealed_len() > 0 && !p.edits().large.is_empty());
        (p, applied)
    }

    #[test]
    fn snapshots_load_identical_edits_and_take_further_brushes() {
        let (a, applied) = edited();
        let bytes = a.snapshot();
        assert_eq!(
            (a.edits().len(), a.edits().hash()),
            (applied.len(), crate::edits::history_hash(applied.iter()))
        );
        let mut b = Planet::from_snapshot(PlanetRecipe::default(), &bytes).unwrap();
        assert_eq!(
            (b.edits().len(), b.edits().hash()),
            (a.edits().len(), a.edits().hash())
        );
        assert_eq!(
            b.edits().baked.brick_counts(),
            a.edits().baked.brick_counts()
        );
        for (key, brick) in a.edits().baked.bricks() {
            assert_eq!(b.edits().baked.brick(&key), Some(brick));
        }
        assert_eq!(b.snapshot(), bytes, "a loaded snapshot writes itself back");
        // The same brush on both continues identically (sealing included).
        let mut a = a;
        for brush in &applied[..300] {
            a.apply(*brush).unwrap();
            b.apply(*brush).unwrap();
        }
        assert_eq!(a.edits().hash(), b.edits().hash());
        assert_eq!(a.snapshot(), b.snapshot());
    }

    #[test]
    fn corrupt_foreign_and_truncated_snapshots_are_rejected() {
        let (p, _) = edited();
        let bytes = p.snapshot();
        let mut flipped = bytes.clone();
        flipped[bytes.len() / 2] ^= 0x10;
        assert_eq!(
            Planet::from_snapshot(PlanetRecipe::default(), &flipped).err(),
            Some(SnapshotError::Corrupt("checksum"))
        );
        // Another terrain on the same grid takes the edits (a generator
        // update keeps them); another grid does not.
        let hills = PlanetRecipe {
            terrain: crate::layers::TerrainLayers::earth()
                .heightfield()
                .source(7),
            ..PlanetRecipe::default()
        };
        assert_eq!(
            Planet::from_snapshot(hills, &bytes).unwrap().edits().hash(),
            p.edits().hash()
        );
        let other = PlanetRecipe {
            voxel_size_m: 0.3,
            ..PlanetRecipe::default()
        };
        assert!(matches!(
            Planet::from_snapshot(other, &bytes),
            Err(SnapshotError::GridMismatch { .. })
        ));
        assert!(Planet::from_snapshot(PlanetRecipe::default(), &bytes[..bytes.len() - 3]).is_err());
        assert!(matches!(
            Planet::from_snapshot(PlanetRecipe::default(), b"nope"),
            Err(SnapshotError::NotASnapshot)
        ));
    }
}
