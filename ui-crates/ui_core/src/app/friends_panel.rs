use gpui::{prelude::*, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window};
use ui::dock::{Panel, PanelEvent};
use ui::v_flex;

pub struct FriendsPanel {
    pub friends_screen: Entity<ui_friends::FriendsScreen>,
    focus_handle: FocusHandle,
}

impl FriendsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let friends_screen = cx.new(|cx| ui_friends::FriendsScreen::new(window, cx));
        let focus_handle = cx.focus_handle();
        Self {
            friends_screen,
            focus_handle,
        }
    }
}

impl EventEmitter<PanelEvent> for FriendsPanel {}

impl Focusable for FriendsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FriendsPanel {
    fn panel_name(&self) -> &'static str {
        "friends"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        "Friends".into_any_element()
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState::new(self)
    }
}

impl Render for FriendsPanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().size_full().child(self.friends_screen.clone())
    }
}
