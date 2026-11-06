//! Details panel: the properties of the selected node or comment.
//!
//! Reads the selection and the document from the canvas and writes edits back
//! through the editor as graph commands, so every change is one undo step. The
//! shader-specific controls live here: reflected property widgets for pin default
//! values (colours, textures, vectors), plain text for the basic WGSL types, and
//! text fields for a node's other properties.

use gpui::prelude::FluentBuilder;
use gpui::*;
use graphy::{DataType, NodeInstance, PinInstance};
use pulsar_reflection::RUNTIME_TYPE_REGISTRY;
use serde_json::Value;
use std::any::Any;
use std::sync::Arc;
use ui::input::{InputEvent, InputState, TextInput};
use ui::{h_flex, v_flex, ActiveTheme as _, IconName, StyledExt};
use ui_common::reflected_properties_panel::rgba_to_hsla;

use crate::definitions::NodeDefinitions;
use crate::editor::panel::ShaderEditorPanel;
use crate::profile::text_to_value;
use crate::types::type_name;

/// Renderer for the details panel
pub struct PropertiesRenderer;

impl PropertiesRenderer {
    pub fn render(
        panel: &mut ShaderEditorPanel,
        window: &mut Window,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> impl IntoElement {
        let (nodes, comments) = {
            let selection = &panel.canvas.read(cx).controller().selection;
            (
                selection.nodes.iter().cloned().collect::<Vec<_>>(),
                selection.comments.iter().copied().collect::<Vec<_>>(),
            )
        };
        let count = nodes.len() + comments.len();
        let content = Self::render_properties_content(panel, &nodes, &comments, window, cx);
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .bg(cx.theme().secondary)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .gap_2()
                    .child(
                        ui::Icon::new(IconName::Settings)
                            .size(px(16.0))
                            .text_color(cx.theme().info),
                    )
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Details"),
                    )
                    .child(
                        div().flex_1().text_right().child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(match count {
                                    0 => "None".to_string(),
                                    1 => "1 item".to_string(),
                                    n => format!("{n} items"),
                                }),
                        ),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .p_3()
                    .scrollable(Axis::Vertical)
                    .child(content),
            )
    }

    fn render_properties_content(
        panel: &mut ShaderEditorPanel,
        nodes: &[String],
        comments: &[usize],
        window: &mut Window,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> AnyElement {
        match (nodes, comments) {
            ([id], []) => {
                let node = panel.canvas.read(cx).graph().nodes.get(id).cloned();
                match node {
                    Some(node) => Self::render_node(panel, &node, window, cx),
                    // Stale selection pointing at a node that no longer exists.
                    None => Self::render_empty_state(cx),
                }
            }
            ([], [index]) => Self::render_comment(panel, *index, window, cx),
            ([], []) => Self::render_empty_state(cx),
            (nodes, comments) => Self::render_multi_selection_state(nodes.len(), comments.len(), cx),
        }
    }

    /// A text input kept in step with `current` while it is not being typed in,
    /// which sends edits to `apply`.
    fn text_input(
        panel: &mut ShaderEditorPanel,
        key: String,
        current: &str,
        placeholder: &'static str,
        apply: impl Fn(&mut ShaderEditorPanel, String, &mut Context<ShaderEditorPanel>) + 'static,
        window: &mut Window,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> Entity<InputState> {
        if let Some(existing) = panel.detail_inputs.get(&key) {
            let existing = existing.clone();
            let focused = existing.read(cx).focus_handle(cx).is_focused(window);
            if !focused && existing.read(cx).text().to_string() != current {
                existing.update(cx, |input, cx| input.set_value(current.to_string(), window, cx));
            }
            return existing;
        }
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder(placeholder);
            input.set_value(current.to_string(), window, cx);
            input
        });
        cx.subscribe_in(
            &input,
            window,
            move |this, state: &Entity<InputState>, event: &InputEvent, _window, cx| {
                if matches!(event, InputEvent::Change | InputEvent::Blur | InputEvent::PressEnter { .. }) {
                    let text = state.read(cx).text().to_string();
                    apply(this, text, cx);
                }
            },
        )
        .detach();
        panel.detail_inputs.insert(key, input.clone());
        input
    }

    fn render_node(
        panel: &mut ShaderEditorPanel,
        node: &NodeInstance,
        window: &mut Window,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> AnyElement {
        let definitions = NodeDefinitions::load();
        let definition = definitions.get_node_definition(&node.node_type);
        let title = definition.map_or_else(|| node.node_type.replace('_', " "), |d| d.name.clone());
        let category = definitions
            .get_category_for_node(&node.node_type)
            .map_or("Node".to_string(), |c| c.name.clone());
        let (x, y) = (node.position.x, node.position.y);

        let pin_ids: Vec<&str> = node.inputs.iter().map(|p| p.id.as_str()).collect();
        let mut other_properties: Vec<(String, Value)> = node
            .properties
            .iter()
            .filter(|(key, _)| !pin_ids.contains(&key.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        other_properties.sort_by(|a, b| a.0.cmp(&b.0));

        let connected_inputs: Vec<String> = panel
            .canvas
            .read(cx)
            .graph()
            .connections
            .iter()
            .filter(|c| c.target_node == node.id)
            .map(|c| c.target_pin.clone())
            .collect();

        let mut input_rows = Vec::new();
        for pin in &node.inputs {
            let connected = connected_inputs.contains(&pin.id);
            input_rows.push(Self::render_input_pin_row(panel, node, pin, connected, window, cx));
        }
        let mut property_rows = Vec::new();
        for (key, value) in &other_properties {
            property_rows.push(Self::render_property_field(panel, node, key, value, window, cx));
        }

        v_flex()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font_bold()
                            .text_color(cx.theme().foreground)
                            .child(title),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .bg(cx.theme().primary.opacity(0.15))
                            .border_1()
                            .border_color(cx.theme().primary.opacity(0.3))
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().primary)
                            .child(format!("{category} Node")),
                    ),
            )
            .when(!input_rows.is_empty(), |el| {
                el.child(Self::render_separator(cx)).child(
                    v_flex()
                        .gap_3()
                        .child(Self::render_section_header("Inputs", cx))
                        .child(v_flex().gap_1p5().children(input_rows)),
                )
            })
            .when(!node.outputs.is_empty(), |el| {
                el.child(Self::render_separator(cx)).child(
                    v_flex()
                        .gap_3()
                        .child(Self::render_section_header("Outputs", cx))
                        .child(
                            v_flex()
                                .gap_1p5()
                                .children(node.outputs.iter().map(|pin| Self::render_pin_row(pin, cx))),
                        ),
                )
            })
            .when(!property_rows.is_empty(), |el| {
                el.child(Self::render_separator(cx)).child(
                    v_flex()
                        .gap_3()
                        .child(Self::render_section_header("Properties", cx))
                        .child(v_flex().gap_3().children(property_rows)),
                )
            })
            .child(Self::render_separator(cx))
            .child(
                v_flex()
                    .gap_3()
                    .child(Self::render_section_header("Node Info", cx))
                    .child(Self::render_info_row("Node ID", &node.id, cx))
                    .child(Self::render_info_row("Type", &node.node_type, cx))
                    .child(Self::render_info_row("Position", &format!("({x:.0}, {y:.0})"), cx)),
            )
            .into_any_element()
    }

    fn render_comment(
        panel: &mut ShaderEditorPanel,
        index: usize,
        window: &mut Window,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> AnyElement {
        let Some(comment) = panel.canvas.read(cx).graph().comments.get(index).cloned() else {
            return Self::render_empty_state(cx);
        };
        let [r, g, b, a] = comment.color;
        let swatch = gpui::Rgba { r, g, b, a: a.max(0.35) };
        let key = format!("comment#{}", comment.id);
        let input = Self::text_input(
            panel,
            key,
            &comment.text,
            "Comment text...",
            move |this, text, cx| this.set_comment_text(index, text, cx),
            window,
            cx,
        );
        v_flex()
            .gap_4()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .child(
                        ui::Icon::new(IconName::Info)
                            .size(px(18.0))
                            .text_color(cx.theme().info),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_bold()
                            .text_color(cx.theme().foreground)
                            .child(comment.text.clone()),
                    ),
            )
            .child(Self::render_separator(cx))
            .child(
                v_flex()
                    .gap_3()
                    .child(Self::render_section_header("Comment Properties", cx))
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Name"),
                            )
                            .child(div().w_full().child(TextInput::new(&input))),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Color"),
                            )
                            .child(
                                div()
                                    .w(px(24.0))
                                    .h(px(24.0))
                                    .rounded(px(4.0))
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(gpui::Hsla::from(swatch)),
                            ),
                    ),
            )
            .child(Self::render_separator(cx))
            .child(
                v_flex()
                    .gap_3()
                    .child(Self::render_section_header("Comment Info", cx))
                    .child(Self::render_info_row("Comment ID", &comment.id, cx))
                    .child(Self::render_info_row(
                        "Position",
                        &format!("({:.0}, {:.0})", comment.position.x, comment.position.y),
                        cx,
                    ))
                    .child(Self::render_info_row(
                        "Size",
                        &format!("{:.0} × {:.0} px", comment.size.0, comment.size.1),
                        cx,
                    ))
                    .child(Self::render_info_row(
                        "Contained Nodes",
                        &comment.contained_node_ids.len().to_string(),
                        cx,
                    )),
            )
            .into_any_element()
    }

    fn render_multi_selection_state(
        node_count: usize,
        comment_count: usize,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> AnyElement {
        let summary = match (node_count, comment_count) {
            (n, 0) => format!("{} nodes selected", n),
            (0, c) => format!("{} comments selected", c),
            (n, c) => format!("{} nodes, {} comments selected", n, c),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                ui::Icon::new(IconName::Copy)
                    .size(px(20.0))
                    .text_color(cx.theme().muted_foreground.opacity(0.5)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(summary),
            )
            .into_any_element()
    }

    fn render_section_header(title: &str, cx: &mut Context<ShaderEditorPanel>) -> impl IntoElement {
        h_flex().items_center().gap_2().child(
            div()
                .text_xs()
                .font_bold()
                .text_color(cx.theme().foreground)
                .child(title.to_uppercase()),
        )
    }

    fn render_separator(cx: &mut Context<ShaderEditorPanel>) -> impl IntoElement {
        div().w_full().h_px().bg(cx.theme().border.opacity(0.3))
    }

    /// Compact, centered placeholder shown when there's nothing to inspect.
    fn render_empty_state(cx: &mut Context<ShaderEditorPanel>) -> AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(
                ui::Icon::new(IconName::Component)
                    .size(px(20.0))
                    .text_color(cx.theme().muted_foreground.opacity(0.5)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Select a node to view its properties"),
            )
            .into_any_element()
    }

    /// An input pin with the control that edits its default value, unless a wire
    /// feeds it. Reflected types get their registered widget (colour picker,
    /// texture slot, vector fields); the basic WGSL types get a text field.
    fn render_input_pin_row(
        panel: &mut ShaderEditorPanel,
        node: &NodeInstance,
        pin: &PinInstance,
        connected: bool,
        window: &mut Window,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> AnyElement {
        let row = Self::render_pin_row(pin, cx);
        if connected {
            return row.into_any_element();
        }
        let ty = pin.data_type();
        let name = type_name(ty);

        let Some(type_info) = runtime_type(ty) else {
            if !matches!(
                name,
                "f32" | "vec2<f32>" | "vec3<f32>" | "vec4<f32>" | "int" | "uint" | "bool" | "mat4x4<f32>"
            ) {
                return row.into_any_element();
            }
            let stored = node
                .properties
                .get(&pin.id)
                .map(property_text)
                .unwrap_or_default();
            let (node_id, pin_id) = (node.id.clone(), pin.id.clone());
            let input = Self::text_input(
                panel,
                format!("{}#{}", node.id, pin.id),
                &stored,
                "value",
                move |this, text, cx| this.set_node_property(&node_id, &pin_id, text_to_value(&text), cx),
                window,
                cx,
            );
            return h_flex().gap_1().child(row).child(TextInput::new(&input)).into_any_element();
        };

        let state_key = format!("{}#{}", node.id, pin.id);
        let current_json = node.properties.get(&pin.id).cloned().unwrap_or(Value::Null);
        let current_any: Box<dyn Any> = if current_json.is_null() {
            Box::new(())
        } else {
            RUNTIME_TYPE_REGISTRY
                .deserialize_json_for_type(type_info, current_json)
                .unwrap_or_else(|_| Box::new(()))
        };

        let editor = cx.entity();
        let (node_id, pin_id) = (node.id.clone(), pin.id.clone());
        let write_back = Arc::new(
            move |new_val: Box<dyn Any + Send>, _window: &mut Window, cx: &mut App| {
                if let Ok(json) = RUNTIME_TYPE_REGISTRY.serialize_json_for_any(new_val.as_ref()) {
                    let value = if json.is_null() { None } else { Some(json) };
                    editor.update(cx, |editor, cx| {
                        editor.set_node_property(&node_id, &pin_id, value, cx)
                    });
                }
            },
        );

        let widget = ui_common::render_property_row_runtime(
            &mut panel.pin_property_state,
            "node-input",
            &state_key,
            &format_property_name(&pin.pin.name),
            &pin.id,
            &pin.pin.name,
            type_info,
            current_any.as_ref(),
            write_back,
            window,
            cx,
        );

        v_flex().gap_1p5().child(row).child(widget).into_any_element()
    }

    fn render_pin_row(pin: &PinInstance, cx: &mut Context<ShaderEditorPanel>) -> impl IntoElement {
        let ty = pin.data_type();
        let badge_color: gpui::Hsla = {
            let [r, g, b, a] = crate::types::pin_color(ty);
            rgba_to_hsla([r, g, b, a]).into()
        };
        let type_label = if ty.is_execution() {
            "Execution".to_string()
        } else if matches!(type_name(ty), "?" | "_") {
            "Wildcard".to_string()
        } else {
            type_name(ty).to_string()
        };
        let name = if pin.pin.name.is_empty() {
            "(unnamed)".to_string()
        } else {
            pin.pin.name.clone()
        };

        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .px_3()
            .py_2()
            .rounded(px(4.0))
            .hover(|style| style.bg(cx.theme().muted.opacity(0.1)))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().size(px(8.0)).rounded_full().bg(badge_color))
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().foreground)
                            .child(name),
                    ),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(4.0))
                    .bg(badge_color.opacity(0.15))
                    .border_1()
                    .border_color(badge_color.opacity(0.4))
                    .text_xs()
                    .font_family("JetBrainsMono-Regular")
                    .text_color(badge_color)
                    .child(type_label),
            )
    }

    /// A property of the node that is not a pin default (a constant's value).
    fn render_property_field(
        panel: &mut ShaderEditorPanel,
        node: &NodeInstance,
        key: &str,
        value: &Value,
        window: &mut Window,
        cx: &mut Context<ShaderEditorPanel>,
    ) -> AnyElement {
        let (node_id, prop) = (node.id.clone(), key.to_string());
        let input = Self::text_input(
            panel,
            format!("{}#prop:{}", node.id, key),
            &property_text(value),
            "value",
            move |this, text, cx| {
                // A blank property is kept blank rather than removed: it is part of the node.
                let value = text_to_value(&text).unwrap_or(Value::String(String::new()));
                this.set_node_property(&node_id, &prop, Some(value), cx)
            },
            window,
            cx,
        );
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_property_name(key)),
            )
            .child(div().w_full().child(TextInput::new(&input)))
            .into_any_element()
    }

    fn render_info_row(label: &str, value: &str, cx: &mut Context<ShaderEditorPanel>) -> impl IntoElement {
        h_flex()
            .w_full()
            .justify_between()
            .items_center()
            .px_3()
            .py_2()
            .rounded(px(4.0))
            .hover(|style| style.bg(cx.theme().muted.opacity(0.1)))
            .child(
                div()
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(4.0))
                    .bg(cx.theme().muted.opacity(0.2))
                    .text_xs()
                    .font_family("JetBrainsMono-Regular")
                    .text_color(cx.theme().foreground)
                    .child(value.to_string()),
            )
    }
}

/// The registered reflection info for a pin's type, for pins whose value is
/// edited with a reflected widget. `None` for execution, wildcard and the basic
/// WGSL types the registry does not know.
fn runtime_type(ty: &DataType) -> Option<&'static pulsar_reflection::RuntimeTypeInfo> {
    let name = type_name(ty);
    if ty.is_execution() || matches!(name, "?" | "_") || name.len() == 1 {
        return None;
    }
    RUNTIME_TYPE_REGISTRY.get_by_name(name)
}

/// A property value as the text field shows it.
fn property_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `snake_case` to `Title Case`.
fn format_property_name(key: &str) -> String {
    key.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}
