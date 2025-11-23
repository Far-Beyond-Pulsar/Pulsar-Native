use gpui::*;
use ui::{
    h_flex, v_flex,
    ActiveTheme as _, StyledExt,
    input::TextInput,
};

use super::{panel::BlueprintEditorPanel, NodeDefinitions};

pub struct NodeLibraryRenderer;

impl NodeLibraryRenderer {
    pub fn render(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .w_full()
            .h_full()
            .min_h_0()
            .gap_2()
            .child(
                div()
                    .w_full()
                    .p_2()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Node Library")
                            )
                            .child(
                                TextInput::new(panel.get_search_input_state())
                            )
                    )
            )
            .child(
                div()
                    .relative()
                    .bg(cx.theme().background)
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .w_full()
                    .h_96()
                    .child(
                        v_flex()
                            .p_2()
                            .id("node-library-content")
                            .scrollable(Axis::Vertical)
                            .gap_3()
                            .child(Self::render_node_categories(panel, cx))
                    )
            )
    }

    fn render_node_categories(panel: &BlueprintEditorPanel, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        let node_definitions = NodeDefinitions::load();
        let search_query = panel.get_search_query().to_lowercase();

        v_flex()
            .gap_3()
            .children(
                node_definitions.categories.iter().filter_map(|category| {
                    // Filter nodes based on search query
                    let filtered_nodes: Vec<_> = if search_query.is_empty() {
                        category.nodes.iter().collect()
                    } else {
                        category.nodes.iter().filter(|node| {
                            node.name.to_lowercase().contains(&search_query) ||
                            node.description.to_lowercase().contains(&search_query) ||
                            category.name.to_lowercase().contains(&search_query)
                        }).collect()
                    };

                    // Only render category if it has matching nodes
                    if filtered_nodes.is_empty() {
                        None
                    } else {
                        // Create a temporary category with filtered nodes
                        let filtered_category = super::NodeCategory {
                            name: category.name.clone(),
                            color: category.color.clone(),
                            nodes: filtered_nodes.into_iter().cloned().collect(),
                        };
                        Some(Self::render_node_category(&category.name, &filtered_category, cx))
                    }
                })
            )
    }

    fn render_node_category(title: &str, category: &crate::tabs::blueprint_editor::NodeCategory, cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(
                div()
                    .p_2()
                    .bg(cx.theme().muted.opacity(0.3))
                    .rounded(px(4.0))
                    .text_sm()
                    .font_semibold()
                    .text_color(cx.theme().foreground)
                    .child(title.to_string())
            )
            .child(
                v_flex()
                    .gap_1()
                    .children(
                        category.nodes.iter().map(|node_def| {
                            let node_name = node_def.name.clone();
                            let node_icon = node_def.icon.clone();
                            let node_id = node_def.id.clone();

                            h_flex()
                                .items_center()
                                .gap_2()
                                .p_2()
                                .rounded(px(4.0))
                                .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
                                .cursor_pointer()
                                .child(node_icon)
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().foreground)
                                        .child(node_name)
                                )
                                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |panel, event: &MouseDownEvent, _window, cx| {
                                    // Create new node from definition at mouse position
                                    let node_definitions = NodeDefinitions::load();
                                    if let Some(definition) = node_definitions.get_node_definition(&node_id) {
                                        let graph_pos = crate::tabs::blueprint_editor::node_graph::NodeGraphRenderer::screen_to_graph_pos(
                                            event.position,
                                            &panel.graph
                                        );
                                        let new_node = crate::tabs::blueprint_editor::BlueprintNode::from_definition(definition, graph_pos);
                                        panel.add_node(new_node, cx);
                                    }
                                }))
                                .into_any_element()
                        })
                    )
            )
    }
}
