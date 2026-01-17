//! Palette View Delegate
//!
//! This module provides the bridge between the Palette data layer and the
//! GenericPalette UI rendering component.

use gpui::{App, AppContext as _, Entity};

use super::palette_data::{ItemId, Palette, PaletteItemData};
use super::palette_trait::{PaletteDelegate, PaletteItem};

/// Delegate that bridges Palette to GenericPalette rendering
///
/// This adapter implements the PaletteDelegate trait to expose a Palette's
/// items to the GenericPalette UI component. It handles selection tracking
/// and item retrieval.
///
/// The delegate caches the palette's categorized items for efficient rendering.
///
/// # Example
/// ```
/// // Create delegate from a palette entity
/// let delegate = palette_ref.read(cx).create_delegate();
///
/// // Use with GenericPalette
/// let view = cx.new(|cx| GenericPalette::new(delegate, window, cx));
///
/// // After dismissal, extract selected item
/// if let Some(item_id) = delegate_mut().take_selected_item() {
///     palette.update(cx, |p, cx| p.execute_item(item_id, window, cx));
/// }
/// ```
pub struct PaletteViewDelegate {
    palette: Entity<Palette>,
    categories: Vec<(String, Vec<PaletteItemData>)>,
    selected_item_id: Option<ItemId>,
}

impl PaletteViewDelegate {
    /// Create a new delegate from a palette entity
    ///
    /// This caches the palette's items for rendering. Note that you need
    /// app context to read from the entity, so this is typically called
    /// from code that has access to `cx`.
    pub fn new(palette: Entity<Palette>, cx: &App) -> Self {
        let categories = palette.read(cx).categorized_items();
        Self {
            palette,
            categories,
            selected_item_id: None,
        }
    }

    /// Take the selected item ID (if any)
    ///
    /// Returns the ID and clears the internal selection state.
    /// Typically called after the palette is dismissed.
    pub fn take_selected_item(&mut self) -> Option<ItemId> {
        self.selected_item_id.take()
    }

    /// Get a reference to the palette entity
    pub fn palette_entity(&self) -> &Entity<Palette> {
        &self.palette
    }
}

impl PaletteDelegate for PaletteViewDelegate {
    type Item = PaletteItemData;

    fn placeholder(&self) -> &str {
        "Search items..."
    }

    fn categories(&self) -> Vec<(String, Vec<Self::Item>)> {
        // Return cached categories
        self.categories.clone()
    }

    fn confirm(&mut self, item: &Self::Item) {
        // Store the selected item ID
        self.selected_item_id = Some(item.id);
    }

    fn categories_collapsed_by_default(&self) -> bool {
        false
    }

    fn supports_docs(&self) -> bool {
        false
    }
}
