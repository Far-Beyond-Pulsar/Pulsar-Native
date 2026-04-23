//! Asset Loading and Embedding
//!
//! This module provides embedded asset loading using `rust-embed`.
//! Assets are embedded into the binary at compile time for easy distribution.
//!
//! ## Embedded Assets
//!
//! - **Icons**: SVG files in `assets/icons/**/*.svg`
//! - **Fonts**: TrueType fonts in `assets/fonts/**/*.ttf`
//! - **Images**: PNG files in `assets/images/**/*.png`
//!
//! ## Usage
//!
//! ```rust,ignore
//! use crate::assets::Assets;
//!
//! // Load an asset
//! if let Some(font_data) = Assets::get("fonts/JetBrainsMono-Regular.ttf") {
//!     // Use font_data.data
//! }
//!
//! // List assets in a directory
//! let icons = Assets::iter()
//!     .filter(|p| p.starts_with("icons/"))
//!     .collect::<Vec<_>>();
//! ```
//!
//! ## Implementation
//!
//! Uses the `rust-embed` crate to embed assets at compile time.
//! Implements the GPUI `AssetSource` trait for integration with the UI framework.

use anyhow::anyhow;
use gpui::{AssetSource, Result, SharedString};
use rust_embed::RustEmbed;
use std::borrow::Cow;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../assets"]
#[include = "icons/**/*.svg"]
#[include = "fonts/**/*.ttf"]
#[include = "images/**/*.png"]
pub struct Assets;

impl AssetSource for Assets {
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
