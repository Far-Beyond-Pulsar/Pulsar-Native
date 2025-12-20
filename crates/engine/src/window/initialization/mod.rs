//! Window and rendering initialization module
//!
//! This module handles initialization of GPUI windows, D3D11 rendering pipelines,
//! and window-type-specific content creation.

pub mod gpui;
pub mod d3d11;
pub mod window_content;

pub use gpui::initialize_gpui_window;
pub use d3d11::initialize_d3d11_pipeline;
pub use window_content::create_window_content;
