//! UI components for the flamegraph viewer

pub mod flamegraph_canvas;
pub mod flamegraph_spans;
pub mod framerate_graph;
pub mod hover_popup;
pub mod statistics_sidebar;
pub mod thread_labels;
pub mod timeline_ruler;

pub use flamegraph_canvas::render_flamegraph_canvas;
pub use flamegraph_spans::render_flamegraph_spans;
pub use framerate_graph::render_framerate_graph;
pub use hover_popup::render_hover_popup;
pub use statistics_sidebar::render_statistics_sidebar;
pub use thread_labels::render_thread_labels;
pub use timeline_ruler::render_timeline_ruler;
