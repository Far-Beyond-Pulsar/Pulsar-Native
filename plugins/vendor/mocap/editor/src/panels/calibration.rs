//! The camera calibration card in the camera view: it walks the subject through
//! the guided movements, shows what it has seen, and reports the solved cameras
//! and their checks.

use crate::diagrams::Diagram;
use crate::model::{CalibrationMethod, CalibrationPhase, SessionModel};
use gpui::prelude::*;
use gpui::*;
use mocap_capture::{Arrangement, GuideState, Step, MIN_SAMPLES, SAMPLES_TARGET};
use ui::button::{Button, ButtonVariants as _};
use ui::panel_canvas::{DragHandle, PanelCanvasState, PanelRef, RndPanel};
use ui::{h_flex, v_flex, ActiveTheme, Sizable as _};

/// The card's id on the overlay canvas.
pub const CARD_ID: &str = "calibration";

fn action(model: &Entity<SessionModel>, id: &'static str, label: &'static str, run: impl Fn(&mut SessionModel) + 'static) -> Button {
    let model = model.clone();
    Button::new(id).label(label).ghost().small().on_click(move |_, _, cx| {
        model.update(cx, |m, cx| {
            run(m);
            cx.notify();
        });
    })
}

fn checklist(model: &SessionModel, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let guide = &model.calib.guide;
    let done = guide.completed();
    let current = guide.current();
    v_flex().gap_0p5().children(Step::ALL.iter().map(|step| {
        let (mark, color) = if done.contains(step) {
            ("done", theme.success)
        } else if Some(*step) == current {
            ("now ", theme.accent)
        } else {
            ("    ", theme.muted_foreground)
        };
        h_flex()
            .gap_2()
            .text_xs()
            .text_color(color)
            .child(div().w(px(30.)).child(mark))
            .child(div().child(step.name()))
    }))
}

fn guiding(model: &Entity<SessionModel>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let m = model.read(cx);
    let (text, progress, detected) = match m.calib.guide.state() {
        GuideState::Step { step, progress, detected } => (step.instruction(), progress, detected),
        GuideState::Finished => ("Steps done. Keep moving: walk around, turn, reach with both arms", (m.calib.collected as f32 / SAMPLES_TARGET as f32).min(1.0), true),
    };
    let ready = m.calib.collected >= MIN_SAMPLES;
    v_flex()
        .gap_2()
        .p_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(text))
        .child(
            div()
                .h(px(6.))
                .w_full()
                .bg(theme.border)
                .child(div().h_full().w(relative(progress)).bg(if detected { theme.success } else { theme.accent })),
        )
        .child(div().text_xs().text_color(if detected { theme.success } else { theme.muted_foreground }).child(match m.calib.guide.hint() {
            "" if detected => "Got it: keep going".to_owned(),
            "" => "Waiting for the movement. Every camera should see you.".to_owned(),
            hint => hint.to_owned(),
        }))
        .child(checklist(m, cx))
        .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{} samples (it solves at {SAMPLES_TARGET}, or press Solve now from {MIN_SAMPLES})", m.calib.collected)))
        .child(
            h_flex()
                .gap_1()
                .child(action(model, "calib-skip", "Skip step", SessionModel::skip_calibration_step))
                .when(ready, |row| row.child(action(model, "calib-solve", "Solve now", SessionModel::solve_camera_calibration)))
                .child(action(model, "calib-cancel", "Cancel", SessionModel::cancel_camera_calibration)),
        )
        .into_any_element()
}

fn result(model: &Entity<SessionModel>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let m = model.read(cx);
    let checks = m.calib.checks.iter().map(|check| {
        let (mark, color) = if check.passed { ("pass", theme.success) } else { ("FAIL", theme.danger) };
        h_flex()
            .gap_2()
            .text_xs()
            .child(div().w(px(32.)).text_color(color).child(mark))
            .child(div().child(format!("{}: {}", check.name, check.detail)))
    });
    v_flex()
        .gap_2()
        .p_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme.success).child(m.calibration_summary().unwrap_or_default()))
        .when_some(m.camera_rig.as_ref(), |col, rig| {
            let per_camera: Vec<String> = rig.cameras.iter().zip(&rig.per_camera_rms).map(|(c, r)| format!("{} {:.1}px", c.id, r)).collect();
            col.child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                "{} samples, spread {:.0} deg between cameras. {}",
                rig.samples,
                rig.parallax_deg,
                per_camera.join(", ")
            )))
        })
        .children(checks)
        .child(
            h_flex()
                .gap_1()
                .child(action(model, "calib-redo", "Change setup", SessionModel::forget_to_setup))
                .child(action(model, "calib-forget", "Forget", SessionModel::clear_camera_calibration)),
        )
        .into_any_element()
}

fn describe(method: CalibrationMethod) -> (&'static str, &'static str, Diagram) {
    match method {
        CalibrationMethod::Layout(Arrangement::Ring) => ("Ring", "Cameras evenly around you, all aimed at you: the best coverage of the whole body. Enter how far they stand and how high, and they are placed exactly. No movement needed.", Diagram::Ring),
        CalibrationMethod::Layout(Arrangement::Arc) => ("Arc", "Cameras spread over a half circle in front of you, like a desk or a stage. Placed exactly from the distance and height you enter. No movement needed.", Diagram::Arc),
        CalibrationMethod::Layout(Arrangement::Line) => ("Line", "Cameras side by side in a row facing you, like a wall of cameras. Placed exactly from the distance and height you enter. No movement needed.", Diagram::Line),
        CalibrationMethod::Body => ("From your movement", "Cameras anywhere: it works their positions out from you doing a few guided movements. Needs the cameras to see overlapping parts of you, and is the least exact method.", Diagram::Body),
    }
}

fn choose(model: &Entity<SessionModel>, id: &'static str, method: CalibrationMethod, current: CalibrationMethod) -> Button {
    let model = model.clone();
    let mut button = Button::new(id).label(describe(method).0).small();
    button = if method == current { button.primary() } else { button.ghost() };
    button.on_click(move |_, _, cx| {
        model.update(cx, |m, cx| {
            m.set_calibration_method(method);
            cx.notify();
        });
    })
}

fn nudge(model: &Entity<SessionModel>, id: &'static str, label: &'static str, step: (f64, f64, f64)) -> Button {
    let model = model.clone();
    Button::new(id).label(label).ghost().small().on_click(move |_, _, cx| {
        model.update(cx, |m, cx| {
            m.adjust_layout(step.0, step.1, step.2);
            cx.notify();
        });
    })
}

fn setting(model: &Entity<SessionModel>, name: &'static str, value: String, minus: (&'static str, (f64, f64, f64)), plus: (&'static str, (f64, f64, f64)), cx: &App) -> impl IntoElement {
    h_flex()
        .gap_1()
        .items_center()
        .text_xs()
        .child(div().w(px(70.)).text_color(cx.theme().muted_foreground).child(name))
        .child(nudge(model, minus.0, "-", minus.1))
        .child(div().w(px(54.)).text_center().child(value))
        .child(nudge(model, plus.0, "+", plus.1))
}

fn idle(model: &Entity<SessionModel>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let m = model.read(cx);
    let current = m.calib.method;
    let (_, text, diagram) = describe(current);
    let layout = matches!(current, CalibrationMethod::Layout(_));
    let failure = match &m.calib.phase {
        CalibrationPhase::Failed(reason) => Some(reason.clone()),
        _ => None,
    };
    let (distance, height, fov) = (m.calib.distance, m.calib.height, m.calib.hfov_deg);
    v_flex()
        .gap_2()
        .p_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Set up the cameras"))
        .child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .child(choose(model, "m-ring", CalibrationMethod::Layout(Arrangement::Ring), current))
                .child(choose(model, "m-arc", CalibrationMethod::Layout(Arrangement::Arc), current))
                .child(choose(model, "m-line", CalibrationMethod::Layout(Arrangement::Line), current))
                .child(choose(model, "m-body", CalibrationMethod::Body, current)),
        )
        .when_some(diagram.image(), |col, image| col.child(img(image).w_full().h(px(150.)).object_fit(ObjectFit::Contain)))
        .child(div().text_xs().text_color(theme.muted_foreground).child(text))
        .when(layout, |col| {
            col.child(setting(model, "Distance", format!("{distance:.1} m"), ("d-", (-0.25, 0.0, 0.0)), ("d+", (0.25, 0.0, 0.0)), cx))
                .child(setting(model, "Height", format!("{height:.2} m"), ("h-", (0.0, -0.1, 0.0)), ("h+", (0.0, 0.1, 0.0)), cx))
                .child(setting(model, "Field of view", format!("{fov:.0} deg"), ("f-", (0.0, 0.0, -5.0)), ("f+", (0.0, 0.0, 5.0)), cx))
        })
        .when_some(failure, |col, reason| col.child(div().text_xs().text_color(theme.danger).child(reason)))
        .when_some(Diagram::Space.image().filter(|_| layout), |col, image| {
            col.child(div().text_xs().text_color(theme.muted_foreground).child("Keep this space clear around you:")).child(img(image).w_full().h(px(120.)).object_fit(ObjectFit::Contain))
        })
        .child(div().text_xs().text_color(theme.muted_foreground).child("Needs two or more cameras with Track body on. Cameras are assigned in the order of their sources."))
        .child(h_flex().gap_1().child(if layout {
            action(model, "calib-apply", "Place cameras", SessionModel::apply_camera_layout)
        } else {
            action(model, "calib-start", "Start guided calibration", SessionModel::start_camera_calibration)
        }))
        .into_any_element()
}

/// The card, for the overlay canvas `overlay`.
pub fn card(model: &Entity<SessionModel>, overlay: &Entity<PanelCanvasState>, cx: &App) -> RndPanel {
    let theme = cx.theme();
    let panel = PanelRef::new(overlay, CARD_ID);
    let phase = model.read(cx).calib.phase.clone();
    let calibrated = model.read(cx).camera_rig.is_some();
    let body = match phase {
        CalibrationPhase::Guiding => guiding(model, cx),
        CalibrationPhase::Solving => div().p_2().text_sm().child("Solving the camera positions...").into_any_element(),
        CalibrationPhase::Done => result(model, cx),
        CalibrationPhase::Idle if calibrated => result(model, cx),
        _ => idle(model, cx),
    };
    RndPanel::new(panel.clone())
        .bg(theme.background.opacity(0.9))
        .border_1()
        .border_color(theme.border)
        .rounded(theme.radius)
        .shadow_md()
        .child(
            h_flex()
                .size_full()
                .items_start()
                .child(DragHandle::new(panel).w(px(14.)).h_full().border_r_1().border_color(theme.border))
                .child(div().flex_1().child(body)),
        )
        .default_slot("calibration")
        .default_size(size(px(380.), px(640.)))
}
