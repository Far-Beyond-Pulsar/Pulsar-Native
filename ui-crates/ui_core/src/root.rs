//! Root wrapper component that contains the titlebar and app

use gpui::{prelude::*, div, Entity, IntoElement, Render, SharedString, Window, Context};
use ui::{v_flex, Root};
use ui_common::menu::AppTitleBar;

use crate::app::PulsarApp;

/// Root wrapper that contains the titlebar, matching gpui-component storybook structure
pub struct PulsarRoot {
    title_bar: Entity<AppTitleBar>,
    app: Entity<PulsarApp>,
}

impl PulsarRoot {
    pub fn new(
        title: impl Into<SharedString>,
        app: Entity<PulsarApp>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let title_bar = cx.new(|cx| AppTitleBar::new(title, window, cx));
        Self { title_bar, app }
    }
}

impl Render for PulsarRoot {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let drawer_layer = Root::render_drawer_layer(window, cx);
        let modal_layer = Root::render_modal_layer(window, cx);
        let notification_layer = Root::render_notification_layer(window, cx);

        div()
            .size_full()
            .child(
                v_flex()
                    .size_full()
                    .child(self.title_bar.clone())
                    .child(div().flex_1().overflow_hidden().child(self.app.clone()))
            )
            .children(drawer_layer)
            .children(modal_layer)
            .children(notification_layer)
    }
}
