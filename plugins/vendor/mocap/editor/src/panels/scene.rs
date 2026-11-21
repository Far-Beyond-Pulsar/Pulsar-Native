//! The scene panel: the final fused picture of everything captured. Every body,
//! every camera's tracked person, every sensor and every camera in one orbitable
//! 3D view, built by `mocap_capture::build_scene` from all running sources (or
//! from the take at the playhead).

use crate::model::SessionModel;
use crate::paint::{pt, stroke_polyline};
use gpui::prelude::*;
use gpui::*;
use mocap_capture::{FigureSource, Scene};
use ui::dock::{Panel, PanelEvent};
use ui::PixelsExt as _;
use ui::{h_flex, v_flex, ActiveTheme};

/// Metres of stage the view fits at zoom 1.
const VIEW_HEIGHT_M: f32 = 2.6;
/// The orbit pivot: the middle of the stage.
const PIVOT: [f32; 3] = [0.0, 0.9, 0.5];

/// The scene, orbitable.
pub struct ScenePanel {
    model: Entity<SessionModel>,
    focus_handle: FocusHandle,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    drag: Option<Point<Pixels>>,
}

impl ScenePanel {
    pub fn new(model: Entity<SessionModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model, focus_handle: cx.focus_handle(), yaw: 0.45, pitch: 0.2, zoom: 1.0, drag: None }
    }
}

/// Rotates `p` by yaw about Y then pitch about X; returns (x, y, depth).
fn orbit(p: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let x = p[0] * cy + p[2] * sy;
    let z = -p[0] * sy + p[2] * cy;
    [x, p[1] * cp - z * sp, p[1] * sp + z * cp]
}

struct Drawn {
    scene: Scene,
    yaw: f32,
    pitch: f32,
    zoom: f32,
}

struct Palette {
    floor: Hsla,
    bodies: [Hsla; 4],
    axes: [Hsla; 3],
    camera: Hsla,
}

fn finite(p: &[f32; 3]) -> bool {
    p.iter().all(|v| v.is_finite())
}

fn paint_scene(bounds: Bounds<Pixels>, drawn: &Drawn, palette: &Palette, window: &mut Window) {
    let scale = drawn.zoom * bounds.size.height.as_f32() / VIEW_HEIGHT_M;
    let (cx, cy) = (
        bounds.origin.x.as_f32() + bounds.size.width.as_f32() * 0.5,
        bounds.origin.y.as_f32() + bounds.size.height.as_f32() * 0.5,
    );
    let project = |p: [f32; 3]| {
        let o = orbit([p[0] - PIVOT[0], p[1] - PIVOT[1], p[2] - PIVOT[2]], drawn.yaw, drawn.pitch);
        pt(cx + o[0] * scale, cy - o[1] * scale)
    };
    let line = |window: &mut Window, a: [f32; 3], b: [f32; 3], width: f32, color: Hsla| {
        if finite(&a) && finite(&b) {
            stroke_polyline(window, &[project(a), project(b)], width, color);
        }
    };
    for i in -6..=6 {
        let t = i as f32 * 0.5;
        line(window, [t, 0.0, -3.5], [t, 0.0, 3.5], 1.0, palette.floor);
        line(window, [-3.0, 0.0, t * 7.0 / 6.0], [3.0, 0.0, t * 7.0 / 6.0], 1.0, palette.floor);
    }
    for (index, figure) in drawn.scene.figures.iter().enumerate() {
        let color = palette.bodies[index % palette.bodies.len()];
        let opacity = if figure.source == FigureSource::Keypoints { 0.8 } else { 1.0 };
        for &(a, b) in &figure.bones {
            if let (Some(pa), Some(pb)) = (figure.joints.get(a), figure.joints.get(b)) {
                line(window, *pa, *pb, 2.5, color.opacity(opacity));
            }
        }
        for joint in figure.joints.iter().filter(|j| finite(j)) {
            let at = project(*joint);
            window.paint_quad(fill(Bounds::new(point(at.x - px(2.5), at.y - px(2.5)), size(px(5.), px(5.))), color.opacity(opacity)));
        }
    }
    for gizmo in &drawn.scene.axes {
        for (axis, color) in gizmo.axes.iter().zip(palette.axes) {
            let tip = [gizmo.origin[0] + axis[0], gizmo.origin[1] + axis[1], gizmo.origin[2] + axis[2]];
            line(window, gizmo.origin, tip, 3.0, color);
        }
    }
    for camera in &drawn.scene.cameras {
        let (p, f) = (camera.position, camera.forward);
        let centre = [p[0] + f[0] * 0.5, p[1] + f[1] * 0.5, p[2] + f[2] * 0.5];
        let corners = [[-0.3f32, 0.18f32], [0.3, 0.18], [0.3, -0.18], [-0.3, -0.18]].map(|[x, y]| [centre[0] + x, centre[1] + y, centre[2]]);
        for (i, corner) in corners.iter().enumerate() {
            line(window, p, *corner, 1.5, palette.camera);
            line(window, *corner, corners[(i + 1) % 4], 1.5, palette.camera);
        }
    }
}

impl EventEmitter<PanelEvent> for ScenePanel {}

impl Focusable for ScenePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for ScenePanel {
    fn panel_name(&self) -> &'static str {
        "mocap-scene"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().text_sm().child("Scene").into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState { panel_name: self.panel_name().to_string(), ..Default::default() }
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }
}

impl ScenePanel {
    /// One legend row per thing in the scene.
    fn legend(scene: &Scene, palette: &Palette, theme: &ui::Theme) -> Vec<AnyElement> {
        let row = |color: Hsla, text: String| {
            h_flex()
                .gap_1()
                .items_center()
                .child(div().size(px(8.)).bg(color))
                .child(div().text_xs().text_color(theme.foreground).child(text))
                .into_any_element()
        };
        let mut rows = Vec::new();
        for (i, figure) in scene.figures.iter().enumerate() {
            let kind = match figure.source {
                FigureSource::Skeleton => "body",
                FigureSource::Keypoints => "camera keypoints (2D, flat)",
            };
            rows.push(row(palette.bodies[i % palette.bodies.len()], format!("{}: {kind}", figure.label)));
        }
        rows.extend(scene.axes.iter().map(|a| row(palette.axes[0], format!("{}: sensor", a.label))));
        rows.extend(scene.cameras.iter().map(|c| row(palette.camera, format!("{}: camera", c.label))));
        rows
    }
}

impl Render for ScenePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let scene = self.model.read(cx).current_scene();
        let palette = Palette {
            floor: theme.border.opacity(0.6),
            bodies: [theme.info, theme.warning, theme.success, theme.accent],
            axes: [theme.danger, theme.success, theme.info],
            camera: theme.muted_foreground,
        };
        let legend = Self::legend(&scene, &palette, &theme);
        let empty = scene.is_empty();
        let drawn = Drawn { scene, yaw: self.yaw, pitch: self.pitch, zoom: self.zoom };
        v_flex()
            .size_full()
            .relative()
            .bg(theme.background)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _, _| this.drag = Some(event.position)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.drag = None))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let Some(last) = this.drag else { return };
                if !event.dragging() {
                    this.drag = None;
                    return;
                }
                this.yaw += (event.position.x - last.x).as_f32() * 0.01;
                this.pitch = (this.pitch + (event.position.y - last.y).as_f32() * 0.01).clamp(-1.4, 1.4);
                this.drag = Some(event.position);
                cx.notify();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                let dy = match event.delta {
                    ScrollDelta::Pixels(p) => p.y.as_f32(),
                    ScrollDelta::Lines(l) => l.y * 20.0,
                };
                this.zoom = (this.zoom * (1.0 + dy * 0.002)).clamp(0.3, 6.0);
                cx.notify();
            }))
            .child(canvas(|bounds, _, _| bounds, move |_, bounds, window, _| paint_scene(bounds, &drawn, &palette, window)).size_full())
            .child(
                v_flex()
                    .absolute()
                    .top_2()
                    .left_2()
                    .gap_0p5()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(if empty {
                        "Nothing in the scene yet: start a source, or open a take"
                    } else {
                        "Drag to orbit, scroll to zoom"
                    }))
                    .children(legend),
            )
    }
}
