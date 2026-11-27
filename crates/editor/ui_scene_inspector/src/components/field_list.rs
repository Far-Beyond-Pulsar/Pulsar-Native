//! The typed fields of a component, one row each.

use gpui::{prelude::*, *};
use pulsar_reflection::prims::style;
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

use style::{INDENT, ROW_HEIGHT};
const MAX_INDENT_DEPTH: u16 = 16;

impl SceneInspectorView {
    pub(crate) fn render_fields(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let list = if self.field_rows.is_empty() {
            let (title, text) = if self.field_filter.trim().is_empty() {
                ("No fields", "This component is a marker: it holds no data.")
            } else {
                (
                    "Nothing matches",
                    "No field name or value contains the filter.",
                )
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
                .child(style::heading(&row.label, theme.muted_foreground))
                .into_any_element();
        }

        let changed = self.changed_fields.contains_key(&row.path);
        let indent = f32::from(row.depth.min(MAX_INDENT_DEPTH)) * INDENT + 8.0;
        let group = SharedString::from("field-row");
        let toggle_path = row.path.clone();
        let copy_path = row.path.clone();
        let expandable = row.open.is_some();

        style::table_row(h_flex().id(("field-row", position)), position % 2 == 1, cx)
            .group(group.clone())
            .w_full()
            .h(px(ROW_HEIGHT))
            .items_center()
            .gap_2()
            .pr_2()
            .pl(px(indent))
            .text_xs()
            .when(changed, |el| el.bg(theme.warning.opacity(0.18)))
            .when(expandable, |el| {
                el.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.toggled.toggle(&toggle_path);
                        this.rebuild_field_rows();
                        cx.notify();
                    }))
            })
            .child(
                div()
                    .flex_none()
                    .w(px(14.0))
                    .when_some(row.open, |el, open| el.child(style::chevron(open, cx))),
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
    match display {
        Display::Category => div().into_any_element(),
        Display::Null => style::null_value(cx).into_any_element(),
        Display::Bool(value) => style::bool_value(*value, cx).into_any_element(),
        Display::Number(text) => style::number_value(text.clone(), cx).into_any_element(),
        Display::Text(text) => style::text_value(text, cx).into_any_element(),
        Display::Variant(name) => style::variant_box(name.clone(), cx).into_any_element(),
        Display::Color(rgba, text) => {
            let channel = |value: f32| value.clamp(0.0, 1.0);
            let swatch: Hsla = Rgba {
                r: channel(rgba[0]),
                g: channel(rgba[1]),
                b: channel(rgba[2]),
                a: channel(rgba[3]),
            }
            .into();
            style::color_value(swatch, text.clone(), cx).into_any_element()
        }
        Display::Vector(items) => style::vector_value(items, cx).into_any_element(),
        Display::Entity(entity) => {
            let entity = *entity;
            if entity == pulsar_scenedb::Entity::DANGLING {
                return style::mono()
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
        Display::Summary(text) => style::summary_value(text.clone(), cx).into_any_element(),
        Display::Error(text) => div()
            .truncate()
            .text_color(theme.danger)
            .child(format!("unreadable: {text}"))
            .into_any_element(),
    }
}
