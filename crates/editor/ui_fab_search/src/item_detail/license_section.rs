use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Icon, IconName, Sizable as _, StyledExt,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

/// Displays the Sketchfab model license with an optional link to the viewer.
#[derive(IntoElement)]
pub struct LicenseSection {
    /// Human-readable license label (e.g. "CC BY", "CC0 (Public Domain)").
    pub license_label: Option<SharedString>,
    /// Whether the model is available for download.
    pub is_downloadable: bool,
    /// Sketchfab viewer URL for the model.
    pub viewer_url: SharedString,
}

impl LicenseSection {
    pub fn new(
        license_label: Option<impl Into<SharedString>>,
        is_downloadable: bool,
        viewer_url: impl Into<SharedString>,
    ) -> Self {
        Self {
            license_label: license_label.map(|l| l.into()),
            is_downloadable,
            viewer_url: viewer_url.into(),
        }
    }
}

impl RenderOnce for LicenseSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let viewer_url = self.viewer_url.clone();
        let viewer_url2 = self.viewer_url.clone();

        v_flex()
            .w_full()
            .px_5()
            .py_4()
            .gap_3()
            .border_b_1()
            .border_color(border)
            // ── section label ────────────────────────────────────────────
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(muted)
                    .child("Licensing"),
            )
            // ── license row ──────────────────────────────────────────────
            .child(
                h_flex()
                    .w_full()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        div().text_sm().font_semibold().text_color(fg).child(
                            self.license_label
                                .unwrap_or_else(|| SharedString::from("Unknown License")),
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_3()
                            .items_center()
                            .when(self.is_downloadable, |el| {
                                el.child(
                                    div()
                                        .text_sm()
                                        .font_bold()
                                        .text_color(success)
                                        .child("Free Download"),
                                )
                                .child(
                                    Button::new("view-on-sketchfab-dl")
                                        .small()
                                        .success()
                                        .icon(Icon::new(IconName::ExternalLink).small())
                                        .label("Download")
                                        .on_click(move |_ev, _win, cx| {
                                            cx.open_url(viewer_url.as_ref());
                                        }),
                                )
                            })
                            .when(!self.is_downloadable, |el| {
                                el.child(
                                    Button::new("view-on-sketchfab")
                                        .small()
                                        .ghost()
                                        .icon(Icon::new(IconName::ExternalLink).small())
                                        .label("View on Sketchfab")
                                        .on_click(move |_ev, _win, cx| {
                                            cx.open_url(viewer_url2.as_ref());
                                        }),
                                )
                            }),
                    ),
            )
    }
}
