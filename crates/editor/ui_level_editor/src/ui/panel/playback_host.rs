//! The level editor as an engine playback host.
//!
//! The global toolbar (`engine_state::playback`) only sends commands and shows
//! state. While a level editor exists it is registered as a host: each frame
//! it services queued commands and mirrors its Play-In-Editor status back into
//! the global store.

use engine_state::playback::{PlayPhase, PlaybackCommand, PlaybackHost, playback};

use super::*;

/// Host registration plus the once-only pump flag.
pub(super) struct HostState {
    _registration: PlaybackHost,
    pump_started: bool,
}

impl HostState {
    pub(super) fn new() -> Self {
        Self {
            _registration: playback().register_host(),
            pump_started: false,
        }
    }
}

impl LevelEditorPanel {
    pub(super) fn start_playback_host(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.host.pump_started {
            return;
        }
        self.host.pump_started = true;
        crate::ui::frame_pump::spawn_frame_pump(&cx.entity(), window, |this, window, cx| {
            this.mirror_playback_state();
            for command in playback().drain_commands() {
                this.run_playback_command(command, window, cx);
            }
        });
    }

    /// Publish the editor's play status; writes only on change.
    fn mirror_playback_state(&self) {
        let (phase, paused, supports_control) = {
            let st = self.shared_state.read();
            let pie = &st.play.pie;
            let phase = if pie.active {
                PlayPhase::Playing
            } else if pie.building || pie.pending_start.is_some() || !st.scene.is_edit_mode() {
                PlayPhase::Building
            } else {
                PlayPhase::Stopped
            };
            (phase, pie.paused, pie.supports_control)
        };
        let pb = playback();
        let now = pb.state();
        if now.phase != phase || now.paused != paused || now.supports_control != supports_control {
            pb.update(|s| {
                s.phase = phase;
                s.paused = paused;
                s.supports_control = supports_control;
            });
        }
    }

    fn run_playback_command(
        &mut self,
        command: PlaybackCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match command {
            PlaybackCommand::Play => self.on_play_scene(&PlayScene, window, cx),
            PlaybackCommand::Stop => self.on_stop_scene(&StopScene, window, cx),
            PlaybackCommand::TogglePause => {
                let mut st = self.shared_state.write();
                if st.play.pie.active && st.play.pie.supports_control {
                    let paused = st.play.pie.paused;
                    st.play.pie.pause_request = Some(!paused);
                    st.play.pie.paused = !paused;
                }
            }
            PlaybackCommand::Step => {
                let mut st = self.shared_state.write();
                if st.play.pie.active && st.play.pie.paused {
                    st.play.pie.step_request = st.play.pie.step_request.saturating_add(1);
                }
            }
        }
    }
}
