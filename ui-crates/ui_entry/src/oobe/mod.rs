//! Out-of-Box Experience (OOBE) - Fancy animated welcome screen
//!
//! A stunning first-run experience inspired by Arc browser's "Meet the internet, again"
//! Features animated gradients, smooth transitions, and ambient sounds

mod audio;
mod gradient;
mod intro_screen;

pub use audio::IntroAudio;
pub use gradient::AnimatedGradient;
pub use intro_screen::{
    has_seen_intro, mark_intro_seen, reset_intro, IntroComplete, IntroPhase, IntroScreen,
};
