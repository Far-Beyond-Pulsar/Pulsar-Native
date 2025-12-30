use gpui::actions;
use serde::Deserialize;
use schemars::JsonSchema;

// Define actions for the level editor
actions!(
    level_editor,
    [
        // Transform tools
        SelectTool,        // Keyboard: Q
        MoveTool,          // Keyboard: W
        RotateTool,        // Keyboard: E
        ScaleTool,         // Keyboard: R

        // Scene operations
        NewScene,
        OpenScene,
        SaveScene,
        SaveSceneAs,

        // Object operations
        AddObject,
        DeleteObject,
        DuplicateObject,
        RenameObject,
        FocusSelected,     // Keyboard: F - Frame selected object

        // Hierarchy operations
        ExpandAll,
        CollapseAll,

        // Gizmo operations
        ToggleSnapping,          // Keyboard: G - Toggle grid snapping
        ToggleLocalSpace,        // Keyboard: L - Toggle local/world space
        IncreaseSnapIncrement,   // Keyboard: ] - Increase snap grid size
        DecreaseSnapIncrement,   // Keyboard: [ - Decrease snap grid size

        // View operations
        ToggleGrid,
        ToggleWireframe,
        ToggleLighting,
        TogglePerformanceOverlay,
        ToggleCameraModeSelector,
        ToggleViewportOptions,
        Toggle3DRenderer,
        ToggleGpuRenderer,
        ToggleFpsGraphType,

        // Performance metrics toggles
        ToggleFpsGraph,
        ToggleTpsGraph,
        ToggleFrameTimeGraph,
        ToggleMemoryGraph,
        ToggleDrawCallsGraph,
        ToggleVerticesGraph,
        ToggleInputLatencyGraph,
        ToggleUiConsistencyGraph,

        // Camera modes
        PerspectiveView,
        OrthographicView,
        TopView,
        FrontView,
        SideView,

        // Play/Edit mode
        PlayScene,
        StopScene,

        // Edit operations
        Undo,
        Redo,
        Copy,
        Paste,
        Cut,
    ]
);

#[derive(gpui::Action, Clone, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = level_editor)]
pub struct AddObjectOfType {
    pub object_type: String,
}

#[derive(gpui::Action, Clone, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = level_editor)]
pub struct SelectObject {
    pub object_id: String,
}

#[derive(gpui::Action, Clone, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = level_editor)]
pub struct ToggleObjectExpanded {
    pub object_id: String,
}

#[derive(gpui::Action, Clone, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = level_editor)]
pub struct UpdateObjectProperty {
    pub object_id: String,
    pub property: String,
    pub value: String,
}
