//! The selected page: its header, then each section's heading and settings,
//! one `list` row each. The list measures every row (`measure_all`), so a
//! wrapped description makes its row taller instead of spilling into the
//! next one, and a section jump scrolls to the heading's measured position.

use gpui::{
    div, list, prelude::FluentBuilder as _, px, AnyElement, Context, IntoElement,
    ParentElement as _, SharedString, Styled, Window,
};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    label::Label,
    scroll::Scrollbar,
    v_flex, ActiveTheme as _, IconName, Sizable as _,
};

use super::{empty_note, SettingsScreen};
use crate::utils::model::{ContentRow, SettingId};
use crate::utils::store;

/// Rows never grow wider than this, so a wide window stays readable.
const MAX_WIDTH: f32 = 880.0;

impl SettingsScreen {
    pub(crate) fn render_content(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.rows.is_empty() {
            return empty_note(
                if self.query.is_empty() {
                    "Choose a page on the left."
                } else {
                    "No setting matches your search."
                },
                cx,
            )
            .into_any_element();
        }
        div()
            .relative()
            .size_full()
            .child(
                list(
                    self.content.clone(),
                    cx.processor(|this, ix: usize, window, cx| this.render_row(ix, window, cx)),
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(12.0))
                    .child(Scrollbar::vertical(&self.content_scrollbar, &self.content)),
            )
            .into_any_element()
    }

    fn render_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().into_any_element();
        };
        let body = match row {
            ContentRow::PageHeader => self.render_page_header(cx),
            ContentRow::SectionHeader(section) => self.render_section_header(section, cx),
            ContentRow::Setting(section, setting) => {
                self.render_setting(section, setting, window, cx)
            }
        };
        // Center the column, capped at MAX_WIDTH.
        h_flex()
            .w_full()
            .justify_center()
            .child(div().w_full().max_w(px(MAX_WIDTH)).px_8().child(body))
            .into_any_element()
    }

    fn render_page_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some((_, page)) = self.current_page() else {
            return div().into_any_element();
        };
        v_flex()
            .pt_6()
            .pb_2()
            .gap_1()
            .child(
                div()
                    .text_xl()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(page.title.clone()),
            )
            .when_some(page.description.clone(), |this, description| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(description),
                )
            })
            .into_any_element()
    }

    fn render_section_header(&self, section: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let title = self
            .current_page()
            .and_then(|(_, page)| page.sections.get(section))
            .map(|section| section.title.clone())
            .unwrap_or_default();
        div()
            .pt_6()
            .pb_2()
            .border_b_1()
            .border_color(theme.border)
            .text_sm()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme.foreground)
            .child(title)
            .into_any_element()
    }

    fn render_setting(
        &mut self,
        section: usize,
        setting: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(info) = self
            .current_page()
            .and_then(|(_, page)| page.sections.get(section))
            .and_then(|section| section.settings.get(setting))
            .cloned()
        else {
            return div().into_any_element();
        };
        let theme = cx.theme().clone();
        let id = SettingId::of(&info);
        let restart = info.requires_restart();
        let label: SharedString = info
            .label
            .clone()
            .unwrap_or_else(|| info.key.clone())
            .into();
        let changed = store::is_changed(&id);
        let field = self.render_field(&info, window, cx);

        h_flex()
            .w_full()
            .py_3()
            .gap_6()
            .items_start()
            .justify_between()
            .border_b_1()
            .border_color(theme.border.opacity(0.4))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(Label::new(label).text_sm())
                            .when(changed, |this| {
                                this.child(div().size(px(6.0)).rounded_full().bg(theme.primary))
                            }),
                    )
                    .when(!info.description.is_empty(), |this| {
                        this.child(
                            div()
                                .w_full()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(info.description.clone()),
                        )
                    })
                    .when(restart, |this| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(theme.warning)
                                .child("Applies after the editor restarts."),
                        )
                    }),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_1()
                    .items_center()
                    .child(field)
                    .child(div().w(px(24.0)).when(changed, |this| {
                        let id = id.clone();
                        this.child(
                            Button::new(SharedString::from(format!("reset-{}", id.key_string())))
                                .icon(IconName::Undo2)
                                .ghost()
                                .xsmall()
                                .tooltip("Reset to default")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    crate::handlers::reset_setting(this, &id, restart, cx);
                                    cx.notify();
                                })),
                        )
                    })),
            )
            .into_any_element()
    }
}
