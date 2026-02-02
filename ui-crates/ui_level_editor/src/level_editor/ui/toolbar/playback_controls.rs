use gpui::*;
use ui::{button::{Button, ButtonVariants as _}, ActiveTheme, IconName, Selectable, Sizable, Disableable};
use std::sync::Arc;
use rust_i18n::t;

use super::super::state::LevelEditorState;

/// Playback controls - Play, Pause, Stop buttons for simulation
pub struct PlaybackControls;

impl PlaybackControls {
    pub fn render<V: 'static>(
        state: &LevelEditorState,
        state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
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
                        .on_click(move |_, _, _| {
                            state_clone.write().enter_play_mode();
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
                    .on_click(move |_, _, _| {
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
                    .on_click(move |_, _, _| {
                        state_clone.write().exit_play_mode();
                    });
                if disabled {
                    btn.opacity(0.5).into_any_element()
                } else {
                    btn.into_any_element()
                }
            })
    }
}
