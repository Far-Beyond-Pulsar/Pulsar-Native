//! Headless snapshot renderer for Helio.
//!
//! Loads any model format supported by `helio-asset-compat` (FBX, glTF, OBJ, USD),
//! spins up a full Helio render pipeline with an auto-placed camera, renders a
//! single frame offscreen, and returns an [`image::RgbaImage`].
//!
//! No window, event loop, or display server is needed.
//!
//! # Example
//! ```no_run
//! use helio_snapshot::{render_snapshot, SnapshotConfig, ViewDirection};
//!
//! let img = render_snapshot("model.fbx", SnapshotConfig {
//!     width: 1024,
//!     height: 1024,
//!     view: ViewDirection::Isometric,
//!     ..Default::default()
//! }).unwrap();
//! img.save("snapshot.png").unwrap();
//! ```

mod renderer;

pub use renderer::{
    render_preview, render_snapshot, SnapshotBatch, SnapshotConfig, SnapshotError, ViewDirection,
};
