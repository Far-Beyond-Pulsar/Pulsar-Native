//! UI components for the flamegraph viewer

pub mod framerate_graph;
pub mod timeline_ruler;
pub mod statistics_sidebar;
pub mod hover_popup;
pub mod thread_labels;
pub mod flamegraph_canvas;

pub use framerate_graph::render_framerate_graph;
pub use timeline_ruler::render_timeline_ruler;
pub use statistics_sidebar::render_statistics_sidebar;
pub use hover_popup::render_hover_popup;
pub use thread_labels::render_thread_labels;
pub use flamegraph_canvas::render_flamegraph_canvas;
