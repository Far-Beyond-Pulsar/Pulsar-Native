use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    IconName, Selectable,
};

use crate::level_editor::state::LevelEditorState;

/// Playback controls - Play, Pause, Stop buttons for simulation
pub struct PlaybackControls;

impl PlaybackControls {
    pub fn render<V>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
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
                if state.scene.is_edit_mode() {
                    Button::new("play")
                        .icon(IconName::Play)
                        .tooltip(t!("LevelEditor.Toolbar.StartSimulation"))
                        .on_click(move |_, window, cx| {
                            // Play In Editor: enter play mode AND build+embed the
                            // game (issue #243). Shared with the `PlayScene` action.
                            crate::level_editor::ui::panel::begin_pie(
                                state_clone.clone(),
                                window,
                                cx,
                            );
                        })
                        .into_any_element()
                } else {
                    // Native hot reload (#653): while a game runs, Play
                    // rebuilds it and swaps the dylib WITHOUT stopping the
                    // world — entities/components survive, actor logic
                    // updates (the same contract `reload_blueprint` gives VM
                    // classes). Stop still ends the session.
                    Button::new("play_active")
                        .icon(IconName::Play)
                        .tooltip(t!("LevelEditor.Toolbar.ReloadSimulation"))
                        .selected(true)
                        .on_click(move |_, window, cx| {
                            crate::level_editor::ui::panel::begin_pie(
                                state_clone.clone(),
                                window,
                                cx,
                            );
                        })
                        .into_any_element()
                }
            })
            .child({
                // Pause / resume the running game's simulation (#925). The
                // viewport applies it to the game's TickLoop; rendering and
                // editing go on while paused.
                let state_clone = state_arc.clone();
                let pie = &state.play.pie;
                let enabled = pie.active && pie.supports_control;
                let paused = pie.paused;
                let btn = Button::new("pause")
                    .icon(if paused { IconName::Play } else { IconName::Pause })
                    .tooltip(if paused {
                        t!("LevelEditor.Toolbar.ResumeSimulation")
                    } else {
                        t!("LevelEditor.Toolbar.PauseSimulation")
                    })
                    .ghost()
                    .selected(paused)
                    .on_click(move |_, _, _| {
                        let mut st = state_clone.write();
                        if st.play.pie.active {
                            let paused = st.play.pie.paused;
                            st.play.pie.pause_request = Some(!paused);
                            st.play.pie.paused = !paused;
                        }
                    });
                if enabled {
                    btn.into_any_element()
                } else {
                    btn.opacity(0.5).into_any_element()
                }
            })
            .child({
                // Step one frame while paused.
                let state_clone = state_arc.clone();
                let pie = &state.play.pie;
                let enabled = pie.active && pie.supports_control && pie.paused;
                let btn = Button::new("step")
                    .icon(IconName::SkipNext)
                    .tooltip(t!("LevelEditor.Toolbar.StepSimulation"))
                    .ghost()
                    .on_click(move |_, _, _| {
                        let mut st = state_clone.write();
                        if st.play.pie.active && st.play.pie.paused {
                            st.play.pie.step_request = st.play.pie.step_request.saturating_add(1);
                        }
                    });
                if enabled {
                    btn.into_any_element()
                } else {
                    btn.opacity(0.5).into_any_element()
                }
            })
            .child({
                let state_clone = state_arc.clone();
                let disabled = state.scene.is_edit_mode();
                let btn = Button::new("stop")
                    .icon(IconName::Square)
                    .tooltip(t!("LevelEditor.Toolbar.StopSimulation"))
                    .on_click(move |_, _, _| {
                        crate::level_editor::ui::panel::end_pie(state_clone.clone());
                    });
                if disabled {
                    btn.opacity(0.5).into_any_element()
                } else {
                    btn.into_any_element()
                }
            })
    }
}
