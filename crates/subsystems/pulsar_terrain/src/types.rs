use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub type MaterialId = u8;
pub const LOD0_CELL_SIZE_METERS: f64 = 0.1;
pub const PAGE_EDGE_CELLS: i64 = 32;
pub const MILLIMETER_INTERACTION_RADIUS_METERS: f64 = 8_192.0;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PlanetId(pub [u8; 16]);

impl PlanetId {
    pub fn from_stable_name(name: &str) -> Self {
        let digest = Sha256::digest(name.as_bytes());
        let mut id = [0_u8; 16];
        id.copy_from_slice(&digest[..16]);
        Self(id)
    }

    pub fn from_hex(value: &str) -> Result<Self, PlanetIdParseError> {
        if value.len() != 32 {
            return Err(PlanetIdParseError::Length(value.len()));
        }
        let mut id = [0_u8; 16];
        for (index, output) in id.iter_mut().enumerate() {
            let offset = index * 2;
            *output = u8::from_str_radix(&value[offset..offset + 2], 16)
                .map_err(|_| PlanetIdParseError::Hex { offset })?;
        }
        Ok(Self(id))
    }

    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(32);
        for byte in self.0 {
            use std::fmt::Write as _;
            write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
        }
        output
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum PlanetIdParseError {
    #[error("planet id must contain exactly 32 hexadecimal characters, got {0}")]
    Length(usize),
    #[error("planet id contains invalid hexadecimal at byte offset {offset}")]
    Hex { offset: usize },
}

/// Authoritative planet-space position at 10 cm LOD0 resolution.
///
/// `lod0_cell` is the persistent integer address. `subcell_m` is private and
/// normalized to `[0, 0.1)` meters on every axis, including negative world
/// coordinates. Serde deserialization runs the same validation as `new`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PlanetPositionWire", into = "PlanetPositionWire")]
pub struct PlanetPosition {
    lod0_cell: [i64; 3],
    subcell_m: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanetPositionWire {
    lod0_cell: [i64; 3],
    subcell_m: [f64; 3],
}

impl PlanetPosition {
    pub fn new(lod0_cell: [i64; 3], mut subcell_m: [f64; 3]) -> Result<Self, PositionError> {
        for value in &mut subcell_m {
            if !value.is_finite() {
                return Err(PositionError::NonFinite);
            }
            if !(0.0..LOD0_CELL_SIZE_METERS).contains(value) {
                return Err(PositionError::SubcellOutOfRange);
            }
            if *value == -0.0 {
                *value = 0.0;
            }
        }
        Ok(Self {
            lod0_cell,
            subcell_m,
        })
    }

    pub const fn from_lod0_cell(lod0_cell: [i64; 3]) -> Self {
        Self {
            lod0_cell,
            subcell_m: [0.0; 3],
        }
    }

    /// Convenience conversion for camera/input values. Terrain persistence and
    /// replication should carry the canonical cell-plus-remainder form.
    pub fn from_meters(meters: [f64; 3]) -> Result<Self, PositionError> {
        let mut cells = [0_i64; 3];
        let mut subcell_m = [0.0_f64; 3];
        for axis in 0..3 {
            let value = meters[axis];
            if !value.is_finite() {
                return Err(PositionError::NonFinite);
            }
            let cell = (value / LOD0_CELL_SIZE_METERS).floor();
            if cell < i64::MIN as f64 || cell >= -(i64::MIN as f64) {
                return Err(PositionError::CoordinateOverflow);
            }
            cells[axis] = cell as i64;
            let mut remainder = value - cell * LOD0_CELL_SIZE_METERS;
            if remainder < 0.0 {
                cells[axis] = cells[axis]
                    .checked_sub(1)
                    .ok_or(PositionError::CoordinateOverflow)?;
                remainder += LOD0_CELL_SIZE_METERS;
            } else if remainder >= LOD0_CELL_SIZE_METERS {
                cells[axis] = cells[axis]
                    .checked_add(1)
                    .ok_or(PositionError::CoordinateOverflow)?;
                remainder -= LOD0_CELL_SIZE_METERS;
            }
            subcell_m[axis] = remainder;
        }
        Self::new(cells, subcell_m)
    }

    pub const fn lod0_cell(self) -> [i64; 3] {
        self.lod0_cell
    }

    pub const fn subcell_m(self) -> [f64; 3] {
        self.subcell_m
    }

    /// Computes `self - origin` before any conversion to floating point.
    pub fn relative_meters(self, origin: Self) -> Result<[f64; 3], PositionError> {
        let mut relative = [0.0_f64; 3];
        for (axis, output) in relative.iter_mut().enumerate() {
            let cell_delta = self.lod0_cell[axis]
                .checked_sub(origin.lod0_cell[axis])
                .ok_or(PositionError::CoordinateOverflow)?;
            *output = cell_delta as f64 * LOD0_CELL_SIZE_METERS
                + (self.subcell_m[axis] - origin.subcell_m[axis]);
        }
        Ok(relative)
    }

    pub fn relative_to_lod0_cell(
        self,
        origin_lod0_cell: [i64; 3],
    ) -> Result<[f64; 3], PositionError> {
        self.relative_meters(Self::from_lod0_cell(origin_lod0_cell))
    }
}

impl Default for PlanetPosition {
    fn default() -> Self {
        Self::from_lod0_cell([0; 3])
    }
}

impl TryFrom<PlanetPositionWire> for PlanetPosition {
    type Error = PositionError;

    fn try_from(value: PlanetPositionWire) -> Result<Self, Self::Error> {
        Self::new(value.lod0_cell, value.subcell_m)
    }
}

impl From<PlanetPosition> for PlanetPositionWire {
    fn from(value: PlanetPosition) -> Self {
        Self {
            lod0_cell: value.lod0_cell,
            subcell_m: value.subcell_m,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetFrame {
    planet: PlanetId,
    camera: PlanetPosition,
    origin_lod0_cell: [i64; 3],
    frame_index: u64,
}

impl PlanetFrame {
    pub fn new(planet: PlanetId, camera: PlanetPosition, frame_index: u64) -> Self {
        let origin_lod0_cell = camera
            .lod0_cell
            .map(|axis| axis.div_euclid(PAGE_EDGE_CELLS) * PAGE_EDGE_CELLS);
        Self {
            planet,
            camera,
            origin_lod0_cell,
            frame_index,
        }
    }

    pub const fn planet(self) -> PlanetId {
        self.planet
    }

    pub const fn camera(self) -> PlanetPosition {
        self.camera
    }

    pub const fn origin_lod0_cell(self) -> [i64; 3] {
        self.origin_lod0_cell
    }

    pub const fn frame_index(self) -> u64 {
        self.frame_index
    }

    pub fn camera_relative_m(self) -> [f64; 3] {
        self.camera
            .relative_to_lod0_cell(self.origin_lod0_cell)
            .expect("a page-snapped camera origin cannot overflow")
    }

    pub fn camera_local_meters(self, position: PlanetPosition) -> Result<[f64; 3], PositionError> {
        position.relative_meters(self.camera)
    }

    pub fn renderer_payload(self) -> PlanetFramePayload {
        PlanetFramePayload {
            planet_id_words: planet_words(self.planet),
            origin_words: self.origin_lod0_cell.map(split_i64),
            frame_index_words: split_u64(self.frame_index),
            camera_relative_m: self.camera_relative_m().map(|value| value as f32),
            lod0_cell_size_m: LOD0_CELL_SIZE_METERS as f32,
            page_edge_cells: PAGE_EDGE_CELLS as u32,
        }
    }
}

/// Renderer-neutral payload matching Helio's planetary frame fields without a
/// Helio dependency or revision pin in the authoritative terrain crate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetFramePayload {
    planet_id_words: [u32; 4],
    origin_words: [[u32; 2]; 3],
    frame_index_words: [u32; 2],
    camera_relative_m: [f32; 3],
    lod0_cell_size_m: f32,
    page_edge_cells: u32,
}

impl PlanetFramePayload {
    pub const fn planet_id_words(self) -> [u32; 4] {
        self.planet_id_words
    }

    pub fn planet_id(self) -> PlanetId {
        let mut bytes = [0_u8; 16];
        for (word, output) in self.planet_id_words.iter().zip(bytes.chunks_exact_mut(4)) {
            output.copy_from_slice(&word.to_le_bytes());
        }
        PlanetId(bytes)
    }

    pub const fn origin_words(self) -> [[u32; 2]; 3] {
        self.origin_words
    }

    pub const fn origin_lod0_cell(self) -> [i64; 3] {
        [
            join_i64(self.origin_words[0]),
            join_i64(self.origin_words[1]),
            join_i64(self.origin_words[2]),
        ]
    }

    pub const fn frame_index_words(self) -> [u32; 2] {
        self.frame_index_words
    }

    pub const fn frame_index(self) -> u64 {
        join_u64(self.frame_index_words)
    }

    pub const fn camera_relative_m(self) -> [f32; 3] {
        self.camera_relative_m
    }

    pub const fn lod0_cell_size_m(self) -> f32 {
        self.lod0_cell_size_m
    }

    pub const fn page_edge_cells(self) -> u32 {
        self.page_edge_cells
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum PositionError {
    #[error("planet position contains a non-finite value")]
    NonFinite,
    #[error("planet position sub-cell remainder must be in [0, 0.1) meters")]
    SubcellOutOfRange,
    #[error("planet position coordinate arithmetic overflowed")]
    CoordinateOverflow,
}

const fn split_u64(value: u64) -> [u32; 2] {
    [value as u32, (value >> 32) as u32]
}

const fn split_i64(value: i64) -> [u32; 2] {
    split_u64(value as u64)
}

const fn join_u64(words: [u32; 2]) -> u64 {
    (words[0] as u64) | ((words[1] as u64) << 32)
}

const fn join_i64(words: [u32; 2]) -> i64 {
    join_u64(words) as i64
}

fn planet_words(planet: PlanetId) -> [u32; 4] {
    let mut words = [0_u32; 4];
    for (word, bytes) in words.iter_mut().zip(planet.0.chunks_exact(4)) {
        *word = u32::from_le_bytes(bytes.try_into().expect("chunks_exact yields four bytes"));
    }
    words
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct PageKey {
    pub lod: u8,
    pub page_xyz: [i64; 3],
}

impl PageKey {
    pub const fn new(lod: u8, page_xyz: [i64; 3]) -> Self {
        Self { lod, page_xyz }
    }

    pub fn parent(self) -> Option<Self> {
        Some(Self {
            lod: self.lod.checked_add(1)?,
            page_xyz: self.page_xyz.map(|axis| axis.div_euclid(2)),
        })
    }

    pub fn lod0_min(self) -> Option<[i64; 3]> {
        let scale = 1_i64.checked_shl(u32::from(self.lod))?;
        Some([
            self.page_xyz[0].checked_mul(scale)?,
            self.page_xyz[1].checked_mul(scale)?,
            self.page_xyz[2].checked_mul(scale)?,
        ])
    }

    /// Number of canonical LOD0 cells covered by one axis of this page.
    pub fn lod0_cell_span(self) -> Option<i64> {
        PAGE_EDGE_CELLS.checked_shl(u32::from(self.lod))
    }

    /// Canonical minimum LOD0-cell coordinate covered by this page.
    pub fn lod0_cell_min(self) -> Option<[i64; 3]> {
        let span = self.lod0_cell_span()?;
        Some([
            self.page_xyz[0].checked_mul(span)?,
            self.page_xyz[1].checked_mul(span)?,
            self.page_xyz[2].checked_mul(span)?,
        ])
    }

    /// Convert a canonical LOD0-cell coordinate into a page and its 0..32
    /// cell coordinate at the requested LOD. Euclidean division keeps the
    /// mapping continuous and unambiguous across every negative-axis boundary.
    pub fn address_lod0_cell(lod: u8, cell_xyz: [i64; 3]) -> Option<(Self, [u8; 3])> {
        let scale = 1_i64.checked_shl(u32::from(lod))?;
        let span = PAGE_EDGE_CELLS.checked_mul(scale)?;
        let page_xyz = cell_xyz.map(|axis| axis.div_euclid(span));
        let local = cell_xyz.map(|axis| (axis.rem_euclid(span) / scale) as u8);
        Some((Self { lod, page_xyz }, local))
    }
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    pub fn to_hex(self) -> String {
        let mut output = String::with_capacity(64);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in self.0 {
            output.push(HEX[usize::from(byte >> 4)] as char);
            output.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        output
    }
}

pub type PageId = ContentHash;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeState {
    Air,
    Solid(MaterialId),
    Procedural(ContentHash),
    Branch,
    Page(PageId),
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellWord(pub u32);

impl CellWord {
    pub const AIR: Self = Self::new(i16::MAX, 0, 0);

    pub const fn new(density: i16, material: MaterialId, flags: u8) -> Self {
        Self((density as u16 as u32) | ((material as u32) << 16) | ((flags as u32) << 24))
    }

    pub const fn density(self) -> i16 {
        self.0 as u16 as i16
    }

    pub const fn material(self) -> MaterialId {
        (self.0 >> 16) as u8
    }

    pub const fn flags(self) -> u8 {
        (self.0 >> 24) as u8
    }

    pub const fn is_solid(self) -> bool {
        self.density() <= 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_cell_to_page_conversion_is_exact_at_every_boundary() {
        for lod in 0..=30 {
            let scale = 1_i64 << lod;
            let span = PAGE_EDGE_CELLS * scale;
            let boundaries = [
                -span - 1,
                -span,
                -span + 1,
                -1,
                0,
                1,
                span - 1,
                span,
                span + 1,
            ];
            for coordinate in boundaries {
                let cell = [coordinate, -coordinate, coordinate.saturating_sub(1)];
                let (key, local) = PageKey::address_lod0_cell(lod, cell).unwrap();
                let page_min = key.lod0_cell_min().unwrap();
                for axis in 0..3 {
                    assert!(local[axis] < PAGE_EDGE_CELLS as u8);
                    let reconstructed = page_min[axis] + i64::from(local[axis]) * scale;
                    assert!(reconstructed <= cell[axis]);
                    assert!(cell[axis] < reconstructed + scale);
                }
            }
        }
    }

    #[test]
    fn page_parent_uses_euclidean_division_on_negative_axes() {
        assert_eq!(
            PageKey::new(0, [-1, -2, -3]).parent(),
            Some(PageKey::new(1, [-1, -1, -2]))
        );
    }

    #[test]
    fn planet_ids_round_trip_and_stable_names_are_deterministic() {
        let id = PlanetId::from_stable_name("scene/earth:0");
        assert_eq!(PlanetId::from_hex(&id.to_hex()).unwrap(), id);
        assert_eq!(PlanetId::from_stable_name("scene/earth:0"), id);
        assert_ne!(PlanetId::from_stable_name("scene/earth:1"), id);
    }

    #[test]
    fn position_serde_is_canonical_and_rejects_invalid_payloads() {
        let position = PlanetPosition::new([-63_710_001, 7, -1], [0.099, 0.0, -0.0]).unwrap();
        let json = serde_json::to_string(&position).unwrap();
        assert_eq!(
            json,
            r#"{"lod0_cell":[-63710001,7,-1],"subcell_m":[0.099,0.0,0.0]}"#
        );
        assert_eq!(
            serde_json::from_str::<PlanetPosition>(&json).unwrap(),
            position
        );
        assert!(serde_json::from_str::<PlanetPosition>(
            r#"{"lod0_cell":[0,0,0],"subcell_m":[0.1,0.0,0.0]}"#
        )
        .is_err());
        assert!(serde_json::from_str::<PlanetPosition>(
            r#"{"sector":[0,0,0],"offset_m":[0.0,0.0,0.0]}"#
        )
        .is_err());
    }

    #[test]
    fn negative_meter_coordinates_use_euclidean_cells() {
        let position = PlanetPosition::from_meters([-0.001, -0.1, -3.201]).unwrap();
        assert_eq!(position.lod0_cell(), [-1, -1, -33]);
        let expected = [0.099, 0.0, 0.099];
        for (actual, expected) in position.subcell_m().into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1.0e-12);
        }
    }

    #[test]
    fn earth_ground_orbit_and_antipode_frames_stay_canonical() {
        let earth_cells = 63_710_000_i64;
        let ground = PlanetPosition::new([earth_cells, 0, 0], [0.001, 0.099, 0.05]).unwrap();
        let orbit = PlanetPosition::new([67_710_000, 0, 0], [0.001, 0.099, 0.05]).unwrap();
        let antipode = PlanetPosition::new([-earth_cells, 0, 0], [0.001, 0.099, 0.05]).unwrap();
        assert_eq!(
            orbit.relative_meters(ground).unwrap(),
            [400_000.0, 0.0, 0.0]
        );
        assert_eq!(
            ground.relative_meters(antipode).unwrap(),
            [12_742_000.0, 0.0, 0.0]
        );

        let frame = PlanetFrame::new(PlanetId([4; 16]), ground, 17);
        assert_eq!(frame.origin_lod0_cell(), [63_709_984, 0, 0]);
        assert_eq!(frame.camera_relative_m(), [1.601, 0.099, 0.05]);
        let payload = frame.renderer_payload();
        assert_eq!(payload.planet_id(), PlanetId([4; 16]));
        assert_eq!(payload.planet_id_words(), [0x0404_0404; 4]);
        assert_eq!(payload.origin_lod0_cell(), frame.origin_lod0_cell());
        assert_eq!(payload.frame_index(), 17);
        assert_eq!(payload.lod0_cell_size_m(), 0.1_f32);
        assert_eq!(payload.page_edge_cells(), 32);
    }

    #[test]
    fn signed_origin_and_frame_words_round_trip_losslessly() {
        for camera_cell in [
            [i64::MIN, -1, 0],
            [-63_710_017, 63_710_017, -32],
            [i64::MAX, i64::MAX - 31, 31],
        ] {
            let frame = PlanetFrame::new(
                PlanetId([8; 16]),
                PlanetPosition::from_lod0_cell(camera_cell),
                u64::MAX,
            );
            let payload = frame.renderer_payload();
            assert_eq!(payload.origin_lod0_cell(), frame.origin_lod0_cell());
            assert_eq!(payload.frame_index(), u64::MAX);
        }
    }

    #[test]
    fn renderer_payload_stays_within_one_millimeter_across_a_rebase() {
        let earth_cells = 63_710_000_i64;
        for camera_cell in [earth_cells + 31, earth_cells + 32] {
            let camera = PlanetPosition::new([camera_cell, -1, 7], [0.037, 0.081, 0.019]).unwrap();
            let point = PlanetPosition::new(
                [
                    camera_cell
                        + (MILLIMETER_INTERACTION_RADIUS_METERS / LOD0_CELL_SIZE_METERS) as i64
                        - 1,
                    -4,
                    9,
                ],
                [0.092, 0.006, 0.071],
            )
            .unwrap();
            let frame = PlanetFrame::new(PlanetId([2; 16]), camera, 5);
            let payload = frame.renderer_payload();
            let expected = frame.camera_local_meters(point).unwrap();
            let origin = payload.origin_lod0_cell();
            let subcell = point.subcell_m();
            let camera_relative = payload.camera_relative_m();
            let reconstructed = [
                ((point.lod0_cell()[0] - origin[0]) as f32
                    + (subcell[0] / LOD0_CELL_SIZE_METERS) as f32)
                    * payload.lod0_cell_size_m()
                    - camera_relative[0],
                ((point.lod0_cell()[1] - origin[1]) as f32
                    + (subcell[1] / LOD0_CELL_SIZE_METERS) as f32)
                    * payload.lod0_cell_size_m()
                    - camera_relative[1],
                ((point.lod0_cell()[2] - origin[2]) as f32
                    + (subcell[2] / LOD0_CELL_SIZE_METERS) as f32)
                    * payload.lod0_cell_size_m()
                    - camera_relative[2],
            ];
            for (actual, expected) in reconstructed.into_iter().zip(expected) {
                assert!((f64::from(actual) - expected).abs() <= 0.001);
            }
        }
    }
}
