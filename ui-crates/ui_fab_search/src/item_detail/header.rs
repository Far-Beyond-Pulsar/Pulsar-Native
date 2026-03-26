use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, ActiveTheme, Icon, IconName, Sizable as _, StyledExt,
};

/// The sticky top-bar of the item detail page.
/// Contains a Back button, the asset title, and an "Open on Fab" external link.
#[derive(IntoElement)]
pub struct DetailHeader {
    pub title: SharedString,
    pub fab_url: SharedString,
    pub on_back: Box<dyn Fn(&mut Window, &mut App) + 'static>,
}

impl DetailHeader {
    pub fn new(
        title: impl Into<SharedString>,
        fab_url: impl Into<SharedString>,
        on_back: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            fab_url: fab_url.into(),
            on_back: Box::new(on_back),
        }
    }
}

impl RenderOnce for DetailHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let fab_url = self.fab_url.clone();

        h_flex()
            .w_full()
            .px_5()
            .py_3()
            .gap_3()
            .border_b_1()
            .border_color(border)
            .items_center()
            .bg(cx.theme().sidebar)
            // ── back button ─────────────────────────────────────────────
            .child(
                Button::new("detail-back")
                    .small()
                    .ghost()
                    .icon(Icon::new(IconName::ArrowLeft).small())
                    .label("Back")
                    .on_click(move |_ev, window, cx| (self.on_back)(window, cx)),
            )
            // ── title ────────────────────────────────────────────────────
            .child(
                div()
                    .flex_1()
                    .text_base()
                    .font_bold()
                    .text_color(fg)
                    .truncate()
                    .child(self.title),
            )
            // ── view on sketchfab ─────────────────────────────────────
            .child(
                Button::new("detail-open-sketchfab")
                    .small()
                    .primary()
                    .icon(Icon::new(IconName::ExternalLink).small())
                    .label("View on Sketchfab")
                    .on_click(move |_ev, _window, cx| {
                        cx.open_url(fab_url.as_ref());
                    }),
            )
    }
}
