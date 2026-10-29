//! Terrain mode's own GPUI dock panels (the "full GPUI in a mode" contract,
//! design doc §11). One file per panel plus a shared widget kit:
//!
//! - [`terrain`] — the one Terrain panel: the World tab (ready-made voxel
//!   worlds, the level's terrains) and the Foliage tab
//! - [`foliage`] — foliage sets/members and their brush
//! - `widgets`   — tool grid, collapsible sections, value boxes

mod foliage;
mod terrain;
mod widgets;

pub use foliage::FoliageSetsPanel;
pub use terrain::TerrainPanel;
