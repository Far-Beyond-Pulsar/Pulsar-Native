use gpui::App;

mod menu_item;

pub mod app_menu_bar;
pub use app_menu_bar::AppMenuBar;
pub mod context_menu;
pub mod popup_menu;
pub(crate) use popup_menu::PopupMenuItem;

pub(crate) fn init(cx: &mut App) {
    app_menu_bar::init(cx);
    popup_menu::init(cx);
}
