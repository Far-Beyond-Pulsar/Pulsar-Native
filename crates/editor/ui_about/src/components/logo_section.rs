use std::sync::Arc;

use gpui::*;
use ui::ActiveTheme;

pub fn render_logo_section(
    logo: &Option<Arc<RenderImage>>,
    theme: &ui::Theme,
) -> impl IntoElement {
    div()
        .w(px(120.0))
        .h(px(120.0))
        .rounded_2xl()
        .bg(theme.accent.opacity(0.15))
        .border_2()
        .border_color(theme.accent.opacity(0.3))
        .flex()
        .items_center()
        .justify_center()
        .shadow_lg()
        .children(logo.clone().map(|logo| {
            img(ImageSource::Render(logo))
                .w(px(100.0))
                .h(px(100.0))
                .object_fit(gpui::ObjectFit::Contain)
        }))
}
