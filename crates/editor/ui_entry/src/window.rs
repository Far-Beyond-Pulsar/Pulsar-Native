use crate::screen::EntryScreen;
use crate::{FabSearchRequested, GitManagerRequested, ProjectSelected, SettingsRequested};
use gpui::*;

pub struct EntryWindow {
    screen: Entity<EntryScreen>,
}

impl EntryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));
        let s = entry_screen.clone();
        cx.subscribe_in(
            &s,
            window,
            |this: &mut Self, _screen, event: &ProjectSelected, _window, cx| {
                cx.emit(event.clone());
            },
        )
        .detach();
        cx.subscribe_in(
            &s,
            window,
            |this: &mut Self, _screen, event: &GitManagerRequested, _window, cx| {
                cx.emit(event.clone());
            },
        )
        .detach();
        cx.subscribe_in(
            &s,
            window,
            |this: &mut Self, _screen, _event: &SettingsRequested, _window, cx| {
                cx.emit(SettingsRequested);
            },
        )
        .detach();
        cx.subscribe_in(
            &s,
            window,
            |this: &mut Self, _screen, _event: &FabSearchRequested, _window, cx| {
                cx.emit(FabSearchRequested);
            },
        )
        .detach();
        Self {
            screen: entry_screen,
        }
    }

    pub fn entry_screen(&self) -> &Entity<EntryScreen> {
        &self.screen
    }
}

impl EventEmitter<ProjectSelected> for EntryWindow {}
impl EventEmitter<GitManagerRequested> for EntryWindow {}
impl EventEmitter<SettingsRequested> for EntryWindow {}
impl EventEmitter<FabSearchRequested> for EntryWindow {}

impl Render for EntryWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.screen.clone().into_any_element()
    }
}
