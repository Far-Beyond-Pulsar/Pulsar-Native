//! OOBE Intro Screen
//!
//! A stunning animated welcome screen for first-time users
//! Features: Animated gradient background, smooth text transitions, continue button

use gpui::*;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use ui::{h_flex, v_flex, ActiveTheme, IconName, StyledExt};

use super::gradient::AnimatedGradient;
use super::audio::IntroAudio;

/// Guard to prevent multiple animation loops
static INTRO_SCREEN_CREATED: AtomicBool = AtomicBool::new(false);

/// Counter to track how many IntroScreen instances have been created
static INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Shared animation state - all instances read from this
static SHARED_ANIM_STATE: Mutex<Option<SharedAnimState>> = Mutex::new(None);

/// Shared animation state that persists across all IntroScreen instances
struct SharedAnimState {
    phase: IntroPhase,
    start_time: Instant,
    phase_start_time: Instant,
    user_interacted: bool,
}

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
    gradient: AnimatedGradient,
    audio: IntroAudio,
    /// Main headline text
    headline: String,
    /// Subtitle text
    subtitle: String,
    /// Button text
    button_text: String,
    /// Animation frame counter for smooth updates
    frame_count: u64,
}

impl EventEmitter<IntroComplete> for IntroScreen {}

impl IntroScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instance_id = INSTANCE_COUNTER.fetch_add(1, Ordering::SeqCst);
        tracing::info!("🎬 [IntroScreen] Instance #{} created", instance_id);
        
        // Initialize shared animation state (first call wins)
        let already_created = INTRO_SCREEN_CREATED.swap(true, Ordering::SeqCst);
        
        {
            let mut state = SHARED_ANIM_STATE.lock();
            if state.is_none() {
                tracing::info!("🎬 [IntroScreen::new] Initializing shared animation state");
                let now = Instant::now();
                *state = Some(SharedAnimState {
                    phase: IntroPhase::FadeIn,
                    start_time: now,
                    phase_start_time: now,
                    user_interacted: false,
                });
            } else {
                tracing::info!("🎬 [IntroScreen] Instance #{} reusing existing shared state at phase {:?}", 
                    instance_id, state.as_ref().map(|s| s.phase));
            }
        }
        
        if already_created {
            tracing::warn!("🎬 [IntroScreen::new] IntroScreen already exists, using shared state");
        } else {
            tracing::info!("🎬 [IntroScreen::new] Creating new IntroScreen instance (first time)");
        }
        
        let audio = IntroAudio::new();
        if !already_created {
            audio.play_ambient();
        }

        let screen = Self {
            gradient: AnimatedGradient::arc_style(),
            audio,
            headline: "Welcome to Pulsar".to_string(),
            subtitle: "Create worlds. Tell stories. Build dreams.".to_string(),
            button_text: "Get Started".to_string(),
            frame_count: 0,
        };

        // Start the animation loop only on first creation
        if !already_created {
            tracing::info!("🎬 [IntroScreen] Starting animation loop");
            cx.spawn(async move |this, mut cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(16)).await; // ~60fps
                    
                    let should_continue = cx.update(|cx| {
                        this.update(cx, |screen, cx| {
                            screen.tick(cx);
                            let phase = SHARED_ANIM_STATE.lock().as_ref().map(|s| s.phase).unwrap_or(IntroPhase::Complete);
                            phase != IntroPhase::Complete
                        }).unwrap_or(false)
                    }).unwrap_or(false);

                    if !should_continue {
                        tracing::info!("🎬 [IntroScreen] Animation loop complete");
                        // DON'T reset the flags - keep state at Complete so no new animations start
                        // The state will be cleaned up when the process exits
                        break;
                    }
                }
            }).detach();
        }

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

    /// Get current phase from shared state
    fn get_phase(&self) -> IntroPhase {
        SHARED_ANIM_STATE.lock().as_ref().map(|s| s.phase).unwrap_or(IntroPhase::Complete)
    }
    
    /// Get phase elapsed time from shared state
    fn get_phase_elapsed(&self) -> Duration {
        SHARED_ANIM_STATE.lock().as_ref().map(|s| s.phase_start_time.elapsed()).unwrap_or(Duration::ZERO)
    }

    /// Update animation state each frame
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.frame_count += 1;
        
        let (current_phase, phase_elapsed) = {
            let state = SHARED_ANIM_STATE.lock();
            if let Some(s) = state.as_ref() {
                (s.phase, s.phase_start_time.elapsed())
            } else {
                return;
            }
        };

        // Auto-advance phases based on timing
        match current_phase {
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
        let mut state = SHARED_ANIM_STATE.lock();
        if let Some(s) = state.as_mut() {
            let old_phase = s.phase;
            // Only advance if this is actually a new phase
            if old_phase == new_phase {
                tracing::warn!("🎬 [advance_phase] Ignoring duplicate phase transition to {:?}", new_phase);
                return;
            }
            tracing::info!("🎬 [advance_phase] {:?} -> {:?}", old_phase, new_phase);
            s.phase = new_phase;
            s.phase_start_time = Instant::now();
        }
        
        // Play transition sound
        if new_phase == IntroPhase::TitleReveal || new_phase == IntroPhase::Ready {
            self.audio.play_transition();
        }
    }

    fn on_continue(&mut self, cx: &mut Context<Self>) {
        let can_continue = {
            let state = SHARED_ANIM_STATE.lock();
            state.as_ref().map(|s| s.phase == IntroPhase::Ready && !s.user_interacted).unwrap_or(false)
        };
        
        if can_continue {
            {
                let mut state = SHARED_ANIM_STATE.lock();
                if let Some(s) = state.as_mut() {
                    s.user_interacted = true;
                }
            }
            self.audio.play_click();
            self.audio.play_complete();
            self.advance_phase(IntroPhase::FadeOut, cx);
        }
    }

    /// Get total elapsed time since animation started
    fn get_total_elapsed(&self) -> f32 {
        SHARED_ANIM_STATE.lock()
            .as_ref()
            .map(|s| s.start_time.elapsed().as_secs_f32())
            .unwrap_or(10.0) // Return large value if complete (everything at final state)
    }

    /// Calculate opacity using simple timeline-based animation
    /// Timeline: staggered fade-ins with smooth easing
    fn calculate_element_opacity(&self, element: &str) -> f32 {
        let elapsed = self.get_total_elapsed();
        let current_phase = self.get_phase();
        
        // During fade out, everything fades together with smooth easing
        if current_phase == IntroPhase::FadeOut {
            let fade_elapsed = self.get_phase_elapsed().as_secs_f32();
            return ease_out_cubic(1.0 - (fade_elapsed / 0.6).min(1.0));
        }
        
        if current_phase == IntroPhase::Complete {
            return 0.0;
        }
        
        // Smooth staggered timeline with generous overlap
        match element {
            "background" => ease_out_cubic((elapsed / 0.8).min(1.0)),
            "title" => {
                if elapsed < 0.2 { 0.0 }
                else { ease_out_quart(((elapsed - 0.2) / 1.0).min(1.0)) }
            }
            "subtitle" => {
                if elapsed < 0.6 { 0.0 }
                else { ease_out_quart(((elapsed - 0.6) / 1.0).min(1.0)) }
            }
            "button" => {
                if elapsed < 1.0 { 0.0 }
                else { ease_out_quart(((elapsed - 1.0) / 0.8).min(1.0)) }
            }
            _ => 1.0,
        }
    }
    
    /// Calculate slide offset with smoother motion (20px to 0px)
    fn calculate_element_offset(&self, element: &str) -> f32 {
        let opacity = self.calculate_element_opacity(element);
        // Use ease-out for the slide so it decelerates smoothly
        20.0 * (1.0 - opacity)
    }
}

/// Smooth ease-out cubic - decelerates nicely
fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Even smoother ease-out quartic - more dramatic deceleration
fn ease_out_quart(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(4)
}

impl Render for IntroScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (color1, color2, color3) = self.gradient.gradient_colors();
        
        // Simple timeline-based opacities
        let bg_opacity = self.calculate_element_opacity("background");
        let title_opacity = self.calculate_element_opacity("title");
        let subtitle_opacity = self.calculate_element_opacity("subtitle");
        let button_opacity = self.calculate_element_opacity("button");

        let title_offset = self.calculate_element_offset("title");
        let subtitle_offset = self.calculate_element_offset("subtitle");
        let button_offset = self.calculate_element_offset("button");

        // Animated background gradient
        let bg_primary = hsla(color1.h, color1.s, color1.l, bg_opacity);
        let bg_secondary = hsla(color2.h, color2.s, color2.l, bg_opacity * 0.8);
        let _bg_tertiary = hsla(color3.h, color3.s, color3.l, bg_opacity * 0.6);

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
                    .bg(hsla(color1.h, 0.9, 0.6, bg_opacity * 0.3))
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
                    .bg(hsla(color3.h, 0.85, 0.5, bg_opacity * 0.25))
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

/// Session flag to prevent OOBE loop when --OOBE is used
static OOBE_SHOWN_THIS_SESSION: AtomicBool = AtomicBool::new(false);

/// Check if the user has seen the intro before
/// Returns false if --OOBE flag is passed (forces OOBE to show once per session)
pub fn has_seen_intro() -> bool {
    // Check for --OOBE flag to force OOBE display (but only once per session)
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--OOBE" || arg == "--oobe") {
        // Only show OOBE once per session even with the flag
        if !OOBE_SHOWN_THIS_SESSION.swap(true, Ordering::SeqCst) {
            tracing::info!("🎯 [OOBE] --OOBE flag detected, forcing OOBE display");
            return false;
        } else {
            tracing::info!("🎯 [OOBE] --OOBE flag present but OOBE already shown this session");
        }
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
