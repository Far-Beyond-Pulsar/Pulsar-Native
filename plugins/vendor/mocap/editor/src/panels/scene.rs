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
    centre: Hsla,
    left: Hsla,
    right: Hsla,
    filled: Hsla,
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
    if let Some(subject) = &drawn.scene.subject {
        let figure = &subject.figure;
        let flat = figure.source == FigureSource::Keypoints;
        let opacity = if flat { 0.8 } else { 1.0 };
        let tint = |joint: usize| {
            let name = figure.names.get(joint).map_or("", String::as_str);
            let bone = name.rsplit('/').next().unwrap_or(name);
            if figure.from_keypoints.contains(&joint) {
                palette.filled
            } else if bone.starts_with("Left") || bone.starts_with("left_") {
                palette.left
            } else if bone.starts_with("Right") || bone.starts_with("right_") {
                palette.right
            } else {
                palette.centre
            }
        };
        for &(a, b) in &figure.bones {
            if let (Some(pa), Some(pb)) = (figure.joints.get(a), figure.joints.get(b)) {
                line(window, *pa, *pb, 2.5, tint(b).opacity(opacity));
            }
        }
        for (index, joint) in figure.joints.iter().enumerate().filter(|(_, j)| finite(j)) {
            let at = project(*joint);
            let half = if figure.from_keypoints.contains(&index) { 4.0 } else { 2.5 };
            window.paint_quad(fill(Bounds::new(point(at.x - px(half), at.y - px(half)), size(px(half * 2.0), px(half * 2.0))), tint(index).opacity(opacity)));
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
        let len = (f[0] * f[0] + f[1] * f[1] + f[2] * f[2]).sqrt().max(1e-6);
        let f = [f[0] / len, f[1] / len, f[2] / len];
        // Right and up from the forward direction and the world up.
        let right = [f[2], 0.0, -f[0]];
        let rl = (right[0] * right[0] + right[2] * right[2]).sqrt().max(1e-6);
        let right = [right[0] / rl, 0.0, right[2] / rl];
        let up = [right[1] * f[2] - right[2] * f[1], right[2] * f[0] - right[0] * f[2], right[0] * f[1] - right[1] * f[0]];
        let at = |r: f32, u: f32| [p[0] + f[0] * 0.5 + right[0] * r + up[0] * u, p[1] + f[1] * 0.5 + right[1] * r + up[1] * u, p[2] + f[2] * 0.5 + right[2] * r + up[2] * u];
        let corners = [at(-0.3, 0.18), at(0.3, 0.18), at(0.3, -0.18), at(-0.3, -0.18)];
        let color = if camera.calibrated { palette.filled } else { palette.camera };
        for (i, corner) in corners.iter().enumerate() {
            line(window, p, *corner, 1.5, color);
            line(window, *corner, corners[(i + 1) % 4], 1.5, color);
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
        if let Some(subject) = &scene.subject {
            let kind = match subject.figure.source {
                FigureSource::Skeleton => "fused character",
                FigureSource::Keypoints => "camera keypoints only (2D, flat)",
                FigureSource::Triangulated => "triangulated from the calibrated cameras (3D)",
            };
            rows.push(row(palette.centre, format!("{}: {kind}", subject.label)));
            rows.extend(subject.contributions.iter().map(|c| row(palette.left, format!("  {} <- {}", c.what, c.source))));
            if !subject.figure.from_keypoints.is_empty() {
                rows.push(row(palette.filled, format!("  {} joint(s) filled from the camera", subject.figure.from_keypoints.len())));
            }
        }
        rows.extend(scene.axes.iter().map(|a| row(palette.axes[0], format!("{}: sensor (unassigned)", a.label))));
        rows.extend(scene.cameras.iter().map(|c| {
            let (color, note) = if c.calibrated { (palette.filled, "camera, calibrated position") } else { (palette.camera, "camera (position unknown: not calibrated)") };
            row(color, format!("{}: {note}", c.label))
        }));
        rows
    }
}

impl Render for ScenePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let scene = self.model.read(cx).current_scene();
        let palette = Palette {
            floor: theme.border.opacity(0.6),
            centre: theme.foreground.opacity(0.9),
            left: theme.info,
            right: theme.warning,
            filled: theme.success,
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
