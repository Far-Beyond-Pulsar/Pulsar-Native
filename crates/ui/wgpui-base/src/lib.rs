//! Compatibility foundation for the upstream GPUI component migration.
#![allow(missing_docs)]

pub mod actions;
pub mod animation;
pub mod component_traits;
pub mod theme_tokens;

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
