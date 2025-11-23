use gpui::*;
use ui::{button::{Button, ButtonVariants}, h_flex, ActiveTheme as _, Colorize, Sizable, StyledExt, IconName};

use super::panel::BlueprintEditorPanel;
use super::{BlueprintGraph, BlueprintNode, NodeType};

pub struct MinimapRenderer;

impl MinimapRenderer {
    const MINIMAP_WIDTH: f32 = 200.0;
    const MINIMAP_HEIGHT: f32 = 150.0;
    const MINIMAP_PADDING: f32 = 16.0;

    pub fn render(
        panel: &BlueprintEditorPanel,
        cx: &mut Context<BlueprintEditorPanel>,
    ) -> impl IntoElement {
        // Calculate graph bounds
        let (min_x, min_y, max_x, max_y) = Self::calculate_graph_bounds(&panel.graph);
        
        // Calculate scale to fit all nodes in minimap
        let graph_width = max_x - min_x;
        let graph_height = max_y - min_y;
        
        let scale_x = if graph_width > 0.0 {
            (Self::MINIMAP_WIDTH - Self::MINIMAP_PADDING * 2.0) / graph_width
        } else {
            1.0
        };
        
        let scale_y = if graph_height > 0.0 {
            (Self::MINIMAP_HEIGHT - Self::MINIMAP_PADDING * 2.0) / graph_height
        } else {
            1.0
        };
        
        let scale = scale_x.min(scale_y).min(0.05); // Cap scale for very large graphs

        div()
            .absolute()
            .bottom(px(Self::MINIMAP_PADDING))
            .left(px(Self::MINIMAP_PADDING))
            .w(px(Self::MINIMAP_WIDTH)) // Hardcoded width to prevent inheritance issues
            .h(px(Self::MINIMAP_HEIGHT)) // Hardcoded height
            .bg(cx.theme().background.opacity(0.95))
            .border_2()
            .border_color(cx.theme().border)
            .rounded(px(8.0))
            .shadow_2xl()
            .overflow_hidden()
            .child(
                div()
                    .size_full()
                    .relative()
                    // Background grid pattern
                    .child(Self::render_minimap_grid(cx))
                    // Nodes
                    .children(panel.graph.nodes.iter().map(|node| {
                        Self::render_minimap_node(node, min_x, min_y, scale, &panel.graph, cx)
                    }))
                    // Viewport indicator
                    .child(Self::render_viewport_indicator(
                        &panel.graph,
                        min_x,
                        min_y,
                        scale,
                        cx,
                    ))
                    // Title overlay with close button
                    .child(
                        h_flex()
                            .absolute()
                            .top_2()
                            .left_2()
                            .right_2()
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .bg(cx.theme().secondary.opacity(0.8))
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Minimap")
                            )
                            .child(
                                Button::new("close_minimap")
                                    .icon(IconName::X)
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener(|panel, _, _, cx| {
                                        panel.show_minimap = false;
                                        cx.notify();
                                    }))
                            )
                    )
            )
    }

    fn calculate_graph_bounds(graph: &BlueprintGraph) -> (f32, f32, f32, f32) {
        if graph.nodes.is_empty() {
            return (0.0, 0.0, 1000.0, 1000.0);
        }

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        for node in &graph.nodes {
            min_x = min_x.min(node.position.x);
            min_y = min_y.min(node.position.y);
            max_x = max_x.max(node.position.x + node.size.width);
            max_y = max_y.max(node.position.y + node.size.height);
        }

        // Add padding
        let padding = 100.0;
        (min_x - padding, min_y - padding, max_x + padding, max_y + padding)
    }

    fn render_minimap_grid(cx: &mut Context<BlueprintEditorPanel>) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .bg(cx.theme().muted.opacity(0.05))
    }

    fn render_minimap_node(
        node: &BlueprintNode,
        min_x: f32,
        min_y: f32,
        scale: f32,
        graph: &BlueprintGraph,
        cx: &mut Context<BlueprintEditorPanel>,
    ) -> impl IntoElement {
        // Calculate minimap position
        let x = (node.position.x - min_x) * scale + Self::MINIMAP_PADDING;
        let y = (node.position.y - min_y) * scale + Self::MINIMAP_PADDING;
        let width = (node.size.width * scale).max(2.0);
        let height = (node.size.height * scale).max(2.0);

        // Color based on node type
        let color = match node.node_type {
            NodeType::Event => cx.theme().danger,
            NodeType::Logic => cx.theme().primary,
            NodeType::Math => cx.theme().success,
            NodeType::Object => cx.theme().warning,
            NodeType::Reroute => cx.theme().accent,
            NodeType::MacroEntry => gpui::Hsla { h: 0.75, s: 0.7, l: 0.6, a: 1.0 },
            NodeType::MacroExit => gpui::Hsla { h: 0.75, s: 0.7, l: 0.6, a: 1.0 },
            NodeType::MacroInstance => gpui::Hsla { h: 0.75, s: 0.5, l: 0.5, a: 1.0 },
        };

        let is_selected = graph.selected_nodes.contains(&node.id);

        let mut node_div = div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .h(px(height))
            .bg(color.opacity(if is_selected { 1.0 } else { 0.7 }))
            .rounded(px(1.0));

        if is_selected {
            node_div = node_div.border_1().border_color(gpui::yellow());
        }

        node_div
    }

    fn render_viewport_indicator(
        graph: &BlueprintGraph,
        min_x: f32,
        min_y: f32,
        scale: f32,
        cx: &mut Context<BlueprintEditorPanel>,
    ) -> impl IntoElement {
        // Calculate visible viewport in graph space
        let viewport_width = 1920.0 / graph.zoom_level;
        let viewport_height = 1080.0 / graph.zoom_level;
        
        let viewport_x = -graph.pan_offset.x;
        let viewport_y = -graph.pan_offset.y;

        // Convert to minimap space
        let x = (viewport_x - min_x) * scale + Self::MINIMAP_PADDING;
        let y = (viewport_y - min_y) * scale + Self::MINIMAP_PADDING;
        let width = viewport_width * scale;
        let height = viewport_height * scale;

        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .h(px(height))
            .border_2()
            .border_color(cx.theme().accent.lighten(0.3))
            .bg(cx.theme().accent.opacity(0.1))
            .rounded(px(2.0))
    }
}
