//! Compatibility foundation for the upstream GPUI component migration.
#![allow(missing_docs)]

pub mod actions;
pub mod animation;
pub mod component_traits;
pub mod theme_tokens;

// The editor core is enabled independently from the still-in-progress full
// upstream component migration. This is the architectural replacement for the
// legacy editor embedded in `ui`: buffer state, display mapping, folding,
// viewport layout, decorations, and language-service coordination live here.
#[cfg(feature = "editor-core")]
pub mod auto_scroll;
#[cfg(feature = "editor-core")]
pub mod button;
#[cfg(feature = "editor-core")]
pub mod geometry;
#[cfg(feature = "editor-core")]
pub mod global_state;
#[cfg(feature = "editor-core")]
pub mod input;
#[cfg(feature = "editor-core")]
pub mod number_input;
#[cfg(feature = "editor-core")]
pub mod observe;
#[cfg(feature = "editor-core")]
pub mod scrollbar;
#[cfg(feature = "editor-core")]
pub mod state_style;
#[cfg(feature = "editor-core")]
pub mod styled;
#[cfg(feature = "editor-core")]
pub mod text_boundary;
#[cfg(feature = "editor-core")]
pub mod theme;
#[cfg(feature = "editor-core")]
pub mod touch_selection;

#[cfg(feature = "editor-core")]
pub use auto_scroll::*;
#[cfg(feature = "editor-core")]
pub use button::*;
#[cfg(feature = "editor-core")]
pub use component_traits::*;
#[cfg(feature = "editor-core")]
pub use geometry::*;
#[cfg(feature = "editor-core")]
pub use global_state::*;
#[cfg(feature = "editor-core")]
pub use input::InputBase;
#[cfg(feature = "editor-core")]
pub use number_input::StepAction;
#[cfg(feature = "editor-core")]
pub use observe::*;
#[cfg(feature = "editor-core")]
pub use scrollbar::*;
#[cfg(feature = "editor-core")]
pub use state_style::*;
#[cfg(feature = "editor-core")]
pub use styled::*;
#[cfg(feature = "editor-core")]
pub use theme_tokens::SemanticThemeTokens;
#[cfg(feature = "editor-core")]
pub use theme::*;
#[cfg(feature = "editor-core")]
pub use touch_selection::*;

/// Lightweight transition compatibility used by scrolling components.
pub mod motion {
    use gpui::{App, Window};
    use std::time::Duration;
    #[derive(Clone, Copy, Debug)]
    pub struct Transition(pub Duration);
    impl Transition {
        pub fn new(duration: Duration) -> Self {
            Self(duration)
        }
    }
    pub fn transition(
        _id: impl Into<gpui::ElementId>,
        target: f32,
        _transition: Transition,
        _window: &mut Window,
        _cx: &mut App,
    ) -> f32 {
        target
    }
}

/// Opt-in marker for the complete upstream implementation while it is adapted.
#[cfg(feature = "upstream-full")]
pub mod upstream_full {
    include!("upstream_full.rs");
}
