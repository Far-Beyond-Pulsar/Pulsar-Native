//! Playback commands on the host bus.
//!
//! Play / Stop / Pause / Step are requests, not state: the global toolbar
//! publishes a [`PlaybackCommand`] and whichever editor hosts play sessions
//! subscribes with [`subscribe_playback_commands`] and acts on it. The toolbar
//! never learns which editors exist, and an editor never learns who asked.
//! What is *currently* happening (phase, paused, speed) is not carried here;
//! that is the `PlaybackState` resource in `engine_state`.
//!
//! Delivery is synchronous on the publishing thread, like every host-bus
//! event, so a subscriber that has thread-affine work (all GPUI editors) queues
//! the command and handles it on its own thread. With no subscriber a command
//! is simply dropped.
//!
//! On the bus a command is the dynamic event `PlaybackCommand` with one string
//! field, `command`.

use gamma::{Channel, DynEvent, DynValue, EventDescriptor, FieldType, SubscribeOptions};

use crate::host::{HostBus, HostSubscription, host_bus};

/// A request to the playback host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackCommand {
    /// Start playing, or hot-reload if already playing.
    Play,
    Stop,
    /// Pause if running, resume if paused.
    TogglePause,
    /// Advance one frame while paused.
    Step,
}

impl PlaybackCommand {
    fn as_str(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Stop => "stop",
            Self::TogglePause => "toggle_pause",
            Self::Step => "step",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "play" => Self::Play,
            "stop" => Self::Stop,
            "toggle_pause" => Self::TogglePause,
            "step" => Self::Step,
            _ => return None,
        })
    }
}

/// The `PlaybackCommand` descriptor on the host bus.
pub fn descriptor() -> EventDescriptor {
    EventDescriptor::dynamic("PlaybackCommand", [("command", FieldType::Str)])
}

fn registered(bus: &HostBus) -> Option<u64> {
    let descriptor = descriptor();
    let id = descriptor.id;
    match bus.register_descriptor(&descriptor) {
        Ok(()) => Some(id),
        Err(error) => {
            tracing::error!("playback bus: cannot register PlaybackCommand: {error}");
            None
        }
    }
}

/// Send `command` to every playback host.
pub fn publish_playback_command(command: PlaybackCommand) {
    publish_on(host_bus(), command);
}

fn publish_on(bus: &HostBus, command: PlaybackCommand) {
    let Some(id) = registered(bus) else { return };
    let event = DynEvent::new(id, vec![DynValue::Str(command.as_str().to_owned())]);
    if let Err(error) = bus.publish_dyn(Channel::Global, &event) {
        tracing::error!("playback bus: publish failed: {error}");
    }
}

/// Call `callback` for every published command until the returned
/// subscription is dropped. The callback runs on the publisher's thread.
pub fn subscribe_playback_commands(
    callback: impl Fn(PlaybackCommand) + Send + Sync + 'static,
) -> PlaybackSubscription {
    subscribe_on(host_bus(), callback)
}

fn subscribe_on(
    bus: &HostBus,
    callback: impl Fn(PlaybackCommand) + Send + Sync + 'static,
) -> PlaybackSubscription {
    let id = registered(bus).unwrap_or_else(|| descriptor().id);
    let inner = bus.subscribe_dyn(id, SubscribeOptions::default(), move |event| {
        let [DynValue::Str(command)] = event.fields.as_slice() else {
            return;
        };
        match PlaybackCommand::parse(command) {
            Some(command) => callback(command),
            None => tracing::warn!("playback bus: unknown command {command:?} ignored"),
        }
    });
    PlaybackSubscription { _inner: inner }
}

/// A live subscription; unsubscribes on drop.
#[must_use = "dropping the subscription unsubscribes immediately"]
#[derive(Debug)]
pub struct PlaybackSubscription {
    _inner: HostSubscription,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn subscribers_receive_commands_until_dropped() {
        let bus = HostBus::local();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = Arc::clone(&seen);
        let sub = subscribe_on(&bus, move |c| s.lock().unwrap().push(c));

        publish_on(&bus, PlaybackCommand::Play);
        publish_on(&bus, PlaybackCommand::Step);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![PlaybackCommand::Play, PlaybackCommand::Step]
        );

        drop(sub);
        publish_on(&bus, PlaybackCommand::Stop);
        assert_eq!(seen.lock().unwrap().len(), 2, "unsubscribed");
    }

    #[test]
    fn every_command_round_trips() {
        for c in [
            PlaybackCommand::Play,
            PlaybackCommand::Stop,
            PlaybackCommand::TogglePause,
            PlaybackCommand::Step,
        ] {
            assert_eq!(PlaybackCommand::parse(c.as_str()), Some(c));
        }
    }
}
