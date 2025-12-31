use gpui::*;
use crate::entry_screen::EntryScreen;
use crate::entry_screen::project_selector::ProjectSelected;
use crate::oobe::{IntroScreen, IntroComplete, has_seen_intro, mark_intro_seen};

/// The current screen state of the entry window
enum ScreenState {
    /// Showing the OOBE intro screen
    Intro(Entity<IntroScreen>),
    /// Showing the main entry/project selection screen
    Entry(Entity<EntryScreen>),
}

pub struct EntryWindow {
    screen_state: ScreenState,
}

impl EntryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Check if we should show the OOBE intro
        let seen_intro = has_seen_intro();
        tracing::debug!("🎯 [EntryWindow] has_seen_intro() = {}", seen_intro);
        
        if !seen_intro {
            tracing::debug!("🎉 [OOBE] Showing intro screen for first-time user");
            // Show the OOBE intro screen first
            let intro_screen = cx.new(|cx| IntroScreen::new(window, cx));
            
            // Subscribe to intro completion
            cx.subscribe_in(&intro_screen, window, |this: &mut Self, _screen, _event: &IntroComplete, window, cx| {
                tracing::debug!("🎉 [OOBE] Intro complete, transitioning to entry screen");
                mark_intro_seen();
                
                // Transition to entry screen
                let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));
                this.screen_state = ScreenState::Entry(entry_screen);
                cx.notify();
            }).detach();
            
            Self {
                screen_state: ScreenState::Intro(intro_screen),
            }
        } else {
            tracing::debug!("🎯 [OOBE] Intro already seen, showing entry screen directly");
            // Skip intro, go directly to entry screen
            let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));
            Self {
                screen_state: ScreenState::Entry(entry_screen),
            }
        }
    }

    pub fn new_placeholder(cx: &mut Context<Self>) -> Self {
        Self {
            screen_state: ScreenState::Entry(cx.new(|cx| {
                // Create a minimal placeholder - this shouldn't be used normally
                panic!("EntryWindow::new_placeholder should not be called in normal operation")
            })),
        }
    }

    pub fn entry_screen(&self) -> Option<&Entity<EntryScreen>> {
        match &self.screen_state {
            ScreenState::Entry(screen) => Some(screen),
            _ => None,
        }
    }
}

impl EventEmitter<ProjectSelected> for EntryWindow {}

impl Render for EntryWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.screen_state {
            ScreenState::Intro(intro) => intro.clone().into_any_element(),
            ScreenState::Entry(screen) => screen.clone().into_any_element(),
        }
    }
}
