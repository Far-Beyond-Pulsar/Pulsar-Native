//! Root wrapper component that contains the titlebar and app

use gpui::{prelude::*, div, Entity, IntoElement, Render, SharedString, Window, Context};
use ui::{v_flex, Root};
use ui_common::menu::{AboutApp, Settings, Preferences, ShowDocumentation, AppTitleBar};

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

        // Belt-and-suspenders action handlers at the root level so that menu
        // actions are always caught regardless of which child element has focus
        // at the time the popup menu fires dispatch_action.
        div()
            .size_full()
            .on_action(cx.listener(|this: &mut PulsarRoot, _: &Settings, window, cx| {
                println!("[MENU] PulsarRoot caught Settings action");
                this.app.update(cx, |app, cx| app.open_settings(window, cx));
            }))
            .on_action(cx.listener(|this: &mut PulsarRoot, _: &ui::OpenSettings, window, cx| {
                println!("[MENU] PulsarRoot caught OpenSettings action");
                this.app.update(cx, |app, cx| app.open_settings(window, cx));
            }))
            .on_action(cx.listener(|this: &mut PulsarRoot, _: &Preferences, window, cx| {
                println!("[MENU] PulsarRoot caught Preferences action");
                this.app.update(cx, |app, cx| app.open_settings(window, cx));
            }))
            .on_action(cx.listener(|this: &mut PulsarRoot, _: &AboutApp, window, cx| {
                println!("[MENU] PulsarRoot caught AboutApp action");
                this.app.update(cx, |app, cx| app.open_about(window, cx));
            }))
            .on_action(cx.listener(|this: &mut PulsarRoot, _: &ShowDocumentation, window, cx| {
                println!("[MENU] PulsarRoot caught ShowDocumentation action");
                this.app.update(cx, |app, cx| app.open_documentation(window, cx));
            }))
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
