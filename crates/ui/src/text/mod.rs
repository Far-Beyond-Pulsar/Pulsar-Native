mod format;
mod inline;
mod node;
mod style;
mod text_view;
mod utils;

use gpui::App;
pub use style::*;
pub use text_view::*;

pub(crate) fn init(cx: &mut App) {
    text_view::init(cx);
}

struct TextInit;
impl crate::registry::UiComponentInit for TextInit {
    fn init(&self, cx: &mut App) {
        init(cx);
    }
}
crate::register_ui_component!(TextInit);
