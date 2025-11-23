use gpui::Point;

/// Event emitted when the blueprint editor wants to show the node picker
#[derive(Clone)]
pub struct ShowNodePickerRequest {
    pub graph_position: Point<f32>,
}

/// Event emitted when a node is added from the picker
#[derive(Clone)]
pub struct NodeAddedFromPicker {
    pub node_id: String,
}
