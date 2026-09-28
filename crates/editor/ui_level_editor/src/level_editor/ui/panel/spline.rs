//! Spline editing panel used by the Spline editor layout.

use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    ActiveTheme, IconName, Sizable,
    button::{Button, ButtonVariants as _},
    v_flex,
};

type SharedState = Arc<parking_lot::RwLock<crate::level_editor::state::LevelEditorState>>;

pub(super) struct SplinePanel {
    state: SharedState,
    pump_started: bool,
    last_signature: (Vec<[i32; 3]>, i32),
}

impl SplinePanel {
    pub(super) fn new(state: SharedState, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let last_signature = Self::signature(&state);
        Self {
            state,
            pump_started: false,
            last_signature,
        }
    }

    fn signature(state: &SharedState) -> (Vec<[i32; 3]>, i32) {
        let state = state.read();
        let spline = &state.editor.spline;
        (
            spline
                .points
                .iter()
                .map(|p| p.map(|v| (v * 100.0) as i32))
                .collect(),
            (spline.total_length_m() * 100.0) as i32,
        )
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        crate::level_editor::ui::frame_pump::spawn_frame_pump(
            &cx.entity(),
            window,
            |this, _, cx| {
                let signature = Self::signature(&this.state);
                if signature != this.last_signature {
                    this.last_signature = signature;
                    cx.notify();
                }
            },
        );
    }
}

impl EventEmitter<ui::dock::PanelEvent> for SplinePanel {}
ui_common::panel_boilerplate!(SplinePanel);

impl Render for SplinePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_pump(window, cx);
        let state = self.state.read();
        let spline = &state.editor.spline;
        let points = spline.points.clone();
        let length = spline.total_length_m();
        drop(state);
        let theme = cx.theme().clone();
        let state = self.state.clone();
        let clear = Button::new("spline_clear")
            .icon(IconName::Trash)
            .label(t!("LevelEditor.SplinePanel.Clear"))
            .small()
            .disabled(points.is_empty())
            .on_click(cx.listener(move |_, _, _, cx| {
                state.write().editor.spline.clear();
                cx.notify();
            }));

        v_flex()
            .size_full()
            .gap_2()
            .p_3()
            .overflow_y_scroll()
            .child(
                div()
                    .text_color(theme.foreground())
                    .font_semibold()
                    .child(t!("LevelEditor.SplinePanel.Title")),
            )
            .child(
                div()
                    .text_color(theme.muted_foreground())
                    .child(t!("LevelEditor.SplinePanel.Instructions")),
            )
            .child(div().child(format!(
                "{}: {}",
                t!("LevelEditor.Spline.PointCount"),
                points.len()
            )))
            .child(div().child(format!(
                "{}: {length:.2} m",
                t!("LevelEditor.Spline.Length")
            )))
            .child(clear)
            .children(points.iter().enumerate().map(|(index, point)| {
                div().text_sm().child(format!(
                    "{} {}  ({:.2}, {:.2}, {:.2})",
                    t!("LevelEditor.SplinePanel.Point"),
                    index + 1,
                    point[0],
                    point[1],
                    point[2]
                ))
            }))
    }
}
