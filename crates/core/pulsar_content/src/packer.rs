//! The content packer: what cooks a project and packs it into a pak set,
//! for the editor's build.
//!
//! Cooking needs every plugin (their record migrations, every scripting
//! language), so it is a host's (`pulsar_package`), and the editor's build
//! runner, a kernel crate, cannot name it. The host registers one
//! [`ContentPacker`]; the runner calls [`packer`]. A build with a pack step
//! and no packer linked fails and says so.

use std::path::Path;

/// One pack: what to cook, where the pak set goes, and how to report.
pub struct PackJob<'a> {
    pub project_root: &'a Path,
    /// The `Content/` folder next to where the game binary is built.
    pub content_dir: &'a Path,
    /// A shipping build (smallest output, the whole set rewritten) or a
    /// development one (fast, incremental).
    pub shipping: bool,
    /// Progress from 0.0 to 1.0, with a short status.
    pub progress: &'a dyn Fn(f32, String),
    pub cancelled: &'a dyn Fn() -> bool,
}

/// Packs a project. Its log lines (target `pack`) are written in the
/// caller's task, so the build's log shows them.
pub struct ContentPacker {
    /// The package that submitted it (`pulsar_plugins::engine_origin!()`).
    pub origin: &'static str,
    /// Returns a one-line summary, or why it failed.
    pub pack: fn(&PackJob<'_>) -> Result<String, String>,
}

impl pulsar_plugins::Contribution for ContentPacker {
    const CATEGORY: &'static str = "content.packer";

    fn origin(&self) -> &'static str {
        self.origin
    }

    fn id(&self) -> String {
        "content_packer".into()
    }

    fn label(&self) -> String {
        "Content packer".into()
    }
}

pulsar_plugins::registration!(ContentPacker);

/// The linked content packer, if the binary has one.
pub fn packer() -> Option<&'static ContentPacker> {
    pulsar_plugins::registered::<ContentPacker>().next()
}
