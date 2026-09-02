use gpui::prelude::FluentBuilder as _;
use gpui::*;
use rust_i18n::t;
use ui::button::{Button, ButtonVariants as _};
use ui::dock::{Panel, PanelEvent};
use ui::{h_flex, v_flex, ActiveTheme, Disableable};

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
        let ed = editor_entity.read(cx);
        let file_name = ed
            .current_path
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| t!("AssetViewer.Unknown").to_string());
        let is_3d = ed.is_3d;
        let dims = ed
            .image_data
            .as_ref()
            .map(|(w, h, _)| format!("{} x {}", w, h))
            .unwrap_or_default();
        let has_undo = !ed.undo_stack.is_empty();
        let has_redo = !ed.redo_stack.is_empty();
        drop(ed);

        if is_3d {
            return render_3d_table(cx, &file_name, &ew);
        }

        div()
            .size_full()
            .overflow_y_scroll()
            .bg(cx.theme().sidebar)
            .p_3()
            .child(
                v_flex()
                    .gap_4()
                    .child(section(
                        cx,
                        t!("AssetViewer.File").as_ref(),
                        div().text_sm().child(file_name),
                    ))
                    .child(section(
                        cx,
                        t!("AssetViewer.Info").as_ref(),
                        div().text_sm().child(dims),
                    ))
                    .child(section(
                        cx,
                        t!("AssetViewer.History").as_ref(),
                        h_flex()
                            .gap_2()
                            .child(
                                ghost_btn(t!("AssetViewer.Undo").as_ref())
                                    .when(!has_undo, |b| b.disabled(true))
                                    .on_mouse_down(MouseButton::Left, {
                                        let ew = ew.clone();
                                        move |_ev, _window, cx| {
                                            ew.clone().update(cx, |e, _cx| e.undo());
                                        }
                                    }),
                            )
                            .child(
                                ghost_btn(t!("AssetViewer.Redo").as_ref())
                                    .when(!has_redo, |b| b.disabled(true))
                                    .on_mouse_down(MouseButton::Left, {
                                        let ew = ew.clone();
                                        move |_ev, _window, cx| {
                                            ew.clone().update(cx, |e, _cx| e.redo());
                                        }
                                    }),
                            )
                            .child(primary_btn(t!("AssetViewer.Save").as_ref()).on_mouse_down(
                                MouseButton::Left,
                                {
                                    let ew = ew.clone();
                                    move |_ev, _window, cx| {
                                        ew.clone().update(cx, |e, _cx| {
                                            if let Err(err) = e.save_image() {
                                                log::error!("Save: {}", err);
                                            }
                                        });
                                    }
                                },
                            )),
                    ))
                    .child(section(
                        cx,
                        t!("AssetViewer.Transform").as_ref(),
                        h_flex()
                            .gap_2()
                            .child(ghost_btn(t!("AssetViewer.RotCW").as_ref()).on_mouse_down(
                                MouseButton::Left,
                                {
                                    let ew = ew.clone();
                                    move |_ev, _window, cx| {
                                        ew.clone().update(cx, |e, _cx| e.rotate_90());
                                    }
                                },
                            ))
                            .child(ghost_btn(t!("AssetViewer.RotCCW").as_ref()).on_mouse_down(
                                MouseButton::Left,
                                {
                                    let ew = ew.clone();
                                    move |_ev, _window, cx| {
                                        ew.clone().update(cx, |e, _cx| e.rotate_ccw());
                                    }
                                },
                            ))
                            .child(ghost_btn(t!("AssetViewer.FlipH").as_ref()).on_mouse_down(
                                MouseButton::Left,
                                {
                                    let ew = ew.clone();
                                    move |_ev, _window, cx| {
                                        ew.clone().update(cx, |e, _cx| e.flip_h());
                                    }
                                },
                            ))
                            .child(ghost_btn(t!("AssetViewer.FlipV").as_ref()).on_mouse_down(
                                MouseButton::Left,
                                {
                                    let ew = ew.clone();
                                    move |_ev, _window, cx| {
                                        ew.clone().update(cx, |e, _cx| e.flip_v());
                                    }
                                },
                            )),
                    ))
                    .child(section(
                        cx,
                        t!("AssetViewer.Adjust").as_ref(),
                        v_flex()
                            .gap_2()
                            .child(
                                ghost_btn(t!("AssetViewer.Grayscale").as_ref()).on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let ew = ew.clone();
                                        move |_ev, _window, cx| {
                                            ew.clone().update(cx, |e, _cx| e.grayscale());
                                        }
                                    },
                                ),
                            )
                            .child(
                                ghost_btn(t!("AssetViewer.InvertColors").as_ref()).on_mouse_down(
                                    MouseButton::Left,
                                    {
                                        let ew = ew.clone();
                                        move |_ev, _window, cx| {
                                            ew.clone().update(cx, |e, _cx| e.invert());
                                        }
                                    },
                                ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        ghost_btn(t!("AssetViewer.BrightnessUp").as_ref())
                                            .on_mouse_down(MouseButton::Left, {
                                                let ew = ew.clone();
                                                move |_ev, _window, cx| {
                                                    ew.clone().update(cx, |e, _cx| {
                                                        e.adjust_brightness(20)
                                                    });
                                                }
                                            }),
                                    )
                                    .child(
                                        ghost_btn(t!("AssetViewer.BrightnessDown").as_ref())
                                            .on_mouse_down(MouseButton::Left, {
                                                let ew = ew.clone();
                                                move |_ev, _window, cx| {
                                                    ew.clone().update(cx, |e, _cx| {
                                                        e.adjust_brightness(-20)
                                                    });
                                                }
                                            }),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        ghost_btn(t!("AssetViewer.ContrastUp").as_ref())
                                            .on_mouse_down(MouseButton::Left, {
                                                let ew = ew.clone();
                                                move |_ev, _window, cx| {
                                                    ew.clone().update(cx, |e, _cx| {
                                                        e.adjust_contrast(1.3)
                                                    });
                                                }
                                            }),
                                    )
                                    .child(
                                        ghost_btn(t!("AssetViewer.ContrastDown").as_ref())
                                            .on_mouse_down(MouseButton::Left, {
                                                let ew = ew.clone();
                                                move |_ev, _window, cx| {
                                                    ew.clone().update(cx, |e, _cx| {
                                                        e.adjust_contrast(0.7)
                                                    });
                                                }
                                            }),
                                    ),
                            ),
                    ))
                    .child(section(
                        cx,
                        t!("AssetViewer.View").as_ref(),
                        ghost_btn(t!("AssetViewer.ResetZoom").as_ref()).on_mouse_down(
                            MouseButton::Left,
                            {
                                let ew = ew.clone();
                                move |_ev, window, cx| {
                                    ew.clone().update(cx, |e, cx| e.zoom_to_fit(window, cx));
                                }
                            },
                        ),
                    )),
            )
            .into_any_element()
    }
}

fn section<C: IntoElement>(cx: &App, title: &str, content: C) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .gap_2()
        .child(div().text_xs().text_color(muted).child(title.to_string()))
        .child(content)
}

fn info_field(cx: &App, label: &str, value: &str) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .gap_1()
        .child(div().text_xs().text_color(muted).child(label.to_string()))
        .child(div().text_sm().child(value.to_string()))
}

fn render_3d_table(cx: &mut App, file_name: &str, ew: &Entity<AssetViewerPanel>) -> AnyElement {
    let stats = ew.read(cx).scene_stats.clone();
    let props = stats.meshes.clone();

    let cell_w = px(56.0);
    let hdr = |t: &str| div().w(cell_w).child(t.to_string());
    let cel = |t: &str| div().w(cell_w).child(t.to_string());

    div()
        .size_full()
        .overflow_y_scroll()
        .bg(cx.theme().sidebar)
        .p_3()
        .child(
            v_flex()
                .gap_4()
                .child(section(
                    cx,
                    t!("AssetViewer.File").as_ref(),
                    v_flex()
                        .gap_1()
                        .child(div().text_sm().child(file_name.to_string())),
                ))
                .child(section(
                    cx,
                    t!("AssetViewer.Scene").as_ref(),
                    v_flex()
                        .gap_1()
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Meshes").as_ref(),
                            &stats.mesh_count.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Vertices").as_ref(),
                            &stats.total_vertices.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Triangles").as_ref(),
                            &(stats.total_indices / 3).to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Materials").as_ref(),
                            &stats.material_count.to_string(),
                        ))
                        .when(!stats.generator.is_empty(), |el| {
                            el.child(stat_row(
                                cx,
                                t!("AssetViewer.Generator").as_ref(),
                                &stats.generator,
                            ))
                        }),
                ))
                .child(section(
                    cx,
                    t!("AssetViewer.Resources").as_ref(),
                    v_flex()
                        .gap_1()
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Textures").as_ref(),
                            &stats.texture_count.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Images").as_ref(),
                            &stats.image_count.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Lights").as_ref(),
                            &stats.light_count.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Cameras").as_ref(),
                            &stats.camera_count.to_string(),
                        )),
                ))
                .child(section(
                    cx,
                    t!("AssetViewer.Animation").as_ref(),
                    v_flex()
                        .gap_1()
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Clips").as_ref(),
                            &stats.animation_count.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Skins").as_ref(),
                            &stats.skin_count.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.Joints").as_ref(),
                            &stats.total_joints.to_string(),
                        ))
                        .child(stat_row(
                            cx,
                            t!("AssetViewer.MorphTargets").as_ref(),
                            &stats.morph_target_count.to_string(),
                        )),
                ))
                .when(!props.is_empty(), |el| {
                    el.child(section(
                        cx,
                        t!("AssetViewer.Meshes").as_ref(),
                        v_flex()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .py_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(hdr(t!("AssetViewer.ColName").as_ref()))
                                    .child(hdr(t!("AssetViewer.ColVerts").as_ref()))
                                    .child(hdr(t!("AssetViewer.ColTris").as_ref()))
                                    .child(hdr(t!("AssetViewer.ColN").as_ref()))
                                    .child(hdr(t!("AssetViewer.ColT").as_ref()))
                                    .child(hdr(t!("AssetViewer.ColUV").as_ref()))
                                    .child(hdr(t!("AssetViewer.ColC").as_ref()))
                                    .child(hdr(t!("AssetViewer.ColMat").as_ref())),
                            )
                            .child(v_flex().children(props.iter().map(|mp| {
                                h_flex()
                                    .gap_1()
                                    .py_px()
                                    .text_xs()
                                    .child(cel(&mp.name))
                                    .child(cel(&mp.vertex_count.to_string()))
                                    .child(cel(&mp.triangle_count.to_string()))
                                    .child(cel(if mp.has_normals { "Y" } else { "—" }))
                                    .child(cel(if mp.has_tangents { "T" } else { "—" }))
                                    .child(cel(if mp.has_uvs { "Y" } else { "—" }))
                                    .child(cel(if mp.has_vertex_colors { "C" } else { "—" }))
                                    .child(cel(if mp.material_name.is_empty() {
                                        "—"
                                    } else {
                                        &mp.material_name
                                    }))
                                    .into_any_element()
                            }))),
                    ))
                }),
        )
        .into_any_element()
}

fn stat_row(cx: &App, label: &str, value: &str) -> impl IntoElement {
    h_flex()
        .justify_between()
        .child(div().text_sm().child(label.to_string()))
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(value.to_string()),
        )
}

fn ghost_btn(label: &str) -> Button {
    let s = label.to_string();
    Button::new(s.clone()).label(s).ghost()
}

fn primary_btn(label: &str) -> Button {
    let s = label.to_string();
    Button::new(s.clone()).label(s).primary()
}
