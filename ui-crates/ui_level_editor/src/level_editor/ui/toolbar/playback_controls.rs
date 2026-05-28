use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    IconName, Selectable,
};

use super::super::state::LevelEditorState;

/// Playback controls - Play, Pause, Stop buttons for simulation
pub struct PlaybackControls;

impl PlaybackControls {
    pub fn render<V>(
        state: &LevelEditorState,
        state_arc: crate::level_editor::StateEntity,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        ui::h_flex()
            .gap_1p5()
            .items_center()
            .child({
                let state_clone = state_arc.clone();
                if state.is_edit_mode() {
                    Button::new("play")
                        .icon(IconName::Play)
                        .tooltip(t!("LevelEditor.Toolbar.StartSimulation"))
                        .on_click(move |_, _, cx| {
                            state_clone.update(cx, |s, cx| { s.enter_play_mode(); cx.notify(); });
                        })
                        .into_any_element()
                } else {
                    Button::new("play_active")
                        .icon(IconName::Play)
                        .tooltip(t!("LevelEditor.Toolbar.SimulationRunning"))
                        .selected(true)
                        .into_any_element()
                }
            })
            .child({
                let disabled = state.is_edit_mode();
                let btn = Button::new("pause")
                    .icon(IconName::Pause)
                    .tooltip(t!("LevelEditor.Toolbar.PauseSimulation"))
                    .ghost()
                    .on_click(move |_, _, cx| {
                        // TODO: Implement pause
                    });
                if disabled {
                    btn.opacity(0.5).into_any_element()
                } else {
                    btn.into_any_element()
                }
            })
            .child({
                let state_clone = state_arc.clone();
                let disabled = state.is_edit_mode();
                let btn = Button::new("stop")
                    .icon(IconName::Square)
                    .tooltip(t!("LevelEditor.Toolbar.StopSimulation"))
                    .on_click(move |_, _, cx| {
                        state_clone.update(cx, |s, cx| { s.exit_play_mode(); cx.notify(); });
                    });
                if disabled {
                    btn.opacity(0.5).into_any_element()
                } else {
                    btn.into_any_element()
                }
            })
    }
}
