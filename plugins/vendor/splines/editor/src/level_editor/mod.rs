//! Splines in the level editor, registered through its extension points
//! (`ui_level_editor::extend`): the Spline tool mode and its panel
//! ([`mode`], [`panel`]), the tool following selections made elsewhere, and
//! the assistant's spline tools ([`ai_tools`]). Curves are the splines
//! plugin's `SplineComponent`s, edited through undoable commands
//! ([`splines`]).

pub mod ai_tools;
pub mod mode;
pub mod panel;
pub mod preview;
pub mod splines;
pub mod state;

use ui_level_editor::ToolModeId;
use ui_level_editor::extend::{SelectionFollowUp, ToolModeRegistration};

pub use mode::SplineMode;
pub use state::{SplineDomain, SplineStateExt};

/// The Spline mode's id.
pub const SPLINE_MODE: ToolModeId = ToolModeId("spline");

inventory::submit! {
    ToolModeRegistration {
        origin: pulsar_plugins::origin!(),
        order: 10,
        make: || Box::new(SplineMode::default()),
    }
}

inventory::submit! {
    SelectionFollowUp {
        origin: pulsar_plugins::origin!(),
        follow: splines::sync_selection,
    }
}
