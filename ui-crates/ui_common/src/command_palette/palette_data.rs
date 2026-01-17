//! Palette Data Structures
//!
//! This module contains the core data structures for palettes and palette items.
//! Palettes hold items with rebindable callbacks that can be dynamically managed.

use gpui::{App, Context, Window};
use std::collections::HashMap;
use std::sync::Arc;
use ui::IconName;

use super::palette_manager::PaletteId;
use super::palette_trait::PaletteItem;

/// Unique identifier for an item within a palette.
///
/// Used to reference specific items for rebinding callbacks or removal.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct ItemId(usize);

/// Internal storage for a palette item
///
/// Contains all display metadata plus a rebindable callback function
#[derive(Clone)]
pub struct PaletteItemData {
    pub id: ItemId,
    pub name: String,
    pub description: String,
    pub icon: IconName,
    pub keywords: Vec<String>,
    pub category: String,
    pub callback: Arc<dyn Fn(&mut Window, &mut App) + Send + Sync>,
}

impl PaletteItem for PaletteItemData {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn icon(&self) -> IconName {
        self.icon.clone()
    }

    fn keywords(&self) -> Vec<&str> {
        self.keywords.iter().map(|s| s.as_str()).collect()
    }

    fn documentation(&self) -> Option<String> {
        None
    }
}

/// A palette contains dynamically registered items with rebindable callbacks
///
/// This is the data container - rendering is handled by GenericPalette via PaletteViewDelegate
///
/// # Example
/// ```
/// // Palette is created via PaletteManager::register_palette
/// palette_ref.update(cx, |palette, cx| {
///     // Add an item
///     let item_id = palette.add_item(
///         "My Action",
///         "Description",
///         IconName::Star,
///         "Category",
///         |window, cx| {
///             // Callback implementation
///         },
///         cx,
///     );
///
///     // Rebind the callback later
///     palette.rebind_callback(
///         item_id,
///         |window, cx| {
///             // New implementation
///         },
///         cx,
///     ).ok();
/// });
/// ```
pub struct Palette {
    id: PaletteId,
    name: String,
    items: HashMap<ItemId, PaletteItemData>,
    next_item_id: usize,
}

impl Palette {
    /// Create a new empty palette
    ///
    /// Note: Typically created via PaletteManager::register_palette
    pub fn new(id: PaletteId, name: String, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            id,
            name,
            items: HashMap::new(),
            next_item_id: 0,
        }
    }

    /// Add an item to the palette
    ///
    /// # Arguments
    /// * `name` - Display name of the item
    /// * `description` - Description/subtitle text
    /// * `icon` - Icon to display
    /// * `category` - Category name for grouping
    /// * `callback` - Function to execute when item is selected
    ///
    /// # Returns
    /// ItemId for managing this item (e.g., for rebinding or removal)
    pub fn add_item<F>(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        icon: IconName,
        category: impl Into<String>,
        callback: F,
        _cx: &mut Context<Self>,
    ) -> ItemId
    where
        F: Fn(&mut Window, &mut App) + Send + Sync + 'static,
    {
        let id = ItemId(self.next_item_id);
        self.next_item_id += 1;

        self.items.insert(
            id,
            PaletteItemData {
                id,
                name: name.into(),
                description: description.into(),
                icon,
                keywords: vec![],
                category: category.into(),
                callback: Arc::new(callback),
            },
        );

        id
    }

    /// Add an item with keywords for better search matching
    ///
    /// Keywords are additional search terms beyond name and description
    pub fn add_item_with_keywords<F>(
        &mut self,
        name: impl Into<String>,
        description: impl Into<String>,
        icon: IconName,
        category: impl Into<String>,
        keywords: Vec<String>,
        callback: F,
        _cx: &mut Context<Self>,
    ) -> ItemId
    where
        F: Fn(&mut Window, &mut App) + Send + Sync + 'static,
    {
        let id = ItemId(self.next_item_id);
        self.next_item_id += 1;

        self.items.insert(
            id,
            PaletteItemData {
                id,
                name: name.into(),
                description: description.into(),
                icon,
                keywords,
                category: category.into(),
                callback: Arc::new(callback),
            },
        );

        id
    }

    /// Remove an item from the palette
    ///
    /// Returns Some(()) if the item existed, None otherwise
    pub fn remove_item(&mut self, item_id: ItemId, _cx: &mut Context<Self>) -> Option<()> {
        self.items.remove(&item_id).map(|_| ())
    }

    /// Rebind a callback for an item
    ///
    /// Replaces the existing callback with a new one. Useful for dynamic behavior changes.
    ///
    /// # Returns
    /// Ok(()) if successful, Err with message if item not found
    pub fn rebind_callback<F>(
        &mut self,
        item_id: ItemId,
        callback: F,
        _cx: &mut Context<Self>,
    ) -> Result<(), String>
    where
        F: Fn(&mut Window, &mut App) + Send + Sync + 'static,
    {
        if let Some(item) = self.items.get_mut(&item_id) {
            item.callback = Arc::new(callback);
            Ok(())
        } else {
            Err(format!("Item {:?} not found in palette", item_id))
        }
    }

    /// Get all items (for internal use by delegate)
    pub fn items(&self) -> &HashMap<ItemId, PaletteItemData> {
        &self.items
    }

    /// Get categories with their items, sorted by category name
    ///
    /// Returns a Vec of (category_name, items) tuples for rendering
    pub fn categorized_items(&self) -> Vec<(String, Vec<PaletteItemData>)> {
        let mut categories: HashMap<String, Vec<PaletteItemData>> = HashMap::new();

        for item in self.items.values() {
            categories
                .entry(item.category.clone())
                .or_insert_with(Vec::new)
                .push(item.clone());
        }

        let mut result: Vec<_> = categories.into_iter().collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    }

    /// Execute an item's callback
    ///
    /// # Returns
    /// Ok(()) if successful, Err with message if item not found
    pub fn execute_item(
        &self,
        item_id: ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<(), String> {
        if let Some(item) = self.items.get(&item_id) {
            (item.callback)(window, cx);
            Ok(())
        } else {
            Err(format!("Item {:?} not found", item_id))
        }
    }

    /// Get the palette's ID
    pub fn palette_id(&self) -> PaletteId {
        self.id
    }

    /// Get the palette's name
    pub fn name(&self) -> &str {
        &self.name
    }
}
