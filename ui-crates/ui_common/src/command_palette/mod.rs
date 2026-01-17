//! Command Palette System
//!
//! A dynamic, extensible command palette system for Pulsar.
//!
//! ## Architecture
//!
//! The system consists of three main layers:
//!
//! 1. **PaletteManager** - Global registry that manages palette lifecycles
//! 2. **Palette** - Data container holding items with rebindable callbacks
//! 3. **GenericPalette** - UI rendering component (via PaletteViewDelegate)
//!
//! ## Usage Example
//!
//! ```rust
//! // Initialize the manager (once at app startup)
//! PaletteManager::init(cx);
//!
//! // Register a new palette
//! let (palette_id, palette_ref) = PaletteManager::register_palette("my_palette", window, cx);
//!
//! // Add items with callbacks
//! palette_ref.update(cx, |palette, cx| {
//!     let item_id = palette.add_item(
//!         "My Command",
//!         "Description of the command",
//!         IconName::Star,
//!         "Category",
//!         |window, cx| {
//!             // Execute action
//!             window.dispatch_action(Box::new(MyAction), cx);
//!         },
//!         cx,
//!     );
//! });
//!
//! // Show the palette
//! let delegate = PaletteViewDelegate::new(palette_ref.clone(), cx);
//! let view = cx.new(|cx| GenericPalette::new(delegate, window, cx));
//!
//! // Rebind a callback later
//! palette_ref.update(cx, |palette, cx| {
//!     palette.rebind_callback(item_id, |window, cx| {
//!         // New implementation
//!     }, cx)
//! });
//! ```

// Core palette traits
mod palette_trait;

// UI rendering component
pub mod generic_palette;

// New dynamic palette system
pub mod palette_manager;
pub mod palette_data;
pub mod palette_delegate;

// Public API exports
pub use palette_trait::{PaletteDelegate, PaletteItem};
pub use generic_palette::GenericPalette;
pub use palette_manager::{PaletteManager, PaletteId};
pub use palette_data::{Palette, ItemId, PaletteItemData};
pub use palette_delegate::PaletteViewDelegate;

