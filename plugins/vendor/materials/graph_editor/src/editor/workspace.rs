//! Workspace initialization and layout
//!
//! Handles setting up the docking workspace: the canvas in the centre, the
//! compiler output and graph index on the left, the material preview over the
//! details on the right.

use gpui::*;
use std::sync::Arc;
use ui::dock::{DockItem, PanelView};
use ui::workspace::Workspace;

use crate::editor::panel::ShaderEditorPanel;
use crate::editor::workspace_panels::{
    CanvasPanel, CompilerPanel, FindPanel, PreviewPanel, PropertiesPanel,
};

impl ShaderEditorPanel {
    /// Initialize the docking workspace with panels
    pub fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        let editor = cx.entity().downgrade();
        let workspace = cx.new(|cx| {
            Workspace::new_with_channel(
                "shader-editor-workspace",
                ui::dock::DockChannel(1),
                window,
                cx,
            )
        });

        workspace.update(cx, |workspace, cx| {
            let dock = workspace.dock_area().downgrade();

            let canvas = cx.new(|cx| CanvasPanel::new(editor.clone(), cx));
            let compiler = cx.new(|cx| CompilerPanel::new(editor.clone(), cx));
            let find = cx.new(|cx| FindPanel::new(editor.clone(), cx));
            let properties = cx.new(|cx| PropertiesPanel::new(editor.clone(), cx));
            let preview = cx.new(|cx| PreviewPanel::new(editor.clone(), cx));

            let center = DockItem::tabs(
                vec![Arc::new(canvas) as Arc<dyn PanelView>],
                Some(0),
                &dock,
                window,
                cx,
            );
            let left = DockItem::split(
                Axis::Vertical,
                vec![
                    DockItem::tabs(vec![Arc::new(compiler)], None, &dock, window, cx),
                    DockItem::tabs(vec![Arc::new(find)], None, &dock, window, cx),
                ],
                &dock,
                window,
                cx,
            );
            let right = DockItem::split(
                Axis::Vertical,
                vec![
                    DockItem::tabs(vec![Arc::new(preview)], None, &dock, window, cx),
                    DockItem::tabs(vec![Arc::new(properties)], None, &dock, window, cx),
                ],
                &dock,
                window,
                cx,
            );
            workspace.initialize(center, Some(left), Some(right), None, window, cx);
        });

        self.workspace = Some(workspace);
    }
}
