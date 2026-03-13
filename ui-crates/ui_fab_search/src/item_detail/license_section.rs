use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    divider::Divider,
    h_flex, v_flex,
    ActiveTheme, Sizable as _, StyledExt,
};

/// One purchasable license tier.
pub struct LicenseEntry {
    pub name: SharedString,
    /// Formatted price string e.g. "USD49.99" or "Free".
    pub price: SharedString,
    /// The Fab offer/purchase URL if available.
    pub purchase_url: Option<SharedString>,
}

/// Displays all available licenses for an asset with their price and a buy button.
#[derive(IntoElement)]
pub struct LicenseSection {
    pub licenses: Vec<LicenseEntry>,
    pub is_free: bool,
}

impl LicenseSection {
    pub fn new(licenses: Vec<LicenseEntry>, is_free: bool) -> Self {
        Self { licenses, is_free }
    }
}

impl RenderOnce for LicenseSection {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let border = cx.theme().border;
        let fg = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;

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
            // ── license rows ─────────────────────────────────────────────
            .children(self.licenses.into_iter().enumerate().map(|(i, entry)| {
                let url = entry.purchase_url.clone();
                div()
                    .w_full()
                    .child(
                        h_flex()
                            .w_full()
                            .gap_3()
                            .items_center()
                            .justify_between()
                            .py_2()
                            // tier name
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(fg)
                                    .child(entry.name),
                            )
                            // price badge + buy action
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    // price
                                    .child(
                                        div()
                                            .text_base()
                                            .font_bold()
                                            .text_color(if self.is_free { success } else { fg })
                                            .child(entry.price),
                                    )
                                    // buy / view button
                                    .when_some(url, |el, purchase_url| {
                                        el.child(
                                            Button::new(
                                                SharedString::from(format!("buy-license-{}", i)),
                                            )
                                            .small()
                                            .success()
                                            .label(if self.is_free { "Get Free" } else { "Buy" })
                                            .on_click(move |_ev, _win, cx| {
                                                cx.open_url(purchase_url.as_ref());
                                            }),
                                        )
                                    }),
                            ),
                    )
                    // subtle separator between rows (except last)
                    .child(Divider::horizontal().color(border))
            }))
    }
}
