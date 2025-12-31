//! OOBE Intro Screen
//!
//! A stunning animated welcome screen for first-time users
//! Features: Multi-page tour, animated gradients, smooth transitions

use gpui::{prelude::*, *};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use ui::{Icon, IconName, Sizable, h_flex, v_flex};

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
    current_page: usize,
    start_time: Instant,
    phase_start_time: Instant,
    page_start_time: Instant,
    user_interacted: bool,
    /// Direction of page transition: 1 = forward (swipe left), -1 = backward (swipe right)
    swipe_direction: i32,
}

/// The current phase of the intro sequence
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum IntroPhase {
    /// Initial fade-in of background
    FadeIn = 0,
    /// Page content animating in
    PageReveal = 1,
    /// Page ready, waiting for user
    Ready = 2,
    /// Transitioning between pages
    PageTransition = 3,
    /// Final fade out
    FadeOut = 4,
    /// Intro complete
    Complete = 5,
}

/// A page in the OOBE tour
#[derive(Clone)]
struct OobePage {
    icon: IconName,
    title: &'static str,
    subtitle: &'static str,
    features: Vec<(&'static str, &'static str)>,
    gradient_hue_offset: f32,
}

/// Event emitted when the intro is complete
pub struct IntroComplete;

/// The main OOBE intro screen component
pub struct IntroScreen {
    gradient: AnimatedGradient,
    audio: IntroAudio,
    pages: Vec<OobePage>,
    frame_count: u64,
    audio_muted: bool,
}

impl EventEmitter<IntroComplete> for IntroScreen {}

impl IntroScreen {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let instance_id = INSTANCE_COUNTER.fetch_add(1, Ordering::SeqCst);
        tracing::debug!("🎬 [IntroScreen] Instance #{} created", instance_id);
        
        // Initialize shared animation state (first call wins)
        let already_created = INTRO_SCREEN_CREATED.swap(true, Ordering::SeqCst);
        
        {
            let mut state = SHARED_ANIM_STATE.lock();
            if state.is_none() {
                tracing::debug!("🎬 [IntroScreen::new] Initializing shared animation state");
                let now = Instant::now();
                *state = Some(SharedAnimState {
                    phase: IntroPhase::FadeIn,
                    current_page: 0,
                    start_time: now,
                    phase_start_time: now,
                    page_start_time: now,
                    user_interacted: false,
                    swipe_direction: 1,
                });
            } else {
                tracing::debug!("🎬 [IntroScreen] Instance #{} reusing existing shared state at phase {:?}", 
                    instance_id, state.as_ref().map(|s| s.phase));
            }
        }
        
        if already_created {
            tracing::warn!("🎬 [IntroScreen::new] IntroScreen already exists, using shared state");
        } else {
            tracing::debug!("🎬 [IntroScreen::new] Creating new IntroScreen instance (first time)");
        }
        
        let audio = IntroAudio::new();
        if !already_created {
            audio.play_ambient();
        }

        let pages = vec![
            OobePage {
                icon: IconName::BrightStar,
                title: "Welcome to Pulsar",
                subtitle: "A modern game engine for creators",
                features: vec![
                    ("🎮", "Build games with a powerful visual scripting system"),
                    ("🚀", "Blazing fast performance powered by Rust"),
                    ("🎨", "Beautiful, customizable editor experience"),
                ],
                gradient_hue_offset: 0.0,
            },
            OobePage {
                icon: IconName::Code,
                title: "Visual Scripting",
                subtitle: "Create logic without writing code",
                features: vec![
                    ("📊", "Node-based blueprint editor"),
                    ("🔗", "Connect nodes to define behavior"),
                    ("⚡", "Real-time preview and debugging"),
                    ("📚", "Extensive library of built-in nodes"),
                ],
                gradient_hue_offset: 0.1,
            },
            OobePage {
                icon: IconName::Folder,
                title: "Asset Management",
                subtitle: "Organize your creative assets",
                features: vec![
                    ("🖼️", "Import textures, models, and audio"),
                    ("📁", "Intuitive file browser with previews"),
                    ("🔄", "Hot-reload for instant iteration"),
                    ("📦", "Smart asset packaging for distribution"),
                ],
                gradient_hue_offset: 0.2,
            },
            OobePage {
                icon: IconName::Globe,
                title: "Level Editor",
                subtitle: "Design immersive worlds",
                features: vec![
                    ("🗺️", "3D viewport with intuitive controls"),
                    ("🏗️", "Place and transform objects easily"),
                    ("💡", "Real-time lighting and shadows"),
                    ("🎭", "Scene hierarchy management"),
                ],
                gradient_hue_offset: 0.3,
            },
            OobePage {
                icon: IconName::Group,
                title: "Collaborate",
                subtitle: "Build together in real-time",
                features: vec![
                    ("👥", "Multi-user editing support"),
                    ("💬", "Built-in chat and voice"),
                    ("📡", "Seamless project synchronization"),
                    ("🔒", "Version control integration"),
                ],
                gradient_hue_offset: 0.4,
            },
            OobePage {
                icon: IconName::Rocket,
                title: "Ready to Create?",
                subtitle: "Your journey begins now",
                features: vec![
                    ("📖", "Extensive documentation and tutorials"),
                    ("🎯", "Start from templates or scratch"),
                    ("💡", "Active community and support"),
                ],
                gradient_hue_offset: 0.5,
            },
        ];

        let screen = Self {
            gradient: AnimatedGradient::arc_style(),
            audio,
            pages,
            frame_count: 0,
            audio_muted: false,
        };

        // Start the animation loop only on first creation
        if !already_created {
            tracing::debug!("🎬 [IntroScreen] Starting animation loop");
            cx.spawn(async move |this, mut cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(16)).await;
                    
                    let should_continue = cx.update(|cx| {
                        this.update(cx, |screen, cx| {
                            screen.tick(cx);
                            let phase = SHARED_ANIM_STATE.lock().as_ref().map(|s| s.phase).unwrap_or(IntroPhase::Complete);
                            phase != IntroPhase::Complete
                        }).unwrap_or(false)
                    }).unwrap_or(false);

                    if !should_continue {
                        tracing::debug!("🎬 [IntroScreen] Animation loop complete");
                        break;
                    }
                }
            }).detach();
        }

        screen
    }

    /// Get current page index from shared state
    fn get_current_page(&self) -> usize {
        SHARED_ANIM_STATE.lock().as_ref().map(|s| s.current_page).unwrap_or(0)
    }

    /// Get current phase from shared state
    fn get_phase(&self) -> IntroPhase {
        SHARED_ANIM_STATE.lock().as_ref().map(|s| s.phase).unwrap_or(IntroPhase::Complete)
    }
    
    /// Get phase elapsed time from shared state
    fn get_phase_elapsed(&self) -> Duration {
        SHARED_ANIM_STATE.lock().as_ref().map(|s| s.phase_start_time.elapsed()).unwrap_or(Duration::ZERO)
    }

    /// Get page elapsed time from shared state
    fn get_page_elapsed(&self) -> Duration {
        SHARED_ANIM_STATE.lock().as_ref().map(|s| s.page_start_time.elapsed()).unwrap_or(Duration::ZERO)
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

        match current_phase {
            IntroPhase::FadeIn => {
                if phase_elapsed > Duration::from_millis(600) {
                    self.advance_phase(IntroPhase::PageReveal, cx);
                }
            }
            IntroPhase::PageReveal => {
                if phase_elapsed > Duration::from_millis(800) {
                    self.advance_phase(IntroPhase::Ready, cx);
                }
            }
            IntroPhase::PageTransition => {
                if phase_elapsed > Duration::from_millis(500) {
                    // Go directly to Ready - no fade in after swipe
                    self.advance_phase(IntroPhase::Ready, cx);
                }
            }
            IntroPhase::FadeOut => {
                if phase_elapsed > Duration::from_millis(500) {
                    self.advance_phase(IntroPhase::Complete, cx);
                    self.audio.stop_all(); // Stop audio when OOBE closes
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
            if old_phase == new_phase {
                return;
            }
            tracing::debug!("🎬 [advance_phase] {:?} -> {:?}", old_phase, new_phase);
            s.phase = new_phase;
            s.phase_start_time = Instant::now();
        }
        
        if new_phase == IntroPhase::PageReveal {
            self.audio.play_transition();
        }
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        let (current_page, total_pages, phase) = {
            let state = SHARED_ANIM_STATE.lock();
            if let Some(s) = state.as_ref() {
                (s.current_page, self.pages.len(), s.phase)
            } else {
                return;
            }
        };

        if phase != IntroPhase::Ready {
            return;
        }

        self.audio.play_click();

        if current_page + 1 >= total_pages {
            self.finish(cx);
        } else {
            {
                let mut state = SHARED_ANIM_STATE.lock();
                if let Some(s) = state.as_mut() {
                    s.current_page += 1;
                    s.page_start_time = Instant::now();
                    s.swipe_direction = 1; // Forward = swipe left
                }
            }
            self.advance_phase(IntroPhase::PageTransition, cx);
        }
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        let (current_page, phase) = {
            let state = SHARED_ANIM_STATE.lock();
            if let Some(s) = state.as_ref() {
                (s.current_page, s.phase)
            } else {
                return;
            }
        };

        if phase != IntroPhase::Ready || current_page == 0 {
            return;
        }

        self.audio.play_click();

        {
            let mut state = SHARED_ANIM_STATE.lock();
            if let Some(s) = state.as_mut() {
                s.current_page -= 1;
                s.page_start_time = Instant::now();
                s.swipe_direction = -1; // Backward = swipe right
            }
        }
        self.advance_phase(IntroPhase::PageTransition, cx);
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        let can_finish = {
            let state = SHARED_ANIM_STATE.lock();
            state.as_ref().map(|s| !s.user_interacted).unwrap_or(false)
        };
        
        if can_finish {
            {
                let mut state = SHARED_ANIM_STATE.lock();
                if let Some(s) = state.as_mut() {
                    s.user_interacted = true;
                }
            }
            self.audio.play_complete();
            self.advance_phase(IntroPhase::FadeOut, cx);
        }
    }

    fn skip(&mut self, cx: &mut Context<Self>) {
        self.audio.play_click();
        self.finish(cx);
    }

    /// Calculate content opacity based on phase
    fn calculate_content_opacity(&self) -> f32 {
        let phase = self.get_phase();
        let phase_elapsed = self.get_phase_elapsed().as_secs_f32();
        
        match phase {
            IntroPhase::FadeIn => ease_out_cubic((phase_elapsed / 0.6).min(1.0)),
            IntroPhase::PageReveal => ease_out_cubic((phase_elapsed / 0.5).min(1.0)),
            IntroPhase::Ready => 1.0,
            IntroPhase::PageTransition => {
                let t = phase_elapsed / 0.4;
                if t < 0.5 {
                    ease_out_cubic(1.0 - t * 2.0)
                } else {
                    ease_out_cubic((t - 0.5) * 2.0)
                }
            }
            IntroPhase::FadeOut => ease_out_cubic(1.0 - (phase_elapsed / 0.5).min(1.0)),
            IntroPhase::Complete => 0.0,
        }
    }

    /// Calculate slide offset for content
    fn calculate_content_offset(&self) -> f32 {
        let phase = self.get_phase();
        let phase_elapsed = self.get_phase_elapsed().as_secs_f32();
        
        match phase {
            IntroPhase::PageReveal => 30.0 * (1.0 - ease_out_quart((phase_elapsed / 0.5).min(1.0))),
            IntroPhase::PageTransition => {
                let t = phase_elapsed / 0.4;
                if t < 0.5 {
                    30.0 * t * 2.0
                } else {
                    30.0 * (1.0 - (t - 0.5) * 2.0)
                }
            }
            _ => 0.0,
        }
    }

    fn render_page(&self, page: &OobePage, opacity: f32, offset: f32, page_elapsed_secs: f32, _cx: &mut Context<Self>) -> impl IntoElement {
        // Staggered feature animations - each feature fades in with a delay
        let features: Vec<_> = page.features.iter().enumerate().map(|(i, (emoji, text))| {
            let delay = 0.3 + (i as f32 * 0.15); // Start at 0.3s, stagger by 0.15s
            let feature_opacity = if page_elapsed_secs > delay {
                ease_out_quart(((page_elapsed_secs - delay) / 0.5).min(1.0))
            } else {
                0.0
            };
            
            div()
                .opacity(feature_opacity * opacity)
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .text_xl()
                                .child(emoji.to_string())
                        )
                        .child(
                            div()
                                .text_base()
                                .text_color(white().opacity(0.85))
                                .child(text.to_string())
                        )
                )
        }).collect();

        v_flex()
            .items_center()
            .gap_6()
            .mt(px(offset))
            .child(
                div()
                    .opacity(opacity)
                    .child(
                        div()
                            .w(px(100.0))
                            .h(px(100.0))
                            .rounded_full()
                            .bg(white().opacity(0.1))
                            .border_1()
                            .border_color(white().opacity(0.2))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                ui::Icon::new(page.icon.clone())
                                    .size(px(48.0))
                                    .text_color(white())
                            )
                    )
            )
            .child(
                div()
                    .opacity(opacity)
                    .child(
                        div()
                            .text_size(px(42.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(white())
                            .child(page.title)
                    )
            )
            .child(
                div()
                    .opacity(opacity * 0.9)
                    .child(
                        div()
                            .text_xl()
                            .text_color(white().opacity(0.8))
                            .child(page.subtitle)
                    )
            )
            .child(
                v_flex()
                    .gap_4()
                    .mt_4()
                    .p_6()
                    .rounded_xl()
                    .bg(black().opacity(0.2))
                    .children(features)
            )
    }

    fn render_navigation(&self, current_page: usize, total_pages: usize, opacity: f32, phase: IntroPhase, cx: &mut Context<Self>) -> impl IntoElement {
        let is_last_page = current_page + 1 >= total_pages;
        let is_first_page = current_page == 0;
        let can_interact = phase == IntroPhase::Ready;

        let back_enabled = can_interact && !is_first_page;
        let back_btn = div()
            .px_5()
            .py_2()
            .rounded_full()
            .bg(white().opacity(if is_first_page { 0.05 } else { 0.1 }))
            .border_1()
            .border_color(white().opacity(if is_first_page { 0.1 } else { 0.2 }))
            .when(back_enabled, |s| s.cursor_pointer())
            .hover(|s| if back_enabled { s.bg(white().opacity(0.2)) } else { s })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        ui::Icon::new(IconName::ArrowLeft)
                            .size_4()
                            .text_color(white().opacity(if is_first_page { 0.3 } else { 0.9 }))
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(white().opacity(if is_first_page { 0.3 } else { 0.9 }))
                            .child("Back")
                    )
            );

        h_flex()
            .gap_4()
            .items_center()
            .opacity(opacity)
            .child(
                div()
                    .id("back-btn")
                    .child(back_btn)
                    .when(back_enabled, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| {
                            this.prev_page(cx);
                        }))
                    })
            )
            .child(
                h_flex()
                    .gap_2()
                    .px_4()
                    .children((0..total_pages).map(|i| {
                        div()
                            .w(px(if i == current_page { 24.0 } else { 8.0 }))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(white().opacity(if i == current_page { 0.9 } else { 0.3 }))
                    }))
            )
            .child({
                let next_btn = div()
                    .px_5()
                    .py_2()
                    .rounded_full()
                    .bg(white().opacity(0.15))
                    .border_1()
                    .border_color(white().opacity(0.3))
                    .when(can_interact, |s| s.cursor_pointer())
                    .hover(|s| if can_interact { s.bg(white().opacity(0.25)) } else { s })
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(white())
                                    .child(if is_last_page { "Get Started" } else { "Next" })
                            )
                            .child(
                                ui::Icon::new(if is_last_page { IconName::Check } else { IconName::ArrowRight })
                                    .size_4()
                                    .text_color(white())
                            )
                    );

                div()
                    .id("next-btn")
                    .child(next_btn)
                    .when(can_interact, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| {
                            this.next_page(cx);
                        }))
                    })
            })
    }
}

/// Smooth ease-out cubic
fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// Even smoother ease-out quartic
fn ease_out_quart(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(4)
}

impl Render for IntroScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Capture all animation state ONCE at the start of render
        // Use TOTAL elapsed time from animation start as single source of truth
        let (current_page, phase, total_elapsed_secs, page_start_elapsed_secs, swipe_direction) = {
            let state = SHARED_ANIM_STATE.lock();
            if let Some(s) = state.as_ref() {
                let total = s.start_time.elapsed().as_secs_f32();
                // Page elapsed = time since this page started (from the absolute timeline)
                let page_elapsed = s.page_start_time.elapsed().as_secs_f32();
                (s.current_page, s.phase, total, page_elapsed, s.swipe_direction)
            } else {
                (0, IntroPhase::Complete, 10.0, 10.0, 1)
            }
        };
        
        let total_pages = self.pages.len();
        let page = self.pages.get(current_page).cloned().unwrap_or_else(|| self.pages[0].clone());
        
        let (color1, color2, color3) = self.gradient.gradient_colors();
        
        // Apply page-specific hue offset for variety
        let hue_offset = page.gradient_hue_offset;
        let color1 = Hsla { h: (color1.h + hue_offset) % 1.0, ..color1 };
        let color2 = Hsla { h: (color2.h + hue_offset) % 1.0, ..color2 };
        let color3 = Hsla { h: (color3.h + hue_offset) % 1.0, ..color3 };
        
        // Calculate background opacity - fade in at start, fade out at end
        let bg_opacity = match phase {
            IntroPhase::FadeIn => ease_out_cubic((total_elapsed_secs / 0.8).min(1.0)),
            IntroPhase::FadeOut => {
                // Use page_start_elapsed as proxy for fade-out progress
                ease_out_cubic(1.0 - (page_start_elapsed_secs / 0.5).min(1.0))
            }
            IntroPhase::Complete => 0.0,
            _ => 1.0,
        };
        
        // Content opacity based on page timeline
        // Only first page fades in - all others just swipe
        let content_opacity = match phase {
            IntroPhase::FadeIn => ease_out_cubic((total_elapsed_secs / 0.8).min(1.0)),
            IntroPhase::PageReveal => ease_out_quart((page_start_elapsed_secs / 0.6).min(1.0)),
            IntroPhase::Ready => 1.0,
            IntroPhase::PageTransition => 1.0, // Full opacity during swipe - no fade
            IntroPhase::FadeOut => ease_out_cubic(1.0 - (page_start_elapsed_secs / 0.5).min(1.0)),
            IntroPhase::Complete => 0.0,
        };
        
        // Vertical content slide offset
        let content_offset = match phase {
            IntroPhase::PageReveal => 20.0 * (1.0 - ease_out_quart((page_start_elapsed_secs / 0.6).min(1.0))),
            _ => 0.0,
        };
        
        // Horizontal swipe offset for page transitions
        // Single slide-in animation - new content slides in from swipe direction
        let swipe_offset: f32 = match phase {
            IntroPhase::PageTransition => {
                let t = (page_start_elapsed_secs / 0.5).min(1.0);
                let direction = swipe_direction as f32;
                // Forward (dir=1): slide in from right (+200 -> 0)
                // Backward (dir=-1): slide in from left (-200 -> 0)
                200.0 * direction * (1.0 - ease_out_quart(t))
            }
            _ => 0.0,
        };

        let bg_primary = hsla(color1.h, color1.s, color1.l * 0.4, bg_opacity);
        let bg_secondary = hsla(color2.h, color2.s, color2.l * 0.3, bg_opacity * 0.8);

        // Mute button state
        let muted = self.audio_muted;

        div()
            .id("oobe-intro")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(bg_primary)
            // Overlay gradient
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .bg(bg_secondary)
                    .opacity(0.6)
            )
            // Animated glow - top right
            .child(
                div()
                    .absolute()
                    .top(px(-100.0))
                    .right(px(-100.0))
                    .w(px(600.0))
                    .h(px(600.0))
                    .rounded_full()
                    .bg(hsla(color1.h, 0.9, 0.5, bg_opacity * 0.25))
            )
            // Animated glow - bottom left
            .child(
                div()
                    .absolute()
                    .bottom(px(-150.0))
                    .left(px(-150.0))
                    .w(px(500.0))
                    .h(px(500.0))
                    .rounded_full()
                    .bg(hsla(color3.h, 0.85, 0.45, bg_opacity * 0.2))
            )
            // Main content with swipe animation (only page content swipes, not navigation)
            .child(
                v_flex()
                    .items_center()
                    .gap_8()
                    .max_w(px(600.0))
                    // Page content swipes
                    .child(
                        div()
                            .ml(px(swipe_offset))
                            .child(self.render_page(&page, content_opacity, content_offset, page_start_elapsed_secs, cx))
                    )
                    // Navigation stays in place
                    .child(
                        div()
                            .mt_6()
                            .child(self.render_navigation(current_page, total_pages, content_opacity, phase, cx))
                    )
            )
            // Skip button
            .child(
                div()
                    .absolute()
                    .top_4()
                    .right_4()
                    .opacity(content_opacity * 0.5)
                    .child(
                        div()
                            .id("skip-btn")
                            .px_4()
                            .py_2()
                            .rounded_lg()
                            .text_sm()
                            .text_color(white().opacity(0.6))
                            .hover(|s| s.text_color(white().opacity(0.9)).cursor_pointer())
                            .child("Skip Tour")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.skip(cx);
                            }))
                    )
            )
            // Page counter and mute button
            .child(
                h_flex()
                    .absolute()
                    .bottom_4()
                    .right_4()
                    .gap_4()
                    .items_center()
                    .opacity(content_opacity * 0.4)
                    .child(
                        div()
                            .text_sm()
                            .text_color(white().opacity(0.5))
                            .child(format!("{} / {}", current_page + 1, total_pages))
                    )
                    .child(
                        div()
                            .id("mute-btn")
                            .px_2()
                            .py_1()
                            .rounded_lg()
                            .bg(hsla(0.0, 0.0, 0.0, 0.15))
                            .hover(|s| s.bg(hsla(0.0, 0.0, 0.0, 0.25)).cursor_pointer())
                            .child(
                                if muted {
                                    Icon::new(IconName::SoundOff).large().render(_window, cx)
                                } else {
                                    Icon::new(IconName::SoundHigh).large().render(_window, cx)
                                }
                            )
                            .on_click(cx.listener(|this, _, _, _| {
                                this.audio_muted = !this.audio_muted;
                                if this.audio_muted {
                                    this.audio.set_enabled(false);
                                    this.audio.stop_all();
                                } else {
                                    this.audio.set_enabled(true);
                                    this.audio.play_ambient();
                                }
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
            tracing::debug!("🎯 [OOBE] --OOBE flag detected, forcing OOBE display");
            return false;
        } else {
            tracing::debug!("🎯 [OOBE] --OOBE flag present but OOBE already shown this session");
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
