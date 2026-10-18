//! # Import companions
//!
//! A plugin can offer to do extra work when the user imports a source file,
//! without the engine knowing what that work is. The shader plugin, for
//! example, offers to build a material from the images found next to an
//! imported model; the import pipeline only knows "a companion has an offer
//! for these sources", shows its label, and runs it if the user accepts.
//!
//! The contract is deliberately one-directional and made of neutral data:
//!
//! - the host describes what it found in the source as [`SourceSurface`]s
//!   (a named surface slot plus the image files it refers to, by role);
//! - the companion answers with an [`ImportOffer`] and, when accepted,
//!   [`CompanionOutput`]s: files it wrote, optionally to be used as the
//!   default for a surface slot.
//!
//! Nothing here names a material, a shader or a blueprint. A companion is
//! registered by the host (built-in providers at startup); the registry lives
//! in the host, since a plugin library links its own copy of this crate.

use std::path::{Path, PathBuf};

/// What an image is for, as far as the source says.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceImageRole {
    BaseColor,
    Normal,
    /// A roughness map on its own.
    Roughness,
    /// A metalness map on its own.
    Metallic,
    /// Roughness and metalness packed in one image (green and blue).
    MetallicRoughness,
    Occlusion,
    Emissive,
}

/// An image file a source surface refers to, located on disk.
#[derive(Clone, Debug)]
pub struct SourceImage {
    pub role: SourceImageRole,
    pub path: PathBuf,
}

/// One surface slot of an imported source (an FBX material) and the image
/// files found for it.
#[derive(Clone, Debug)]
pub struct SourceSurface {
    /// The slot index on the native asset this surface belongs to.
    pub slot: u32,
    pub name: String,
    pub images: Vec<SourceImage>,
}

/// What the host knows before the user decides, cheap to compute.
pub struct ImportProbe<'a> {
    /// The importer that will convert the sources (`mesh`).
    pub importer: &'a str,
    /// The source files about to be imported.
    pub sources: &'a [PathBuf],
    /// Image files found next to the sources (same folder, a `textures`
    /// subfolder, the model's `.fbm` folder).
    pub nearby_images: &'a [PathBuf],
}

/// A companion's offer, shown as an option in the import configurator.
#[derive(Clone, Debug)]
pub struct ImportOffer {
    pub label: String,
    pub description: String,
    pub default_enabled: bool,
}

/// What a companion works from when it runs, after the native asset exists.
pub struct ImportCompanionContext<'a> {
    pub project_root: &'a Path,
    pub source: &'a Path,
    /// The primary native asset the import wrote.
    pub native_asset: &'a Path,
    pub surfaces: &'a [SourceSurface],
}

/// A file a companion wrote.
#[derive(Clone, Debug)]
pub struct CompanionOutput {
    /// Absolute path of the file or folder written (or already present).
    pub path: PathBuf,
    /// What it is, in the companion's own words; recorded with the import.
    pub kind: String,
    /// The surface name it was made from, to pair it on the next import.
    pub source_name: Option<String>,
    /// Use it as the native asset's default for this surface slot, if the
    /// slot has none yet.
    pub assign_to_slot: Option<u32>,
}

pub trait ImportCompanion: Send + Sync {
    /// Stable id, stored with the import so a re-import runs it again.
    fn id(&self) -> &'static str;

    /// Whether this companion can help with these sources. `None` keeps the
    /// option out of the configurator.
    fn offer(&self, probe: &ImportProbe) -> Option<ImportOffer>;

    /// Do the work. Must not overwrite anything the user may have edited:
    /// a file that already exists is reported as an output and left alone.
    fn run(&self, context: &ImportCompanionContext) -> Result<Vec<CompanionOutput>, String>;
}
