use gpui::*;
use crate::entry_screen::EntryScreen;
use crate::entry_screen::project_selector::ProjectSelected;

pub struct EntryWindow {
    entry_screen: Option<Entity<EntryScreen>>,
}

impl EntryWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let entry_screen = cx.new(|cx| EntryScreen::new(window, cx));


        Self {
            entry_screen: Some(entry_screen),
        }
    }

    pub fn new_placeholder(cx: &mut Context<Self>) -> Self {
        Self {
            entry_screen: None,
        }
    }

    pub fn entry_screen(&self) -> Option<&Entity<EntryScreen>> {
        self.entry_screen.as_ref()
    }
}

impl EventEmitter<ProjectSelected> for EntryWindow {}

impl Render for EntryWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(screen) = &self.entry_screen {
            screen.clone().into_any_element()
        } else {
            div().into_any_element()
        }
    }
}
