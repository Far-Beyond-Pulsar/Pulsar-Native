//! Asset Loading and Embedding
//!
//! This module provides embedded asset loading using `rust-embed`.
//! Assets are embedded into the binary at compile time for easy distribution.
//!
//! ## Embedded Assets
//!
//! - **Icons**: SVG files from WGPUI-Component `assets/icons/**/*.svg`
//! - **Fonts**: TrueType fonts in `assets/fonts/**/*.ttf`
//! - **Images**: PNG files in `assets/images/**/*.png`
//! - **Meshes**: All files in `assets/meshes/**`
//!
//! Icons are loaded from the WGPUI-Component crate, while other assets come from the engine.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::assets::Assets;
//!
//! // Assets automatically delegates to the correct source
//! // Icons come from ui::assets::Assets
//! // Everything else comes from engine EngineAssets
//! if let Some(icon) = Assets::get("icons/folder.svg") {
//!     // Use icon data
//! }
//! ```
//!
//! ## Implementation
//!
//! Uses the `rust-embed` crate to embed assets at compile time.
//! Implements the GPUI `AssetSource` trait for integration with the UI framework.

use anyhow::anyhow;
use gpui::{AssetSource, Result, SharedString};
use rust_embed::{EmbeddedFile, RustEmbed};
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../assets"]
#[include = "fonts/**/*.ttf"]
#[include = "images/**/*.png"]
#[include = "meshes/**"]
#[include = "default.level"]
pub struct EngineAssets;

/// Combined asset source that delegates to UI assets for icons and engine assets for everything else
pub struct CombinedAssets;

impl CombinedAssets {
    /// Get a single asset by path (RustEmbed-compatible method)
    pub fn get(path: &str) -> Option<EmbeddedFile> {
        if path.starts_with("icons/") {
            <ui::assets::Assets as RustEmbed>::get(path)
        } else {
            <EngineAssets as RustEmbed>::get(path)
        }
    }

    /// Iterate over all assets (RustEmbed-compatible method)
    pub fn iter() -> impl Iterator<Item = Cow<'static, str>> {
        // Combine iterators from both sources
        <ui::assets::Assets as RustEmbed>::iter()
            .filter(|p| p.starts_with("icons/"))
            .chain(<EngineAssets as RustEmbed>::iter())
    }
}

impl AssetSource for CombinedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.is_empty() {
            return Ok(None);
        }

        Self::get(path)
            .map(|f| Some(f.data))
            .ok_or_else(|| anyhow!("could not find asset at path \"{path}\""))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter_map(|p| p.starts_with(path).then(|| p.into()))
            .collect())
    }
}

// Re-export for backward compatibility
pub use CombinedAssets as Assets;
