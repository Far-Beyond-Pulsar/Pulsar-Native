use gpui::SharedString;

#[derive(Clone, PartialEq, Eq, gpui::Action)]
#[action(namespace = ui, no_json)]
pub struct SelectThemeAction {
    pub theme_name: SharedString,
}

impl SelectThemeAction {
    pub fn new(theme_name: SharedString) -> Self {
        Self { theme_name }
    }
}
