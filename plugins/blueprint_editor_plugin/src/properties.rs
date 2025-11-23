use gpui::*;
use ui::{
    button::ButtonVariants as _,
    h_flex, v_flex,
    ActiveTheme as _, StyledExt, Colorize,
    IconName,
};

use super::*;
use super::panel::BlueprintEditorPanel;

pub struct PropertiesRenderer;

impl PropertiesRenderer {
    pub fn render(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                // STUDIO-QUALITY HEADER (Unreal Details panel style)
                v_flex()
                    .w_full()
                    .child(
                        // Main header with professional styling
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
                                    .text_color(cx.theme().info)
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Details")
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_right()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(if panel.graph.selected_nodes.len() > 1 {
                                                format!("{} items", panel.graph.selected_nodes.len())
                                            } else if panel.graph.selected_nodes.len() == 1 {
                                                "1 item".to_string()
                                            } else {
                                                "None".to_string()
                                            })
                                    )
                            )
                    )
                    .child(
                        // Compact selection type indicator
                        h_flex()
                            .w_full()
                            .px_2()
                            .py_1()
                            .bg(cx.theme().sidebar.darken(0.02))
                            .border_b_1()
                            .border_color(cx.theme().border.opacity(0.2))
                            .items_center()
                            .gap_1p5()
                            .child(
                                ui::Icon::new(if panel.graph.selected_nodes.len() > 1 {
                                    IconName::Copy
                                } else {
                                    IconName::Component
                                })
                                .size(px(12.0))
                                .text_color(cx.theme().info.opacity(0.8))
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(if panel.graph.selected_nodes.len() > 1 {
                                        "Multiple"
                                    } else if panel.graph.selected_nodes.len() == 1 {
                                        "Properties"
                                    } else {
                                        "NO SELECTION"
                                    })
                            )
                            .child(
                                if !panel.graph.selected_nodes.is_empty() {
                                    div()
                                        .px_2()
                                        .py_1()
                                        .rounded(px(4.0))
                                        .bg(cx.theme().info.opacity(0.15))
                                        .text_xs()
                                        .font_family("JetBrainsMono-Regular")
                                        .text_color(cx.theme().info)
                                        .child(format!("{}", panel.graph.selected_nodes.len()))
                                } else {
                                    div() // Empty div when no selection
                                }
                            )
                    )
            )
            .child(
                // CONTENT AREA - clean scrollable content
                v_flex()
                    .flex_1()
                    .overflow_hidden()
                    .p_3()
                    .scrollable(Axis::Vertical)
                    .child(Self::render_properties_content(panel, cx))
            )
    }

    fn render_properties_content(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        // Check if we're inside a macro tab (not the main event graph)
        let is_in_macro = panel.active_tab_index < panel.open_tabs.len() && !panel.open_tabs[panel.active_tab_index].is_main;

        // If in macro and nothing selected, or subgraph_input/output selected, show interface editor
        if is_in_macro {
            let show_interface_editor = if let Some(selected_node_id) = panel.graph.selected_nodes.first() {
                if let Some(selected_node) = panel.graph.nodes.iter().find(|n| n.id == *selected_node_id) {
                    selected_node.definition_id == "subgraph_input" || selected_node.definition_id == "subgraph_output"
                } else {
                    false
                }
            } else {
                // Nothing selected in sub-graph - show interface editor as default
                true
            };

            if show_interface_editor {
                return Self::render_interface_editor(panel, cx);
            }
        }

        if let Some(selected_node_id) = panel.graph.selected_nodes.first() {
            if let Some(selected_node) = panel.graph.nodes.iter().find(|n| n.id == *selected_node_id) {
                v_flex()
                    .gap_4()
                    .child(
                        // Node header with icon and type badge
                        v_flex()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_3()
                                    .child(
                                        div()
                                            .text_2xl()
                                            .child(selected_node.icon.clone())
                                    )
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_bold()
                                            .text_color(cx.theme().foreground)
                                            .child(selected_node.title.clone())
                                    )
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_1()
                                    .rounded(px(4.0))
                                    .bg(Self::get_node_type_color(&selected_node.node_type, cx).opacity(0.15))
                                    .border_1()
                                    .border_color(Self::get_node_type_color(&selected_node.node_type, cx).opacity(0.3))
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(Self::get_node_type_color(&selected_node.node_type, cx))
                                    .child(format!("{:?} Node", selected_node.node_type))
                            )
                    )
                    .child(Self::render_separator(cx))
                    .child(
                        // Properties section
                        v_flex()
                            .gap_3()
                            .child(Self::render_section_header("Properties", IconName::Settings, cx))
                            .child(Self::render_node_properties(selected_node, cx))
                    )
                    .child(Self::render_separator(cx))
                    .child(
                        // Node info section
                        v_flex()
                            .gap_3()
                            .child(Self::render_section_header("Node Info", IconName::Info, cx))
                            .child(Self::render_node_info(selected_node, cx))
                    )
                    .into_any_element()
            } else {
                Self::render_empty_state(cx)
            }
        } else {
            Self::render_empty_state(cx)
        }
    }

    fn render_section_header(title: &str, _icon: IconName, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_bold()
                    .text_color(cx.theme().accent)
                    .child(title.to_uppercase())
            )
    }

    fn render_separator(cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        div()
            .w_full()
            .h_px()
            .bg(cx.theme().border.opacity(0.3))
    }

    fn get_node_type_color(node_type: &NodeType, cx: &mut Context<BlueprintEditorPanel>) -> gpui::Hsla {
        match node_type {
            NodeType::Event => cx.theme().danger,
            NodeType::Logic => cx.theme().primary,
            NodeType::Math => cx.theme().success,
            NodeType::Object => cx.theme().warning,
            NodeType::Reroute => cx.theme().accent,
            NodeType::MacroEntry => gpui::Hsla { h: 0.75, s: 0.7, l: 0.6, a: 1.0 },
            NodeType::MacroExit => gpui::Hsla { h: 0.75, s: 0.7, l: 0.6, a: 1.0 },
            NodeType::MacroInstance => gpui::Hsla { h: 0.75, s: 0.5, l: 0.5, a: 1.0 },
        }
    }

    fn render_empty_state(cx: &mut Context<BlueprintEditorPanel>) -> AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_xl()
                    .child("📋📋📋")
            )
            .child(
                div()
                    .text_sm()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .child("No node selected")
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                    .child("Select a node to view its properties")
            )
            .into_any_element()
    }

    fn render_node_properties(node: &BlueprintNode, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .children(
                node.properties.iter().map(|(key, value)| {
                    Self::render_property_field(key, value, cx)
                })
            )
    }

    fn render_property_field(key: &str, value: &str, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(Self::format_property_name(key))
            )
            .child(
                div()
                    .w_full()
                    .px_3()
                    .py_2p5()
                    .bg(cx.theme().input)
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.6))
                    .rounded(px(6.0))
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(value.to_string())
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .border_color(cx.theme().accent.opacity(0.8))
                            .bg(cx.theme().input.lighten(0.02))
                    })
            )
    }

    fn render_node_info(node: &BlueprintNode, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .gap_2p5()
            .child(Self::render_info_row("Node ID", &node.id, cx))
            .child(Self::render_info_row("Position", &format!("({:.0}, {:.0})", node.position.x, node.position.y), cx))
            .child(Self::render_info_row("Size", &format!("{:.0} × {:.0} px", node.size.width, node.size.height), cx))
            .child(Self::render_separator(cx))
            .child(Self::render_info_row("Input Pins", &node.inputs.len().to_string(), cx))
            .child(Self::render_info_row("Output Pins", &node.outputs.len().to_string(), cx))
    }

    fn render_info_row(label: &str, value: &str, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
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
                    .child(label.to_string())
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
                    .child(value.to_string())
            )
    }

    fn format_property_name(key: &str) -> String {
        // Convert snake_case to Title Case
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

    fn render_interface_editor(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> AnyElement {
        // Find subgraph_input and subgraph_output nodes
        let input_node = panel.graph.nodes.iter().find(|n| n.definition_id == "subgraph_input");
        let output_node = panel.graph.nodes.iter().find(|n| n.definition_id == "subgraph_output");

        v_flex()
            .gap_4()
            .child(
                // Interface header
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .text_2xl()
                                    .child("⚙️")
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_bold()
                                    .text_color(cx.theme().foreground)
                                    .child("Macro Interface")
                            )
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .bg(cx.theme().accent.opacity(0.15))
                            .border_1()
                            .border_color(cx.theme().accent.opacity(0.3))
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().accent)
                            .child("Sub-Graph Interface")
                    )
            )
            .child(Self::render_separator(cx))
            .child(
                // Inputs section
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(Self::render_section_header("Inputs", IconName::ArrowRight, cx))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded(px(4.0))
                                            .bg(cx.theme().success.opacity(0.15))
                                            .text_xs()
                                            .font_family("JetBrainsMono-Regular")
                                            .text_color(cx.theme().success)
                                            .child(format!("{}", input_node.map(|n| n.outputs.len()).unwrap_or(0)))
                                    )
                                    .child(
                                        ui::button::Button::new("add-input-pin")
                                            .icon(IconName::Plus)
                                            .ghost()
                                            .tooltip("Add Input Pin")
                                            .on_click(cx.listener(|panel, _, _, cx| {
                                                panel.add_input_pin(cx);
                                            }))
                                    )
                            )
                    )
                    .child(
                        if let Some(node) = input_node {
                            Self::render_interface_pins(&node.outputs, true, cx)
                        } else {
                            Self::render_no_pins_message("No inputs defined", cx)
                        }
                    )
            )
            .child(Self::render_separator(cx))
            .child(
                // Outputs section
                v_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(Self::render_section_header("Outputs", IconName::ArrowLeft, cx))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded(px(4.0))
                                            .bg(cx.theme().warning.opacity(0.15))
                                            .text_xs()
                                            .font_family("JetBrainsMono-Regular")
                                            .text_color(cx.theme().warning)
                                            .child(format!("{}", output_node.map(|n| n.inputs.len()).unwrap_or(0)))
                                    )
                                    .child(
                                        ui::button::Button::new("add-output-pin")
                                            .icon(IconName::Plus)
                                            .ghost()
                                            .tooltip("Add Output Pin")
                                            .on_click(cx.listener(|panel, _, _, cx| {
                                                panel.add_output_pin(cx);
                                            }))
                                    )
                            )
                    )
                    .child(
                        if let Some(node) = output_node {
                            Self::render_interface_pins(&node.inputs, false, cx)
                        } else {
                            Self::render_no_pins_message("No outputs defined", cx)
                        }
                    )
            )
            .child(Self::render_separator(cx))
            .child(
                // Help text
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().info)
                            .child("ℹ️ Interface Editor")
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("This shows the input and output pins of the macro. To edit the interface, modify the subgraph_input and subgraph_output nodes in the graph.")
                    )
            )
            .into_any_element()
    }

    fn render_interface_pins(pins: &[Pin], is_input: bool, cx: &mut Context<BlueprintEditorPanel>) -> AnyElement {
        if pins.is_empty() {
            return Self::render_no_pins_message(
                if is_input { "No inputs defined" } else { "No outputs defined" },
                cx
            );
        }

        v_flex()
            .gap_2()
            .children(
                pins.iter().map(|pin| {
                    Self::render_interface_pin_row(pin, is_input, cx)
                })
            )
            .into_any_element()
    }

    fn render_interface_pin_row(pin: &Pin, is_input: bool, cx: &mut Context<BlueprintEditorPanel>) -> AnyElement {
        let type_info = ui::graph::TypeInfo::parse(&format!("{:?}", pin.data_type));
        let pin_color = type_info.generate_color();
        let pin_id = pin.id.clone();

        // Generate a unique hash for the button ID
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        pin.id.hash(&mut hasher);
        let pin_hash = hasher.finish() as usize;

        h_flex()
            .w_full()
            .px_3()
            .py_3()
            .gap_3()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border.opacity(0.4))
            .rounded(px(8.0))
            .child(
                // Type color indicator
                div()
                    .flex_shrink_0()
                    .w(px(14.))
                    .h(px(14.))
                    .rounded_full()
                    .bg(gpui::Rgba { r: pin_color.r, g: pin_color.g, b: pin_color.b, a: pin_color.a })
                    .border_2()
                    .border_color(cx.theme().border)
                    .shadow_sm()
            )
            .child(
                // Pin details
                v_flex()
                    .flex_1()
                    .gap_1p5()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(pin.name.clone())
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .bg(cx.theme().muted.opacity(0.2))
                            .text_xs()
                            .font_family("JetBrainsMono-Regular")
                            .text_color(cx.theme().muted_foreground)
                            .child(ui::compiler::type_extractor::get_type_display_name(&format!("{:?}", pin.data_type)))
                    )
            )
            .child(
                // Direction indicator
                div()
                    .text_xs()
                    .text_color(if is_input { cx.theme().success } else { cx.theme().warning })
                    .child(if is_input { "→" } else { "←" })
            )
            .child(
                // Remove button
                ui::button::Button::new(("remove-pin", pin_hash))
                    .icon(IconName::Close)
                    .ghost()
                    .tooltip("Remove Pin")
                    .on_click(cx.listener(move |panel, _, _, cx| {
                        if is_input {
                            panel.remove_input_pin(&pin_id, cx);
                        } else {
                            panel.remove_output_pin(&pin_id, cx);
                        }
                    }))
            )
            .into_any_element()
    }

    fn render_no_pins_message(message: &str, cx: &mut Context<BlueprintEditorPanel>) -> AnyElement {
        div()
            .w_full()
            .px_3()
            .py_4()
            .bg(cx.theme().muted.opacity(0.1))
            .border_1()
            .border_color(cx.theme().border.opacity(0.3))
            .rounded(px(6.0))
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(message.to_string())
            .into_any_element()
    }
}
