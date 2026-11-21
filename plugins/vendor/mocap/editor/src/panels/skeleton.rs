//! The skeleton preview: the live or scrubbed pose, orbit and zoom, drawn on
//! a GPUI canvas.

use crate::model::SessionModel;
use crate::paint::{pt, stroke_polyline};
use gpui::prelude::*;
use gpui::*;
use ui::dock::{Panel, PanelEvent};
use ui::PixelsExt as _;
use ui::{v_flex, ActiveTheme};

/// Metres of figure the view fits at zoom 1 (the figure is about 1.8 m).
const VIEW_HEIGHT_M: f32 = 2.3;
/// The orbit pivot: the figure's belly.
const PIVOT_Y: f32 = 0.9;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Centre,
    Left,
    Right,
}

/// The pose, orbitable.
pub struct SkeletonPanel {
    model: Entity<SessionModel>,
    focus_handle: FocusHandle,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    drag: Option<Point<Pixels>>,
}

impl SkeletonPanel {
    pub fn new(model: Entity<SessionModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { model, focus_handle: cx.focus_handle(), yaw: 0.5, pitch: 0.15, zoom: 1.0, drag: None }
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

struct Scene {
    joints: Vec<[f32; 3]>,
    bones: Vec<(usize, usize)>,
    sides: Vec<Side>,
    yaw: f32,
    pitch: f32,
    zoom: f32,
}

fn paint_scene(bounds: Bounds<Pixels>, scene: &Scene, colors: [Hsla; 4], window: &mut Window) {
    let [floor, centre, left, right] = colors;
    let scale = scene.zoom * bounds.size.height.as_f32() / VIEW_HEIGHT_M;
    let (cx, cy) = (bounds.origin.x.as_f32() + bounds.size.width.as_f32() * 0.5, bounds.origin.y.as_f32() + bounds.size.height.as_f32() * 0.5);
    let project = |p: [f32; 3]| {
        let o = orbit([p[0], p[1] - PIVOT_Y, p[2]], scene.yaw, scene.pitch);
        pt(cx + o[0] * scale, cy - o[1] * scale)
    };
    for i in -4..=4 {
        let t = i as f32 * 0.25;
        stroke_polyline(window, &[project([t, 0.0, -1.0]), project([t, 0.0, 1.0])], 1.0, floor);
        stroke_polyline(window, &[project([-1.0, 0.0, t]), project([1.0, 0.0, t])], 1.0, floor);
    }
    let tint = |j: usize| match scene.sides[j] {
        Side::Centre => centre,
        Side::Left => left,
        Side::Right => right,
    };
    for &(a, b) in &scene.bones {
        stroke_polyline(window, &[project(scene.joints[a]), project(scene.joints[b])], 2.5, tint(b));
    }
    for (j, joint) in scene.joints.iter().enumerate() {
        let at = project(*joint);
        window.paint_quad(fill(Bounds::new(point(at.x - px(2.5), at.y - px(2.5)), size(px(5.), px(5.))), tint(j)));
    }
}

impl EventEmitter<PanelEvent> for SkeletonPanel {}

impl Focusable for SkeletonPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SkeletonPanel {
    fn panel_name(&self) -> &'static str {
        "mocap-skeleton"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().text_sm().child("Skeleton").into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState { panel_name: self.panel_name().to_string(), ..Default::default() }
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for SkeletonPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let model = self.model.read(cx);
        let scene = model.current_pose().map(|pose| Scene {
            joints: model.skeleton.joints(&pose),
            bones: model.skeleton.bones(),
            sides: model
                .rig
                .joints
                .iter()
                .map(|j| {
                    if j.name.starts_with("Left") {
                        Side::Left
                    } else if j.name.starts_with("Right") {
                        Side::Right
                    } else {
                        Side::Centre
                    }
                })
                .collect(),
            yaw: self.yaw,
            pitch: self.pitch,
            zoom: self.zoom,
        });
        let colors = [theme.border.opacity(0.6), theme.foreground.opacity(0.85), theme.info, theme.warning];
        let empty = scene.is_none();
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
            .child(
                canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, window, _| {
                        if let Some(scene) = &scene {
                            paint_scene(bounds, scene, colors, window);
                        }
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .top_2()
                    .left_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if empty { "No pose yet: start a pose source or open a take" } else { "Drag to orbit, scroll to zoom" }),
            )
    }
}
