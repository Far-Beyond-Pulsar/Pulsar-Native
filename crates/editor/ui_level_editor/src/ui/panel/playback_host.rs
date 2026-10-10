//! The level editor as a playback host.
//!
//! The global toolbar shows the `PlaybackState` resource and publishes
//! `PlaybackCommand`s on the host bus. This editor subscribes while it exists,
//! runs the commands on the UI thread, and reports its Play-In-Editor status
//! back into the resource. Bus delivery happens on the publisher's thread, so
//! the subscription only queues; an async task drains the queue in the window.
//!
//! With several level editors open, one is the playback host (#1009): the
//! editor the user last pressed Play in, or else the first one to report. Only
//! the host reports its status and handles Stop, Pause and Step; Play goes to
//! the editor in the active window, which becomes the host.

use engine_state::playback::{
    claim_playback_host, claim_playback_host_if_free, is_playback_host, playback,
    release_playback_host, update_playback_if_changed, PlayPhase,
};
use pulsar_events::{subscribe_playback_commands, PlaybackCommand, PlaybackSubscription};

use super::*;

/// Keeps the editor subscribed to playback commands and, when it goes away
/// as the playback host, leaves the shared state as "stopped" since it no
/// longer hosts a session.
pub(super) struct PlaybackHostBinding {
    host_id: u64,
    _subscription: PlaybackSubscription,
    _commands: Task<()>,
}

impl Drop for PlaybackHostBinding {
    fn drop(&mut self) {
        // Another editor's status is not this one's to reset; the next editor
        // to report becomes the host.
        if release_playback_host(self.host_id) {
            update_playback_if_changed(|s| {
                s.phase = PlayPhase::Stopped;
                s.paused = false;
                s.supports_control = false;
            });
        }
    }
}

impl LevelEditorPanel {
    /// `host_id` comes from `new_playback_host_id`, and is the one this
    /// editor's poll loop passes to [`publish_playback_status`].
    pub(super) fn bind_playback_host(
        host_id: u64,
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
                    // Every open level editor hears the command. Play goes to
                    // the one in the window the user is working in, unless
                    // another editor's session is still running; the rest go
                    // to the host.
                    let acts = match command {
                        PlaybackCommand::Play => {
                            window.is_window_active()
                                && (is_playback_host(host_id) || playback().get().is_stopped())
                        }
                        _ => is_playback_host(host_id),
                    };
                    if acts {
                        if command == PlaybackCommand::Play {
                            claim_playback_host(host_id);
                        }
                        panel.run_playback_command(command, window, cx);
                    }
                });
                if handled.is_err() {
                    break;
                }
            }
        });
        PlaybackHostBinding {
            host_id,
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

/// Publish the editor's play status to the shared resource, if it is the
/// playback host (or nobody is). Called from the panel's existing poll loop;
/// only a real change wakes watchers.
pub(super) fn publish_playback_status(host_id: u64, state: &LevelEditorState) {
    if !claim_playback_host_if_free(host_id) {
        return;
    }
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
