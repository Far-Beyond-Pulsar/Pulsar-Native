use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{DockChannel, DockItem, PanelEvent};
use ui::workspace::Workspace;
use ui::{h_flex, v_flex, ActiveTheme, Selectable};
use ui::tooltip::Tooltip;

use super::panel::{AssetViewerPanel, MeshRenderMode};
use super::workspace_panels::AssetPropertiesPanel;

impl AssetViewerPanel {
    pub fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        let ew = cx.entity().downgrade();

        let workspace = cx.new(|cx| {
            Workspace::new_with_channel("asset-viewer-workspace", DockChannel(1), window, cx)
        });

        workspace.update(cx, |workspace, cx| {
            let dock_area_weak = workspace.dock_area().downgrade();

            let viewport = cx.new(|cx| ViewportPanel::new(ew.clone(), window, cx));
            let properties = cx.new(|cx| AssetPropertiesPanel::new(ew, cx));

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
    pub fn new(
        editor: WeakEntity<AssetViewerPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
        if let Some(editor_entity) = self.editor.upgrade() {
            editor_entity.update(cx, |editor, cx| {
                editor.render_content(window, cx);
                // A shader-graph material can animate: keep frames coming
                // so its `time` node advances.
                if editor.graph_draws.iter().any(Option::is_some) {
                    window.request_animation_frame();
                }

                let surface_elem: gpui::AnyElement = if let Some(surface) = &editor.surface_handle {
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
                        .child(t!("AssetViewer.Loading").to_string())
                        .into_any_element()
                };

                if editor.is_3d {
                    let mode = editor.render_mode;
                    let overlay = h_flex()
                        .gap_1()
                        .p_1()
                        .items_center()
                        .bg(cx.theme().background.opacity(0.9))
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(render_mode_button(
                            "asset_view_lit",
                            "Lit",
                            mode == MeshRenderMode::Lit,
                            editor_entity.clone(),
                            MeshRenderMode::Lit,
                        ))
                        .child(render_mode_button(
                            "asset_view_unlit",
                            "Unlit",
                            mode == MeshRenderMode::Unlit,
                            editor_entity.clone(),
                            MeshRenderMode::Unlit,
                        ))
                        .child(render_mode_button(
                            "asset_view_wireframe",
                            "Wireframe",
                            mode == MeshRenderMode::Wireframe,
                            editor_entity.clone(),
                            MeshRenderMode::Wireframe,
                        ))
                        .child(render_mode_button(
                            "asset_view_normals",
                            "Normals",
                            mode == MeshRenderMode::Normals,
                            editor_entity.clone(),
                            MeshRenderMode::Normals,
                        ))
                        .child(render_mode_button(
                            "asset_view_uv0",
                            "UV 1",
                            mode == MeshRenderMode::Uv0,
                            editor_entity.clone(),
                            MeshRenderMode::Uv0,
                        ))
                        .child(render_mode_button(
                            "asset_view_uv1",
                            "UV 2",
                            mode == MeshRenderMode::Uv1,
                            editor_entity.clone(),
                            MeshRenderMode::Uv1,
                        ))
                        .child(render_mode_button(
                            "asset_view_vertex_density",
                            "Density",
                            mode == MeshRenderMode::VertexDensity,
                            editor_entity.clone(),
                            MeshRenderMode::VertexDensity,
                        ));

                    div()
                        .relative()
                        .size_full()
                        .min_h(px(200.0))
                        .bg(gpui::rgb(0x1a1a1a))
                        .track_focus(&editor.focus_handle)
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            AssetViewerPanel::on_orbit_mouse_down(cx),
                        )
                        .on_mouse_move(AssetViewerPanel::on_orbit_mouse_move(cx))
                        .on_mouse_up(
                            gpui::MouseButton::Right,
                            AssetViewerPanel::on_orbit_mouse_up(cx),
                        )
                        .on_mouse_up_out(
                            gpui::MouseButton::Right,
                            AssetViewerPanel::on_orbit_mouse_up(cx),
                        )
                        .on_scroll_wheel(AssetViewerPanel::on_orbit_scroll(cx))
                        .on_key_down(AssetViewerPanel::on_key_down(cx))
                        .on_key_up(AssetViewerPanel::on_key_up(cx))
                        .child(surface_elem)
                        .child(div().absolute().top(px(12.0)).left(px(12.0)).child(overlay))
                        .child(if mode == MeshRenderMode::VertexDensity {
                            v_flex()
                                .absolute()
                                .bottom(px(12.0))
                                .left(px(12.0))
                                .items_center()
                                .gap_1()
                                .px_2()
                                .py_2()
                                .bg(cx.theme().background.opacity(0.9))
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .text_xs()
                                .child("Bad")
                                .child(
                                    v_flex()
                                        .w(px(14.0))
                                        .h(px(72.0))
                                        .children([
                                            (0xf2080a, "red"),
                                            (0xff6100, "orange"),
                                            (0xfff200, "yellow"),
                                            (0x0de61f, "green"),
                                            (0x00edf5, "cyan"),
                                            (0x0d33ff, "blue"),
                                            (0x8c14ff, "violet"),
                                        ].into_iter().enumerate().map(|(index, (color, name))| {
                                            let band = 6 - index as u32;
                                            let tooltip = editor.density_band_ranges.map(|ranges| {
                                                let (low, high) = ranges[band as usize];
                                                format!("{name}: {:.3}–{:.3} vertices / unit²", low, high)
                                            }).unwrap_or_else(|| format!("{name} density"));
                                            let editor_entity = editor_entity.clone();
                                            div()
                                                .flex_1()
                                                .w_full()
                                                .bg(gpui::rgb(color))
                                                .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                                                .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                                                    let selected = if *hovered { Some(band) } else { None };
                                                    let _ = editor_entity.update(cx, |panel, cx| {
                                                        if selected.is_some() || panel.density_hover_band == Some(band) {
                                                            panel.density_hover_band = selected;
                                                            cx.notify();
                                                        }
                                                    });
                                                }))
                                        })),
                                )
                                .child("Good")
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        })
                        .child(if let Some(progress) = editor.density_progress {
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    v_flex()
                                        .w(px(280.0))
                                        .gap_2()
                                        .p_3()
                                        .bg(cx.theme().background.opacity(0.95))
                                        .rounded(cx.theme().radius)
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .child("Computing density of the mesh...")
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(10.0))
                                                .rounded(px(5.0))
                                                .bg(cx.theme().secondary)
                                                .child(
                                                    div()
                                                        .w(px(250.0 * progress.clamp(0.0, 1.0)))
                                                        .h_full()
                                                        .rounded(px(5.0))
                                                        .bg(cx.theme().accent),
                                                ),
                                        )
                                        .child(format!("{}%", (progress * 100.0) as u32)),
                                )
                                .into_any_element()
                        } else if let Some(error) = &editor.density_error {
                            div()
                                .absolute()
                                .bottom(px(12.0))
                                .left(px(12.0))
                                .p_2()
                                .bg(cx.theme().background.opacity(0.95))
                                .text_color(cx.theme().danger)
                                .rounded(cx.theme().radius)
                                .child(error.clone())
                                .into_any_element()
                        } else {
                            div().into_any_element()
                        })
                        .into_any_element()
                } else {
                    div()
                        .size_full()
                        .bg(gpui::rgb(0x1a1a1a))
                        .track_focus(&editor.focus_handle)
                        .on_mouse_down(
                            gpui::MouseButton::Right,
                            AssetViewerPanel::on_pan_mouse_down(cx),
                        )
                        .on_mouse_move(AssetViewerPanel::on_pan_mouse_move(cx))
                        .on_mouse_up(
                            gpui::MouseButton::Right,
                            AssetViewerPanel::on_pan_mouse_up(cx),
                        )
                        .on_mouse_up_out(
                            gpui::MouseButton::Right,
                            AssetViewerPanel::on_pan_mouse_up(cx),
                        )
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

fn render_mode_button(
    id: &'static str,
    label: &'static str,
    selected: bool,
    editor: Entity<AssetViewerPanel>,
    mode: MeshRenderMode,
) -> impl IntoElement {
    Button::new(id)
        .label(label)
        .ghost()
        .selected(selected)
        .on_click(move |_, _, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_render_mode(mode, cx);
            });
        })
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
