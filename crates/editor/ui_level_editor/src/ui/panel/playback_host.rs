//! The level editor as a playback host.
//!
//! The global toolbar shows the `PlaybackState` resource and publishes
//! `PlaybackCommand`s on the host bus. This editor subscribes while it exists,
//! runs the commands on the UI thread, and reports its Play-In-Editor status
//! back into the resource. Bus delivery happens on the publisher's thread, so
//! the subscription only queues; an async task drains the queue in the window.

use engine_state::playback::{PlayPhase, update_playback_if_changed};
use pulsar_events::{PlaybackCommand, PlaybackSubscription, subscribe_playback_commands};

use super::*;

/// Keeps the editor subscribed to playback commands and, when it goes away,
/// leaves the shared state as "stopped" since nothing is hosting a session.
pub(super) struct PlaybackHostBinding {
    _subscription: PlaybackSubscription,
    _commands: Task<()>,
}

impl Drop for PlaybackHostBinding {
    fn drop(&mut self) {
        update_playback_if_changed(|s| {
            s.phase = PlayPhase::Stopped;
            s.paused = false;
            s.supports_control = false;
        });
    }
}

impl LevelEditorPanel {
    pub(super) fn bind_playback_host(
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PlaybackHostBinding {
        let (tx, rx) = smol::channel::unbounded();
        let subscription = subscribe_playback_commands(move |command| {
            let _ = tx.try_send(command);
        });
        let commands = cx.spawn_in(window, async move |this, cx| {
            while let Ok(command) = rx.recv().await {
                let handled = this.update_in(cx, |panel, window, cx| {
                    // Every open level editor hears the command; only the one
                    // in the window the user is working in acts on it.
                    if window.is_window_active() {
                        panel.run_playback_command(command, window, cx);
                    }
                });
                if handled.is_err() {
                    break;
                }
            }
        });
        PlaybackHostBinding {
            _subscription: subscription,
            _commands: commands,
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

/// Publish the editor's play status to the shared resource. Called from the
/// panel's existing poll loop; only a real change wakes watchers.
pub(super) fn publish_playback_status(state: &LevelEditorState) {
    let pie = &state.play.pie;
    let phase = if pie.active {
        PlayPhase::Playing
    } else if pie.building || pie.pending_start.is_some() || !state.scene.is_edit_mode() {
        PlayPhase::Building
    } else {
        PlayPhase::Stopped
    };
    update_playback_if_changed(|s| {
        s.phase = phase;
        s.paused = pie.paused;
        s.supports_control = pie.supports_control;
    });
}
