//! Build configurations for the editor: the Build button's picker, the
//! configurator window, and the runner that executes a configuration.
//!
//! The data model lives in `engine_state::build_config`; this crate is
//! everything that shows it and acts on it.
//!
//! - [`runner`]: a configuration becomes a [`runner::plan::Plan`] of cargo
//!   commands, which a worker thread runs with live progress and cancellation.
//! - [`picker`]: the searchable dropdown behind the Build button.
//! - [`button`]: the Build split button the global toolbar shows.
//! - [`configurator`]: the window for creating and editing configurations.

pub mod button;
pub mod configurator;
pub mod picker;
pub mod runner;

pub use button::build_button;
pub use configurator::BuildConfiguratorWindow;
pub use picker::BuildPicker;
pub use runner::{cancel_build, run_configuration};

/// Load the project's build configurations. Call before showing the Build
/// button; safe to repeat.
pub fn init() {
    engine_state::build_config::ensure_loaded();
    std::hint::black_box(configurator::link_anchor());
}
