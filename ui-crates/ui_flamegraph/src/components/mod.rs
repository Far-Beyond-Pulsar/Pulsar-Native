//! UI components for the flamegraph viewer

pub mod flamegraph_canvas;
pub mod framerate_graph;
pub mod hover_popup;
pub mod thread_labels;

pub use framerate_graph::render_framerate_graph;
pub use hover_popup::render_hover_popup;
pub use thread_labels::render_thread_labels;
