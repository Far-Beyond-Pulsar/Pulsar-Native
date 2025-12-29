//! OOBE Intro Screen
//!
//! A stunning animated welcome screen for first-time users
//! Features: Animated gradient background, smooth text transitions, continue button

use gpui::*;
use std::time::{Duration, Instant};
use ui::{h_flex, v_flex, ActiveTheme, IconName, StyledExt};

use super::gradient::AnimatedGradient;
use super::audio::IntroAudio;

/// The current phase of the intro sequence
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum IntroPhase {
    /// Initial fade-in of background
    FadeIn = 0,
    /// Main title appears with animation
    TitleReveal = 1,
    /// Subtitle fades in
    SubtitleReveal = 2,
    /// Button appears, waiting for user
    Ready = 3,
    /// User clicked continue, fading out
    FadeOut = 4,
    /// Intro complete
    Complete = 5,
}

/// Event emitted when the intro is complete
pub struct IntroComplete;

/// The main OOBE intro screen component
pub struct IntroScreen {
    phase: IntroPhase,
    start_time: Instant,
    phase_start_time: Instant,
    gradient: AnimatedGradient,
    audio: IntroAudio,
    /// Main headline text
    headline: String,
    /// Subtitle text
    subtitle: String,
    /// Button text
    button_text: String,
    /// Whether the user has interacted
    user_interacted: bool,
    /// Animation frame counter for smooth updates
    frame_count: u64,
}

impl EventEmitter<IntroComplete> for IntroScreen {}

impl IntroScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        tracing::info!("🎬 [IntroScreen::new] Creating new IntroScreen instance");
        
        let audio = IntroAudio::new();
        audio.play_ambient();

        let screen = Self {
            phase: IntroPhase::FadeIn,
            start_time: Instant::now(),
            phase_start_time: Instant::now(),
            gradient: AnimatedGradient::arc_style(),
            audio,
            headline: "Welcome to Pulsar".to_string(),
            subtitle: "Create worlds. Tell stories. Build dreams.".to_string(),
            button_text: "Get Started".to_string(),
            user_interacted: false,
            frame_count: 0,
        };

        // Start the animation loop
        tracing::info!("🎬 [IntroScreen] Starting animation loop");
        cx.spawn(async move |this, mut cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(16)).await; // ~60fps
                
                let should_continue = cx.update(|cx| {
                    this.update(cx, |screen, cx| {
                        screen.tick(cx);
                        screen.phase != IntroPhase::Complete
                    }).unwrap_or(false)
                }).unwrap_or(false);

                if !should_continue {
                    tracing::info!("🎬 [IntroScreen] Animation loop complete");
                    break;
                }
            }
        }).detach();

        screen
    }

    /// Create with custom text
    pub fn with_text(
        headline: impl Into<String>,
        subtitle: impl Into<String>,
        button_text: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut screen = Self::new(window, cx);
        screen.headline = headline.into();
        screen.subtitle = subtitle.into();
        screen.button_text = button_text.into();
        screen
    }

    /// Update animation state each frame
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.frame_count += 1;
        let phase_elapsed = self.phase_start_time.elapsed();

        // Auto-advance phases based on timing
        match self.phase {
            IntroPhase::FadeIn => {
                if phase_elapsed > Duration::from_millis(800) {
                    self.advance_phase(IntroPhase::TitleReveal, cx);
                }
            }
            IntroPhase::TitleReveal => {
                if phase_elapsed > Duration::from_millis(1000) {
                    self.advance_phase(IntroPhase::SubtitleReveal, cx);
                }
            }
            IntroPhase::SubtitleReveal => {
                if phase_elapsed > Duration::from_millis(800) {
                    self.advance_phase(IntroPhase::Ready, cx);
                }
            }
            IntroPhase::FadeOut => {
                if phase_elapsed > Duration::from_millis(500) {
                    self.advance_phase(IntroPhase::Complete, cx);
                    cx.emit(IntroComplete);
                }
            }
            _ => {}
        }

        cx.notify();
    }

    fn advance_phase(&mut self, new_phase: IntroPhase, _cx: &mut Context<Self>) {
        self.phase = new_phase;
        self.phase_start_time = Instant::now();
        
        // Play transition sound
        if new_phase == IntroPhase::TitleReveal || new_phase == IntroPhase::Ready {
            self.audio.play_transition();
        }
    }

    fn on_continue(&mut self, cx: &mut Context<Self>) {
        if self.phase == IntroPhase::Ready && !self.user_interacted {
            self.user_interacted = true;
            self.audio.play_click();
            self.audio.play_complete();
            self.advance_phase(IntroPhase::FadeOut, cx);
        }
    }

    /// Calculate opacity for fade effects
    fn calculate_opacity(&self, phase: IntroPhase) -> f32 {
        let elapsed = self.phase_start_time.elapsed().as_secs_f32();
        
        match self.phase {
            IntroPhase::FadeIn => {
                // Everything fades in together
                (elapsed / 0.8).min(1.0)
            }
            IntroPhase::FadeOut => {
                // Everything fades out
                1.0 - (elapsed / 0.5).min(1.0)
            }
            _ => {
                // Element-specific reveal
                match phase {
                    IntroPhase::TitleReveal => {
                        if self.phase >= IntroPhase::TitleReveal {
                            let title_elapsed = if self.phase == IntroPhase::TitleReveal {
                                elapsed
                            } else {
                                1.0
                            };
                            (title_elapsed / 0.6).min(1.0)
                        } else {
                            0.0
                        }
                    }
                    IntroPhase::SubtitleReveal => {
                        if self.phase >= IntroPhase::SubtitleReveal {
                            let sub_elapsed = if self.phase == IntroPhase::SubtitleReveal {
                                elapsed
                            } else {
                                1.0
                            };
                            (sub_elapsed / 0.5).min(1.0)
                        } else {
                            0.0
                        }
                    }
                    IntroPhase::Ready => {
                        if self.phase >= IntroPhase::Ready {
                            let ready_elapsed = if self.phase == IntroPhase::Ready {
                                elapsed
                            } else {
                                1.0
                            };
                            (ready_elapsed / 0.4).min(1.0)
                        } else {
                            0.0
                        }
                    }
                    _ => 1.0,
                }
            }
        }
    }

    /// Calculate Y offset for slide-up animation
    fn calculate_slide_offset(&self, phase: IntroPhase) -> f32 {
        let opacity = self.calculate_opacity(phase);
        // Slide up from 30px to 0px as opacity goes from 0 to 1
        30.0 * (1.0 - opacity)
    }
}

impl Render for IntroScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (color1, color2, color3) = self.gradient.gradient_colors();
        
        let overall_opacity = match self.phase {
            IntroPhase::FadeIn => (self.phase_start_time.elapsed().as_secs_f32() / 0.8).min(1.0),
            IntroPhase::FadeOut => 1.0 - (self.phase_start_time.elapsed().as_secs_f32() / 0.5).min(1.0),
            _ => 1.0,
        };

        let title_opacity = self.calculate_opacity(IntroPhase::TitleReveal) * overall_opacity;
        let subtitle_opacity = self.calculate_opacity(IntroPhase::SubtitleReveal) * overall_opacity;
        let button_opacity = self.calculate_opacity(IntroPhase::Ready) * overall_opacity;

        let title_offset = self.calculate_slide_offset(IntroPhase::TitleReveal);
        let subtitle_offset = self.calculate_slide_offset(IntroPhase::SubtitleReveal);
        let button_offset = self.calculate_slide_offset(IntroPhase::Ready);

        // Animated background gradient
        let bg_primary = hsla(color1.h, color1.s, color1.l, overall_opacity);
        let bg_secondary = hsla(color2.h, color2.s, color2.l, overall_opacity * 0.8);
        let bg_tertiary = hsla(color3.h, color3.s, color3.l, overall_opacity * 0.6);

        div()
            .id("oobe-intro")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(bg_primary)
            // Overlay darker gradient at bottom for depth
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(bg_secondary)
                    .opacity(0.7)
            )
            // Glow effect from top-right
            .child(
                div()
                    .absolute()
                    .top(px(-100.0))
                    .right(px(-100.0))
                    .w(px(600.0))
                    .h(px(600.0))
                    .rounded_full()
                    .bg(hsla(color1.h, 0.9, 0.6, overall_opacity * 0.3))
            )
            // Glow effect from bottom-left
            .child(
                div()
                    .absolute()
                    .bottom(px(-150.0))
                    .left(px(-150.0))
                    .w(px(500.0))
                    .h(px(500.0))
                    .rounded_full()
                    .bg(hsla(color3.h, 0.85, 0.5, overall_opacity * 0.25))
            )
            // Content container
            .child(
                v_flex()
                    .items_center()
                    .gap_6()
                    // Logo/Icon (optional)
                    .child(
                        div()
                            .opacity(title_opacity)
                            .child(
                                div()
                                    .w(px(80.0))
                                    .h(px(80.0))
                                    .rounded_full()
                                    .bg(white().opacity(0.1))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        ui::Icon::new(IconName::BrightStar)
                                            .size(px(40.0))
                                            .text_color(white())
                                    )
                            )
                    )
                    // Main headline
                    .child(
                        div()
                            .opacity(title_opacity)
                            .mt(px(title_offset))
                            .child(
                                div()
                                    .text_size(px(48.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(white().opacity(0.95))
                                    .child(self.headline.clone())
                            )
                    )
                    // Subtitle
                    .child(
                        div()
                            .opacity(subtitle_opacity)
                            .mt(px(subtitle_offset))
                            .child(
                                div()
                                    .text_xl()
                                    .text_color(white().opacity(0.8))
                                    .child(self.subtitle.clone())
                            )
                    )
                    // Continue button
                    .child(
                        div()
                            .opacity(button_opacity)
                            .mt(px(button_offset + 20.0))
                            .child(
                                div()
                                    .id("continue-btn")
                                    .px_6()
                                    .py_3()
                                    .rounded_full()
                                    .bg(white().opacity(0.15))
                                    .border_1()
                                    .border_color(white().opacity(0.3))
                                    .hover(|s| s.bg(white().opacity(0.25)).cursor_pointer())
                                    .active(|s| s.bg(white().opacity(0.1)))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .text_base()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(white())
                                                    .child(self.button_text.clone())
                                            )
                                            .child(
                                                ui::Icon::new(IconName::ArrowRight)
                                                    .size_4()
                                                    .text_color(white())
                                            )
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.on_continue(cx);
                                    }))
                            )
                    )
            )
            // Skip button in corner
            .child(
                div()
                    .absolute()
                    .top_4()
                    .right_4()
                    .opacity(button_opacity * 0.6)
                    .child(
                        div()
                            .id("skip-btn")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .text_sm()
                            .text_color(white().opacity(0.6))
                            .hover(|s| s.text_color(white().opacity(0.9)).cursor_pointer())
                            .child("Skip")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.on_continue(cx);
                            }))
                    )
            )
    }
}

/// Check if the user has seen the intro before
/// Returns false if --OOBE flag is passed (forces OOBE to show)
pub fn has_seen_intro() -> bool {
    // Check for --OOBE flag to force OOBE display
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--OOBE" || arg == "--oobe") {
        tracing::info!("🎯 [OOBE] --OOBE flag detected, forcing OOBE display");
        return false;
    }
    
    let prefs_path = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
        .map(|proj| proj.data_dir().join("oobe_complete"))
        .unwrap_or_else(|| std::path::PathBuf::from("oobe_complete"));
    
    prefs_path.exists()
}

/// Mark the intro as seen
pub fn mark_intro_seen() {
    if let Some(proj_dirs) = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine") {
        let data_dir = proj_dirs.data_dir();
        let _ = std::fs::create_dir_all(data_dir);
        let prefs_path = data_dir.join("oobe_complete");
        let _ = std::fs::write(prefs_path, "1");
    }
}

/// Reset the intro (for testing)
pub fn reset_intro() {
    if let Some(proj_dirs) = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine") {
        let prefs_path = proj_dirs.data_dir().join("oobe_complete");
        let _ = std::fs::remove_file(prefs_path);
    }
}
