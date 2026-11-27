use engine_state::playback::{PlayPhase, PlaybackState};
use gpui::*;
use pulsar_events::{publish_playback_command, PlaybackCommand};
use rust_i18n::t;
use ui::{
    button::{Button, ButtonVariants as _},
    IconName, Selectable,
};

/// Play / Pause / Step / Stop.
///
/// A view of [`PlaybackState`]. `play` is the Play split button (play in the
/// editor, or run a build configuration); Pause, Step and Stop send
/// [`PlaybackCommand`]s that whichever editor is the playback host carries
/// out. Stop also ends a game a build launched.
pub struct PlaybackControls;

impl PlaybackControls {
    pub fn render(state: &PlaybackState, play: impl IntoElement) -> impl IntoElement {
        let stopped = state.phase == PlayPhase::Stopped;
        let game_running = state.game_running;
        let can_control = state.phase == PlayPhase::Playing && state.supports_control;
        let paused = state.paused;

        ui::h_flex()
            .gap_1p5()
            .items_center()
            .child(play)
            .child({
                let btn = Button::new("pause")
                    .icon(if paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .tooltip(if paused {
                        t!("LevelEditor.Toolbar.ResumeSimulation")
                    } else {
                        t!("LevelEditor.Toolbar.PauseSimulation")
                    })
                    .ghost()
                    .selected(paused)
                    .on_click(|_, _, _| send(PlaybackCommand::TogglePause));
                if can_control {
                    btn.into_any_element()
                } else {
                    btn.opacity(0.5).into_any_element()
                }
            })
            .child({
                let btn = Button::new("step")
                    .icon(IconName::SkipNext)
                    .tooltip(t!("LevelEditor.Toolbar.StepSimulation"))
                    .ghost()
                    .on_click(|_, _, _| send(PlaybackCommand::Step));
                if can_control && paused {
                    btn.into_any_element()
                } else {
                    btn.opacity(0.5).into_any_element()
                }
            })
            .child({
                let btn = Button::new("stop")
                    .icon(IconName::Square)
                    .tooltip(t!("LevelEditor.Toolbar.StopSimulation"))
                    .on_click(move |_, _, _| {
                        if game_running {
                            ui_build::stop_game();
                        } else {
                            send(PlaybackCommand::Stop);
                        }
                    });
                if stopped && !game_running {
                    btn.opacity(0.5).into_any_element()
                } else {
                    btn.into_any_element()
                }
            })
    }
}

fn send(command: PlaybackCommand) {
    publish_playback_command(command);
}
