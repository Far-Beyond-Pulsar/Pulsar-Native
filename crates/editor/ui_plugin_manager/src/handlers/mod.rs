use gpui::*;

use crate::screen::PluginManagerWindow;

pub fn on_refresh(
    this: &mut PluginManagerWindow,
    _: &ClickEvent,
    _window: &mut Window,
    cx: &mut Context<PluginManagerWindow>,
) {
    this.refresh(cx);
}
