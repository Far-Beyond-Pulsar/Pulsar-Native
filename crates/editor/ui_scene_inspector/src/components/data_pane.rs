//! The right pane: the selected component as typed fields, hex or binary.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, IconName, Sizable as _,
};

use crate::components::widgets::{chip, empty_state, MONO};
use crate::screen::{DataTab, SceneInspectorView};
use crate::utils::detail::ComponentData;
use crate::utils::format;
use crate::utils::hex;

impl SceneInspectorView {
    pub(crate) fn render_data_pane(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let pane = v_flex().size_full().bg(theme.background);
        let Some(data) = self.data.clone() else {
            return pane
                .child(empty_state(
                    IconName::Database,
                    "No component to show",
                    "Pick a component to see its fields or its bytes.",
                    cx,
                ))
                .into_any_element();
        };

        // A type nothing describes can only be shown as bytes; the choice of
        // tab is kept for the next type that has fields.
        let tab = self.tab;
        let tabs = [
            (DataTab::Fields, "Fields"),
            (DataTab::Hex, "Hex"),
            (DataTab::Binary, "Binary"),
        ]
        .into_iter()
        .enumerate()
        .map(|(ix, (choice, label))| {
            chip(("data-tab", ix), label, tab == choice, cx, move |this, cx| {
                this.set_tab(choice, cx)
            })
            .into_any_element()
        })
        .collect::<Vec<_>>();

        let body = match tab {
            DataTab::Fields if data.fields.is_some() => self.render_fields(cx),
            DataTab::Fields => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(div().h(px(150.0)).w_full().child(empty_state(
                    IconName::Database,
                    "No reflection for this type",
                    "It registers no component class and no runtime type information. \
                     Its bytes can still be read.",
                    cx,
                )))
                .child(
                    h_flex()
                        .gap_2()
                        .child(chip("open-hex", "View as hex", true, cx, |this, cx| {
                            this.set_tab(DataTab::Hex, cx)
                        }))
                        .child(chip("open-binary", "View as binary", true, cx, |this, cx| {
                            this.set_tab(DataTab::Binary, cx)
                        })),
                )
                .into_any_element(),
            DataTab::Hex | DataTab::Binary => self.render_bytes(cx),
        };

        pane.child(self.data_header(&data, tabs, cx))
            .child(div().flex_1().min_h_0().w_full().child(body))
            .into_any_element()
    }

    fn data_header(
        &self,
        data: &ComponentData,
        tabs: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let source = data
            .fields
            .as_ref()
            .map_or("bytes only", |set| set.source.label());
        let full = data.full;
        let json = data.fields.as_ref().map(|set| set.to_json());
        let bytes = data.bytes.clone();
        v_flex()
            .flex_none()
            .gap_1p5()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child(data.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(MONO)
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(full),
                    )
                    .child(
                        Button::new("copy-type")
                            .ghost()
                            .xsmall()
                            .label("Type name")
                            .icon(IconName::Copy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy(full.to_string(), "the type name", cx)
                            })),
                    )
                    .when_some(json, |header, json| {
                        header.child(
                            Button::new("copy-json")
                                .ghost()
                                .xsmall()
                                .label("JSON")
                                .icon(IconName::Copy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let text = serde_json::to_string_pretty(&json)
                                        .unwrap_or_default();
                                    this.copy(text, "the value as JSON", cx)
                                })),
                        )
                    })
                    .child(
                        Button::new("copy-dump")
                            .ghost()
                            .xsmall()
                            .label("Hex dump")
                            .icon(IconName::Copy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.copy(hex::dump(&bytes, 16), "the value's hex dump", cx)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .children(tabs)
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!(
                                "{} · align {} · {source}",
                                format::bytes(data.bytes.len() as u64),
                                data.align
                            )),
                    ),
            )
            .into_any_element()
    }
}
