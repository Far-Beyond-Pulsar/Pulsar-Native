//! The `logicv2` experimental nodes, defined in `logicexec`.

// Links to nodes //
// Private like `experimental::nodes`: a public `nodes` here would reach the
// crate root through the glob re-exports and clash with `engine::nodes`.
mod nodes;
pub use nodes::*;
// Link end //
