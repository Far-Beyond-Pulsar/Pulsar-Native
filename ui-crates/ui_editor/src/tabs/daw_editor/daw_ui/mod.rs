/// DAW UI Module
/// Production-quality interface components for the embedded DAW

pub mod state;
pub mod panel;
mod mixer;

// Individual panels - to be implemented one by one
mod timeline;
mod transport;
mod browser;
mod toolbar;
mod track_header;

pub use panel::DawPanel;
