//! Out-of-Box Experience (OOBE) - Fancy animated welcome screen
//!
//! A stunning first-run experience inspired by Arc browser's "Meet the internet, again"
//! Features animated gradients, smooth transitions, and ambient sounds

mod gradient;
mod intro_screen;
mod audio;

pub use intro_screen::{IntroScreen, IntroComplete, IntroPhase, has_seen_intro, mark_intro_seen, reset_intro};
pub use gradient::AnimatedGradient;
pub use audio::IntroAudio;
