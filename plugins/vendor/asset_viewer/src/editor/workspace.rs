use gpui::*;
use std::sync::Arc;
use ui::dock::{DockChannel, DockItem, PanelEvent};
use ui::workspace::Workspace;
use ui::button::ButtonVariants as _;
use ui::{h_flex, v_flex, ActiveTheme};

use super::panel::AssetViewerPanel;
use super::workspace_panels::AssetPropertiesPanel;

impl AssetViewerPanel {
    pub fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        let ew = cx.entity().downgrade();

        let workspace = cx.new(|cx| {
            Workspace::new_with_channel(
                "asset-viewer-workspace",
                DockChannel(1),
                window,
                cx,
            )
        });

        workspace.update(cx, |workspace, cx| {
            let dock_area_weak = workspace.dock_area().downgrade();

            let viewport =
                cx.new(|cx| ViewportPanel::new(ew.clone(), window, cx));
            let properties =
                cx.new(|cx| AssetPropertiesPanel::new(ew, cx));

            let center = DockItem::tabs(
                vec![Arc::new(viewport) as Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area_weak,
                window,
                cx,
            );

            let right = DockItem::tabs(
                vec![Arc::new(properties) as Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area_weak,
                window,
                cx,
            );

            workspace.initialize(center, None, Some(right), None, window, cx);
        });

        self.workspace = Some(workspace);
    }
}

pub struct ViewportPanel {
    editor: WeakEntity<AssetViewerPanel>,
    focus_handle: FocusHandle,
}

impl ViewportPanel {
    pub fn new(editor: WeakEntity<AssetViewerPanel>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            editor,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for ViewportPanel {}

impl Focusable for ViewportPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ViewportPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(editor) = self.editor.upgrade() {
            editor.update(cx, |editor, cx| {
                editor.render_content(window, cx);

                let surface_elem: gpui::AnyElement =
                    if let Some(surface) = &editor.surface_handle {
                        gpui::wgpu_surface(surface.clone())
                            .defer_resize_until_mouse_up(true)
                            .size_full()
                            .into_any_element()
                    } else {
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(gpui::rgb(0x888888))
                            .child(t!("AssetViewer.Loading"))
                            .into_any_element()
                    };

                if editor.is_3d {
                    div()
                        .size_full()
                        .min_h(px(200.0))
                        .bg(gpui::rgb(0x1a1a1a))
                        .track_focus(&editor.focus_handle)
                        .on_mouse_down(gpui::MouseButton::Right, AssetViewerPanel::on_orbit_mouse_down(cx))
                        .on_mouse_move(AssetViewerPanel::on_orbit_mouse_move(cx))
                        .on_mouse_up(gpui::MouseButton::Right, AssetViewerPanel::on_orbit_mouse_up(cx))
                        .on_mouse_up_out(gpui::MouseButton::Right, AssetViewerPanel::on_orbit_mouse_up(cx))
                        .on_scroll_wheel(AssetViewerPanel::on_orbit_scroll(cx))
                        .on_key_down(AssetViewerPanel::on_key_down(cx))
                        .on_key_up(AssetViewerPanel::on_key_up(cx))
                        .child(surface_elem)
                        .into_any_element()
                } else {
                    div()
                        .size_full()
                        .bg(gpui::rgb(0x1a1a1a))
                        .track_focus(&editor.focus_handle)
                        .on_mouse_down(gpui::MouseButton::Right, AssetViewerPanel::on_pan_mouse_down(cx))
                        .on_mouse_move(AssetViewerPanel::on_pan_mouse_move(cx))
                        .on_mouse_up(gpui::MouseButton::Right, AssetViewerPanel::on_pan_mouse_up(cx))
                        .on_mouse_up_out(gpui::MouseButton::Right, AssetViewerPanel::on_pan_mouse_up(cx))
                        .on_scroll_wheel(AssetViewerPanel::on_image_scroll(cx))
                        .child(surface_elem)
                        .into_any_element()
                }
            })
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("Editor not available")
                .into_any_element()
        }
    }
}

impl ui::dock::Panel for ViewportPanel {
    fn panel_name(&self) -> &'static str {
        "asset-viewer-viewport"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_sm().child("Viewport"))
            .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            ..Default::default()
        }
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }
}
