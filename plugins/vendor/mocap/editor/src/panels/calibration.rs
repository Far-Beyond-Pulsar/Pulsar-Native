//! The camera calibration card in the camera view: it walks the subject through
//! the guided movements, shows what it has seen, and reports the solved cameras
//! and their checks.

use crate::model::{CalibrationPhase, SessionModel};
use gpui::prelude::*;
use gpui::*;
use mocap_capture::{GuideState, Step, MIN_SAMPLES, SAMPLES_TARGET};
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
                .child(action(model, "calib-redo", "Recalibrate", SessionModel::start_camera_calibration))
                .child(action(model, "calib-forget", "Forget", SessionModel::clear_camera_calibration)),
        )
        .into_any_element()
}

fn idle(model: &Entity<SessionModel>, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let m = model.read(cx);
    let (headline, hint) = match &m.calib.phase {
        CalibrationPhase::Failed(reason) => ("Calibration failed".to_owned(), reason.clone()),
        _ => (
            "Cameras not calibrated".to_owned(),
            "Turn on Track body for two or more cameras, then calibrate: you will be asked to do a few movements, and the cameras' positions around you are worked out from them.".to_owned(),
        ),
    };
    v_flex()
        .gap_2()
        .p_2()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(headline))
        .child(div().text_xs().text_color(theme.muted_foreground).child(hint))
        .child(h_flex().gap_1().child(action(model, "calib-start", "Calibrate cameras", SessionModel::start_camera_calibration)))
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
        .default_size(size(px(360.), px(300.)))
}
