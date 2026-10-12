//! The footer under the editor, as its own view.
//!
//! What it shows changes in the background (the editor task count,
//! rust-analyzer's progress, the multiuser session), and a notify reaches
//! every view that reads the notified entity. Those updates notify this view
//! instead of [`PulsarApp`], so the views that read the app's state (the left
//! sidebar) replay rather than rebuild. The footer itself is still
//! [`PulsarApp::render_footer`], reading the app's state.

use gpui::{div, Context, IntoElement, ParentElement as _, Render, WeakEntity, Window};

use super::PulsarApp;

/// The footer's height: [`PulsarApp::render_footer`] draws one 28 px row.
pub(crate) const STATUS_BAR_HEIGHT: f32 = 28.;

pub struct StatusBar {
    app: WeakEntity<PulsarApp>,
}

impl StatusBar {
    pub(crate) fn new(app: WeakEntity<PulsarApp>) -> Self {
        Self { app }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(app) = self.app.upgrade() else {
            return div().into_any_element();
        };
        app.update(cx, |app, cx| {
            let drawer_shown = app.state.drawer_open || app.state.drawer_docked;
            div()
                .child(app.render_footer(drawer_shown, cx))
                .into_any_element()
        })
    }
}
