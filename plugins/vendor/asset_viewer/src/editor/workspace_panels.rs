use gpui::*;
use ui::dock::{Panel, PanelEvent};

use super::panel::AssetViewerPanel;

pub struct ImagePropertiesPanel {
    focus_handle: FocusHandle,
    viewer: WeakEntity<AssetViewerPanel>,
}

impl ImagePropertiesPanel {
    pub fn new(viewer: WeakEntity<AssetViewerPanel>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            viewer,
        }
    }
}

impl EventEmitter<PanelEvent> for ImagePropertiesPanel {}

impl Render for ImagePropertiesPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(viewer) = self.viewer.upgrade() else {
            return div().child("Viewer not available").into_any_element();
        };
        let viewer = viewer.read(cx);
        let (file_name, dims, zoom_val) = if let Some((w, h, _)) = viewer.image_data {
            let name = viewer
                .current_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string();
            (name, format!("{} × {}", w, h), viewer.zoom)
        } else {
            ("No image".to_string(), "—".to_string(), 1.0)
        };
        div()
            .p_4()
            .gap_4()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_sm()
                    .child(format!("File: {}", file_name)),
            )
            .child(
                div()
                    .text_sm()
                    .child(format!("Dimensions: {}", dims)),
            )
            .child(
                div()
                    .text_sm()
                    .child(format!("Zoom: {:.0}%", zoom_val * 100.0)),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .px_3()
                            .py_1()
                            .bg(gpui::rgb(0x444444))
                            .rounded(px(4.0))
                            .cursor_pointer()
                            .child("Reset Zoom")
                            .on_mouse_down(
                                MouseButton::Left,
                                {
                                    let viewer = self.viewer.clone();
                                    move |_event, window, cx| {
                                        if let Some(v) = viewer.upgrade() {
                                            v.update(cx, |panel, cx| {
                                                panel.zoom_to_fit(window, cx);
                                            });
                                        }
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
                            .child("Save")
                            .on_mouse_down(
                                MouseButton::Left,
                                {
                                    let viewer = self.viewer.clone();
                                    move |_event, _window, cx| {
                                        if let Some(v) = viewer.upgrade() {
                                            v.update(cx, |panel, cx| {
                                                if let Err(e) = panel.save_image() {
                                                    log::error!("Save failed: {}", e);
                                                }
                                                cx.notify();
                                            });
                                        }
                                    }
                                },
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for ImagePropertiesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ImagePropertiesPanel {
    fn panel_name(&self) -> &'static str {
        "image-properties"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Properties".into_any_element()
    }
}
