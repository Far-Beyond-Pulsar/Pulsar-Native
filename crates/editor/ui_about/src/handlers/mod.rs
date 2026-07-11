use gpui::*;

use crate::screen::AboutWindow;

pub fn on_open_github(
    _this: &mut AboutWindow,
    _: &ClickEvent,
    _window: &mut Window,
    cx: &mut Context<AboutWindow>,
) {
    cx.open_url("https://github.com/Far-Beyond-Pulsar/Pulsar-Native");
}

pub fn on_open_docs(
    _this: &mut AboutWindow,
    _: &ClickEvent,
    _window: &mut Window,
    cx: &mut Context<AboutWindow>,
) {
    cx.open_url("https://docs.pulsarengine.dev");
}
