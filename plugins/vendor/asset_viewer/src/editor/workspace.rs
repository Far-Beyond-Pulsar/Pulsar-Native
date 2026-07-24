use gpui::*;
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::{DockItem, Panel, PanelEvent, PanelView};
use ui::workspace::Workspace;

use super::panel::AssetViewerPanel;
use super::workspace_panels::ImagePropertiesPanel;

pub struct ImageViewerWorkspace {
    pub workspace: Entity<Workspace>,
    pub viewer: Entity<AssetViewerPanel>,
    pub properties: Entity<ImagePropertiesPanel>,
    pub focus_handle: FocusHandle,
}

impl ImageViewerWorkspace {
    pub fn new(
        file_path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let viewer =
            cx.new(|cx| AssetViewerPanel::new(file_path, window, cx));
        let properties = cx.new(|cx| ImagePropertiesPanel::new(viewer.downgrade(), cx));

        let focus_handle = cx.focus_handle();

        let workspace_entity = cx.new(|cx| {
            let mut ws = Workspace::new("image-viewer-workspace", window, cx);
            let dock_area_weak = ws.dock_area().downgrade();

            let center = DockItem::tabs(
                vec![Arc::new(viewer.clone()) as Arc<dyn PanelView>],
                None,
                &dock_area_weak,
                window,
                cx,
            );

            let right = DockItem::tabs(
                vec![Arc::new(properties.clone()) as Arc<dyn PanelView>],
                None,
                &dock_area_weak,
                window,
                cx,
            );

            ws.initialize(center, None, Some(right), None, window, cx);
            ws
        });

        Self {
            workspace: workspace_entity,
            viewer,
            properties,
            focus_handle,
        }
    }
}

impl EventEmitter<PanelEvent> for ImageViewerWorkspace {}

impl Render for ImageViewerWorkspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_1()
                    .bg(gpui::rgb(0x222222))
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .bg(gpui::rgb(0x555555))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .child("Save")
                            .on_mouse_down(
                                MouseButton::Left,
                                {
                                    let viewer = self.viewer.clone();
                                    move |_event, _window, cx| {
                                        viewer.update(cx, |panel, cx| {
                                            if let Err(e) = panel.save_image() {
                                                log::error!("Save failed: {}", e);
                                            }
                                            cx.notify();
                                        });
                                    }
                                },
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .bg(gpui::rgb(0x555555))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .child("Zoom to Fit")
                            .on_mouse_down(
                                MouseButton::Left,
                                {
                                    let viewer = self.viewer.clone();
                                    move |_event, window, cx| {
                                        viewer.update(cx, |panel, cx| {
                                            panel.zoom_to_fit(window, cx);
                                        });
                                    }
                                },
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.workspace.clone()),
            )
    }
}

impl Focusable for ImageViewerWorkspace {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ImageViewerWorkspace {
    fn panel_name(&self) -> &'static str {
        "Asset Viewer"
    }

    fn panel_file_path(&self, cx: &App) -> Option<PathBuf> {
        self.viewer.read(cx).current_path.clone()
    }

    fn title(&self, _window: &Window, cx: &App) -> AnyElement {
        let name = self
            .viewer
            .read(cx)
            .current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("Asset Viewer")
            .to_string();
        div().text_sm().child(name).into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}
