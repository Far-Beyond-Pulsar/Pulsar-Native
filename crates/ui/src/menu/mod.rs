use gpui::App;

mod menu_item;

pub mod app_menu_bar;
pub use app_menu_bar::{AppMenuBar, AppMenusCache};
pub mod context_menu;
pub mod popup_menu;
pub use popup_menu::{PopupMenu, PopupMenuExt as DropdownMenu, PopupMenuItem};

pub(crate) fn init(cx: &mut App) {
    app_menu_bar::init(cx);
    popup_menu::init(cx);
}

struct MenuInit;
impl crate::registry::UiComponentInit for MenuInit {
    fn init(&self, cx: &mut App) {
        init(cx);
    }
}
crate::register_ui_component!(MenuInit);
