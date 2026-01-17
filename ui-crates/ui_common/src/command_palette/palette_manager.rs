//! Palette Manager - Global Registry for Command Palettes
//!
//! This module provides the central registry for managing command palette lifecycles.
//! Palettes can be registered, retrieved, and unregistered by ID.

use gpui::{App, AppContext as _, Entity, Global, WeakEntity, Window};
use std::collections::HashMap;

use super::palette_data::Palette;

/// Unique identifier for a palette in the global registry.
///
/// Used to retrieve palettes after registration via `PaletteManager::get_palette()`.
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq)]
pub struct PaletteId(usize);

/// Entry in the palette registry
struct PaletteEntry {
    id: PaletteId,
    name: String,
    entity: WeakEntity<Palette>,
}

/// Global palette registry - manages all registered palettes
///
/// Palettes can be registered with a name and ID, items can be added dynamically,
/// and the registry provides ID-based lookup for palette management.
///
/// # Example
/// ```
/// // Initialize the global registry
/// PaletteManager::init(cx);
///
/// // Register a palette
/// let (palette_id, palette_ref) = PaletteManager::register_palette("my_palette", window, cx);
///
/// // Add items to the palette
/// palette_ref.update(cx, |palette, cx| {
///     palette.add_item("Action", "Description", IconName::Star, "Category", |w, cx| { }, cx);
/// });
///
/// // Retrieve palette later by ID
/// if let Some(palette) = PaletteManager::get_palette(palette_id, cx) {
///     // Use the palette
/// }
/// ```
pub struct PaletteManager {
    palettes: HashMap<PaletteId, PaletteEntry>,
    next_id: usize,
}

impl Global for PaletteManager {}

impl PaletteManager {
    /// Initialize the palette manager as a global
    ///
    /// This should be called once during application initialization.
    /// If already initialized, this is a no-op.
    pub fn init(cx: &mut App) {
        if cx.try_global::<PaletteManager>().is_none() {
            cx.set_global(PaletteManager::new());
        }
    }

    /// Create a new palette manager
    fn new() -> Self {
        Self {
            palettes: HashMap::new(),
            next_id: 0,
        }
    }

    /// Access the global palette manager (immutable)
    pub fn global(cx: &App) -> &Self {
        cx.global::<PaletteManager>()
    }

    /// Access the global palette manager (mutable)
    pub fn global_mut(cx: &mut App) -> &mut Self {
        cx.global_mut::<PaletteManager>()
    }

    /// Register a new palette and get back its ID and Entity reference
    ///
    /// # Arguments
    /// * `name` - Display name for the palette
    /// * `window` - Window context for entity creation
    /// * `cx` - Application context
    ///
    /// # Returns
    /// A tuple of (PaletteId, Entity<Palette>) for managing the palette
    pub fn register_palette(
        name: impl Into<String>,
        window: &mut Window,
        cx: &mut App,
    ) -> (PaletteId, Entity<Palette>) {
        let id = PaletteId(Self::global(cx).next_id);
        let name = name.into();

        let palette_entity = cx.new(|cx| Palette::new(id, name.clone(), window, cx));

        Self::global_mut(cx).palettes.insert(
            id,
            PaletteEntry {
                id,
                name,
                entity: palette_entity.downgrade(),
            },
        );
        Self::global_mut(cx).next_id += 1;

        (id, palette_entity)
    }

    /// Get a palette by its ID
    ///
    /// Returns None if the palette doesn't exist or has been deallocated.
    pub fn get_palette(id: PaletteId, cx: &App) -> Option<Entity<Palette>> {
        Self::global(cx)
            .palettes
            .get(&id)
            .and_then(|entry| entry.entity.upgrade())
    }

    /// Unregister a palette by ID
    ///
    /// Removes the palette from the registry. The palette entity will be
    /// deallocated when all references are dropped.
    pub fn unregister_palette(id: PaletteId, cx: &mut App) {
        Self::global_mut(cx).palettes.remove(&id);
    }

    /// Get all registered palette IDs
    pub fn palette_ids(cx: &App) -> Vec<PaletteId> {
        Self::global(cx).palettes.keys().copied().collect()
    }

    /// Get the name of a palette by ID
    pub fn palette_name(id: PaletteId, cx: &App) -> Option<String> {
        Self::global(cx)
            .palettes
            .get(&id)
            .map(|entry| entry.name.clone())
    }
}
