//! Comment operations - drag, resize, edit
//!
//! All operations related to comment box manipulation in the graph.

use gpui::*;
use super::core::{BlueprintEditorPanel, ResizeHandle};
use super::super::BlueprintComment;

impl BlueprintEditorPanel {
    /// Update comment drag position
    pub fn update_comment_drag(&mut self, mouse_pos: Point<f32>, cx: &mut Context<Self>) {
        if let Some(comment_id) = &self.dragging_comment.clone() {
            let new_position = Point::new(
                mouse_pos.x - self.drag_offset.x,
                mouse_pos.y - self.drag_offset.y,
            );

            if let Some(comment) = self.graph.comments.iter_mut().find(|c| c.id == *comment_id) {
                let delta = Point::new(
                    new_position.x - comment.position.x,
                    new_position.y - comment.position.y,
                );

                comment.position = new_position;

                // Move all contained nodes with the comment
                for node_id in &comment.contained_node_ids {
                    if let Some(node) = self.graph.nodes.iter_mut().find(|n| n.id == *node_id) {
                        node.position.x += delta.x;
                        node.position.y += delta.y;
                    }
                }

                cx.notify();
            }
        }
    }

    /// End comment drag
    pub fn end_comment_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(comment_id) = &self.dragging_comment.clone() {
            if let Some(comment) = self.graph.comments.iter_mut().find(|c| c.id == *comment_id) {
                comment.update_contained_nodes(&self.graph.nodes);
            }
        }

        self.dragging_comment = None;
        cx.notify();
    }

    /// Update comment resize
    pub fn update_comment_resize(&mut self, mouse_pos: Point<f32>, cx: &mut Context<Self>) {
        if let Some((comment_id, handle)) = &self.resizing_comment.clone() {
            if let Some(comment) = self.graph.comments.iter_mut().find(|c| c.id == *comment_id) {
                let delta_x = mouse_pos.x - self.drag_offset.x;
                let delta_y = mouse_pos.y - self.drag_offset.y;

                match handle {
                    ResizeHandle::TopLeft => {
                        comment.position.x += delta_x;
                        comment.position.y += delta_y;
                        comment.size.width -= delta_x;
                        comment.size.height -= delta_y;
                    }
                    ResizeHandle::TopRight => {
                        comment.position.y += delta_y;
                        comment.size.width += delta_x;
                        comment.size.height -= delta_y;
                    }
                    ResizeHandle::BottomLeft => {
                        comment.position.x += delta_x;
                        comment.size.width -= delta_x;
                        comment.size.height += delta_y;
                    }
                    ResizeHandle::BottomRight => {
                        comment.size.width += delta_x;
                        comment.size.height += delta_y;
                    }
                    ResizeHandle::Top => {
                        comment.position.y += delta_y;
                        comment.size.height -= delta_y;
                    }
                    ResizeHandle::Bottom => {
                        comment.size.height += delta_y;
                    }
                    ResizeHandle::Left => {
                        comment.position.x += delta_x;
                        comment.size.width -= delta_x;
                    }
                    ResizeHandle::Right => {
                        comment.size.width += delta_x;
                    }
                }

                // Enforce minimum size
                comment.size.width = comment.size.width.max(100.0);
                comment.size.height = comment.size.height.max(50.0);

                self.drag_offset = mouse_pos;
                cx.notify();
            }
        }
    }

    /// End comment resize
    pub fn end_comment_resize(&mut self, cx: &mut Context<Self>) {
        if let Some((comment_id, _)) = &self.resizing_comment.clone() {
            if let Some(comment) = self.graph.comments.iter_mut().find(|c| c.id == *comment_id) {
                comment.update_contained_nodes(&self.graph.nodes);
            }
        }

        self.resizing_comment = None;
        cx.notify();
    }

    /// Finish editing comment text
    pub fn finish_comment_editing(&mut self, cx: &mut Context<Self>) {
        if let Some(comment_id) = &self.editing_comment.clone() {
            let new_text = self.comment_text_input.read(cx).text().to_string();

            if let Some(comment) = self.graph.comments.iter_mut().find(|c| c.id == *comment_id) {
                comment.text = new_text;
            }

            self.editing_comment = None;
            cx.notify();
        }
    }

    /// Create a new comment at center of view
    pub fn create_comment_at_center(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let center_screen = Point::new(1920.0 / 2.0, 1080.0 / 2.0);
        let center_graph = super::super::node_graph::NodeGraphRenderer::screen_to_graph_pos(
            gpui::Point::new(px(center_screen.x), px(center_screen.y)),
            &self.graph,
        );

        let new_comment = BlueprintComment::new(center_graph, window, cx);

        // Subscribe to color picker changes
        if let Some(picker_state) = new_comment.color_picker_state.as_ref() {
            let comment_id = new_comment.id.clone();
            let _ = cx.subscribe_in(
                picker_state,
                window,
                move |this: &mut BlueprintEditorPanel,
                      _picker,
                      event: &ui::color_picker::ColorPickerEvent,
                      _window,
                      cx| {
                    if let ui::color_picker::ColorPickerEvent::Change(Some(color)) = event {
                        if let Some(comment) = this.graph.comments.iter_mut().find(|c| c.id == comment_id) {
                            comment.color = *color;
                            cx.notify();
                        }
                    }
                },
            );
        }

        self.graph.comments.push(new_comment);
        cx.notify();
    }
}
