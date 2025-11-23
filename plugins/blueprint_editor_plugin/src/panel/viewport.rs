//! Viewport operations - pan, zoom, and camera controls

use gpui::*;
use super::core::BlueprintEditorPanel;
use super::super::node_graph::NodeGraphRenderer;

impl BlueprintEditorPanel {
    /// Start panning the viewport
    pub fn start_panning(&mut self, start_pos: Point<f32>, cx: &mut Context<Self>) {
        self.is_panning = true;
        self.pan_start = start_pos;
        self.pan_start_offset = self.graph.pan_offset;
        cx.notify();
    }

    /// Check if currently panning
    pub fn is_panning(&self) -> bool {
        self.is_panning
    }

    /// Update pan position
    pub fn update_pan(&mut self, current_pos: Point<f32>, cx: &mut Context<Self>) {
        if self.is_panning {
            let delta = Point::new(
                current_pos.x - self.pan_start.x,
                current_pos.y - self.pan_start.y,
            );
            self.graph.pan_offset = Point::new(
                self.pan_start_offset.x + delta.x / self.graph.zoom_level,
                self.pan_start_offset.y + delta.y / self.graph.zoom_level,
            );
            cx.notify();
        }
    }

    /// End panning
    pub fn end_panning(&mut self, cx: &mut Context<Self>) {
        self.is_panning = false;
        cx.notify();
    }

    /// Handle zoom with mouse wheel
    /// Zooms around the cursor position to keep the point under cursor fixed
    pub fn handle_zoom(&mut self, delta_y: f32, screen_pos: Point<Pixels>, cx: &mut Context<Self>) {
        let screen: Point<f32> = Point::new(screen_pos.x.into(), screen_pos.y.into());

        // Get graph position under cursor before zoom
        let focus_graph_pos = NodeGraphRenderer::screen_to_graph_pos(
            Point::new(px(screen.x), px(screen.y)),
            &self.graph,
        );

        // Calculate new zoom level (inverted scroll direction)
        let zoom_factor = if delta_y > 0.0 { 1.1 } else { 0.9 };
        let new_zoom = (self.graph.zoom_level * zoom_factor).clamp(0.1, 3.0);

        println!(
            "[ZOOM DEBUG] screen=({},{}), focus_graph=({},{}), old_zoom={}, old_pan=({},{}), delta_y={}",
            screen.x, screen.y,
            focus_graph_pos.x, focus_graph_pos.y,
            self.graph.zoom_level,
            self.graph.pan_offset.x, self.graph.pan_offset.y,
            delta_y
        );

        // Calculate new pan to keep focus point under cursor
        let mut new_pan_offset = Point::new(
            (screen.x / new_zoom) - focus_graph_pos.x,
            (screen.y / new_zoom) - focus_graph_pos.y,
        );

        // Apply temporarily to measure coordinate differences
        let old_zoom = self.graph.zoom_level;
        let old_pan = self.graph.pan_offset;

        self.graph.zoom_level = new_zoom;
        self.graph.pan_offset = new_pan_offset;

        // Measure screen position after zoom
        let screen_after = NodeGraphRenderer::graph_to_screen_pos(focus_graph_pos, &self.graph);
        let diff_x = screen_after.x - screen.x;
        let diff_y = screen_after.y - screen.y;

        // Correct pan to compensate for coordinate system differences
        new_pan_offset.x -= diff_x / new_zoom;
        new_pan_offset.y -= diff_y / new_zoom;

        // Commit corrected values
        self.graph.zoom_level = new_zoom;
        self.graph.pan_offset = new_pan_offset;

        println!(
            "[ZOOM DEBUG] screen_after=({:.2},{:.2}), diff=({:.2},{:.2}), new_zoom={:.3}, new_pan=({:.3},{:.3})",
            screen_after.x, screen_after.y,
            diff_x, diff_y,
            new_zoom,
            new_pan_offset.x, new_pan_offset.y
        );

        cx.notify();
    }
}
