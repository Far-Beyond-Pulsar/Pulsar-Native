//! The typed fields of a component, one row each.

use gpui::{prelude::*, *};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex,
    input::TextInput,
    v_flex, ActiveTheme, Icon, IconName, Sizable as _,
};

use crate::components::widgets::{empty_state, virtual_list, MONO};
use crate::screen::SceneInspectorView;
use crate::utils::fields::{self, Display, FieldRow};
use crate::utils::format;

const ROW_HEIGHT: f32 = 26.0;
const INDENT: f32 = 16.0;
const MAX_INDENT_DEPTH: u16 = 16;
/// Characters of a string drawn in its row; copying gives all of it.
const MAX_TEXT: usize = 400;

impl SceneInspectorView {
    pub(crate) fn render_fields(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list = if self.field_rows.is_empty() {
            let (title, text) = if self.field_filter.trim().is_empty() {
                ("No fields", "This component is a marker: it holds no data.")
            } else {
                ("Nothing matches", "No field name or value contains the filter.")
            };
            empty_state(IconName::Search, title, text, cx)
        } else {
            virtual_list(
                "scene-fields",
                self.field_rows.len(),
                &self.field_scroll,
                cx,
                |this, range, cx| {
                    range
                        .filter_map(|position| {
                            let row = this.field_rows.get(position)?.clone();
                            Some(this.field_row(position, row, cx))
                        })
                        .collect()
                },
            )
            .into_any_element()
        };

        v_flex()
            .size_full()
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .px_3()
                    .py_1p5()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div().flex_1().child(
                            TextInput::new(&self.field_search)
                                .small()
                                .cleanable()
                                .prefix(Icon::new(IconName::Search).size_3p5()),
                        ),
                    )
                    .when(self.field_search_truncated, |bar| {
                        bar.child(
                            div()
                                .text_xs()
                                .text_color(theme.warning)
                                .child("Too large to search all of it"),
                        )
                    }),
            )
            .child(v_flex().flex_1().min_h_0().child(list))
            .into_any_element()
    }

    fn field_row(&self, position: usize, row: FieldRow, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        if row.display == Display::Category {
            return h_flex()
                .w_full()
                .h(px(ROW_HEIGHT))
                .items_end()
                .px_3()
                .pb_1()
                .border_b_1()
                .border_color(theme.border)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted_foreground)
                .child(row.label.to_uppercase())
                .into_any_element();
        }

        let changed = self.changed_fields.contains_key(&row.path);
        let indent = f32::from(row.depth.min(MAX_INDENT_DEPTH)) * INDENT + 8.0;
        let group = SharedString::from("field-row");
        let toggle_path = row.path.clone();
        let copy_path = row.path.clone();
        let expandable = row.open.is_some();

        h_flex()
            .id(("field-row", position))
            .group(group.clone())
            .w_full()
            .h(px(ROW_HEIGHT))
            .items_center()
            .gap_2()
            .pr_2()
            .pl(px(indent))
            .text_xs()
            .border_b_1()
            .border_color(theme.border.opacity(0.35))
            .when(position % 2 == 1, |el| el.bg(theme.muted.opacity(0.08)))
            .when(changed, |el| el.bg(theme.warning.opacity(0.18)))
            .hover(|el| el.bg(theme.list_hover))
            .when(expandable, |el| {
                el.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggled.toggle(&toggle_path);
                        this.rebuild_field_rows();
                        cx.notify();
                    }))
            })
            .child(
                div().flex_none().w(px(14.0)).when_some(row.open, |el, open| {
                    el.child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size_3()
                        .text_color(theme.muted_foreground),
                    )
                }),
            )
            .child(
                div()
                    .flex_none()
                    .w(px((240.0 - indent).max(90.0)))
                    .truncate()
                    .text_color(theme.foreground)
                    .child(row.label.clone()),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(value_element(position, &row.display, cx)),
            )
            .child(
                div()
                    .flex_none()
                    .max_w(px(220.0))
                    .truncate()
                    .font_family(MONO)
                    .text_color(theme.muted_foreground.opacity(0.8))
                    .child(row.type_label.clone()),
            )
            .child(
                div()
                    .flex_none()
                    .invisible()
                    .group_hover(group, |el| el.visible())
                    .child(
                        Button::new(("copy-field", position))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip("Copy this value")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.copy_field(&copy_path, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The whole value of the row at `path`: a container as JSON.
    fn copy_field(&mut self, path: &str, cx: &mut Context<Self>) {
        let value = self
            .data
            .as_ref()
            .and_then(|data| data.fields.as_ref())
            .and_then(|set| set.value_at(path))
            .map(fields::plain);
        let text = match value {
            Some(serde_json::Value::String(text)) => text,
            Some(value) => serde_json::to_string_pretty(&value).unwrap_or_default(),
            None => {
                self.notify_user("A range has no value of its own to copy".to_string());
                cx.notify();
                return;
            }
        };
        self.copy(text, "the value", cx);
    }
}

fn value_element(
    position: usize,
    display: &Display,
    cx: &mut Context<SceneInspectorView>,
) -> AnyElement {
    let theme = cx.theme().clone();
    let mono = || div().truncate().font_family(MONO);
    match display {
        Display::Category => div().into_any_element(),
        Display::Null => mono()
            .text_color(theme.muted_foreground)
            .italic()
            .child("none")
            .into_any_element(),
        Display::Bool(value) => h_flex()
            .gap_1p5()
            .items_center()
            .child(
                div()
                    .size(px(12.0))
                    .rounded(px(3.0))
                    .border_1()
                    .border_color(if *value { theme.success } else { theme.border })
                    .when(*value, |el| {
                        el.bg(theme.success.opacity(0.25))
                            .child(Icon::new(IconName::Check).size_2p5().text_color(theme.success))
                    }),
            )
            .child(
                mono()
                    .text_color(if *value {
                        theme.success
                    } else {
                        theme.muted_foreground
                    })
                    .child(value.to_string()),
            )
            .into_any_element(),
        Display::Number(text) => mono()
            .text_color(theme.cyan)
            .child(text.clone())
            .into_any_element(),
        Display::Text(text) => {
            let mut shown: String = text.chars().take(MAX_TEXT).collect();
            // A row is one line.
            shown = shown.replace('\n', "↵");
            if text.chars().count() > MAX_TEXT {
                shown.push('…');
            }
            mono()
                .text_color(theme.green)
                .child(format!("\"{shown}\""))
                .into_any_element()
        }
        Display::Variant(name) => div()
            .px_1p5()
            .rounded(px(4.0))
            .bg(theme.magenta.opacity(0.16))
            .font_family(MONO)
            .text_color(theme.magenta)
            .child(name.clone())
            .into_any_element(),
        Display::Color(rgba, text) => {
            let channel = |value: f32| value.clamp(0.0, 1.0);
            let swatch: Hsla = Rgba {
                r: channel(rgba[0]),
                g: channel(rgba[1]),
                b: channel(rgba[2]),
                a: channel(rgba[3]),
            }
            .into();
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .flex_none()
                        .w(px(28.0))
                        .h(px(14.0))
                        .rounded(px(3.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(swatch),
                )
                .child(mono().text_color(theme.foreground).child(text.clone()))
                .into_any_element()
        }
        Display::Vector(items) => {
            let axes = [theme.red, theme.green, theme.blue, theme.yellow];
            let names = ["x", "y", "z", "w"];
            h_flex()
                .gap_1()
                .children(items.iter().zip(axes.iter().zip(names)).map(
                    |(item, (color, name))| {
                        h_flex()
                            .gap_1()
                            .px_1p5()
                            .min_w(px(64.0))
                            .rounded(px(3.0))
                            .bg(theme.muted.opacity(0.3))
                            .font_family(MONO)
                            .child(div().text_color(*color).child(name))
                            .child(div().text_color(theme.foreground).child(item.clone()))
                    },
                ))
                .into_any_element()
        }
        Display::Entity(entity) => {
            let entity = *entity;
            if entity == pulsar_scenedb::Entity::DANGLING {
                return mono()
                    .text_color(theme.muted_foreground)
                    .child("no entity")
                    .into_any_element();
            }
            div()
                .id(("field-link", position))
                .cursor_pointer()
                .font_family(MONO)
                .text_color(theme.link)
                .hover(|el| el.underline())
                .child(format!("→ {}", format::entity(entity)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.follow_link(entity, cx);
                }))
                .into_any_element()
        }
        Display::Summary(text) => div()
            .truncate()
            .text_color(theme.muted_foreground)
            .child(text.clone())
            .into_any_element(),
        Display::Error(text) => div()
            .truncate()
            .text_color(theme.danger)
            .child(format!("unreadable: {text}"))
            .into_any_element(),
    }
}
