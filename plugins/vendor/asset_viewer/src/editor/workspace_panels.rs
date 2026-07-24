use gpui::*;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{Panel, PanelEvent};
use ui::{h_flex, v_flex, ActiveTheme};

use super::panel::AssetViewerPanel;

pub struct AssetPropertiesPanel {
    editor: WeakEntity<AssetViewerPanel>,
    focus_handle: FocusHandle,
}

impl AssetPropertiesPanel {
    pub fn new(editor: WeakEntity<AssetViewerPanel>, cx: &mut Context<Self>) -> Self {
        Self {
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for AssetPropertiesPanel {}

impl Focusable for AssetPropertiesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for AssetPropertiesPanel {
    fn panel_name(&self) -> &'static str {
        "asset-viewer-properties"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().child("Properties"))
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }
}

impl Render for AssetPropertiesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(editor_entity) = self.editor.upgrade() else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("Editor closed")
                .into_any_element();
        };

        let ew = editor_entity.clone();
        let ew2 = editor_entity.clone();

        editor_entity.update(cx, |editor, cx| {
            let file_name = editor
                .current_path
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown")
                .to_string();

            let dims = if let Some((w, h, _)) = editor.image_data {
                format!("{} x {}", w, h)
            } else if editor.is_3d {
                "3D Model".to_string()
            } else {
                "No image".to_string()
            };

            let zoom_text = format!("Zoom: {:.0}%", editor.zoom * 100.0);

            div()
                .size_full()
                .bg(cx.theme().sidebar)
                .p_3()
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("File"),
                                )
                                .child(div().text_sm().child(file_name)),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Dimensions"),
                                )
                                .child(div().text_sm().child(dims)),
                        )
                        .child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("View"),
                                )
                                .child(div().text_sm().child(zoom_text)),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .child(
                                    Button::new("zoom-to-fit")
                                        .label("Reset Zoom")
                                        .ghost()
                                        .on_click(cx.listener(move |_this, _ev, window, cx| {
                                            ew.update(cx, |e, cx| {
                                                e.zoom_to_fit(window, cx);
                                            });
                                        })),
                                )
                                .child(
                                    Button::new("save-image")
                                        .label("Save")
                                        .primary()
                                        .on_click(cx.listener(move |_this, _ev, _window, cx| {
                                            ew2.update(cx, |e, _cx| {
                                                if let Err(err) = e.save_image() {
                                                    log::error!("Save failed: {}", err);
                                                }
                                            });
                                        })),
                                ),
                        ),
                )
                .into_any_element()
        })
    }
}
