//! The `game::` natives every script has: loading a level, the input mode,
//! quitting and the game instance.
//!
//! The first three are requests to the host: like `world::spawn`, a native
//! only queues them on the running driver frame, and the tick loop applies
//! them after the script phase (a level load starts at the next tick). They
//! work the same in a packaged game, in Play-in-Editor and headless (where
//! the input mode changes no cursor).
//!
//! Two parameters carry a `pin_type.<parameter>` attribute: the name of a
//! reflected type whose property editor a node editor offers for the pin's
//! literal value (the level picker, the input mode choice). Scripts see a
//! plain string either way.

use pulsar_scenedb::Entity;
use pulsar_script_vm::{Host, NativeFn, NativeRegistration};

use super::commands::{queue, WorldCommand};
use super::driver::GameInstanceEntity;
use crate::game_control::{HostRequest, InputMode};

/// Called by [`new_runtime`](super::new_runtime) so the linker keeps this
/// module's registrations in every host that builds the engine's runtime.
pub(super) fn keep_linked() {}

/// Queue `request` on the running driver frame; warn outside one.
fn request(native: &str, request: HostRequest) {
    if !queue(WorldCommand::Host(request)) {
        tracing::warn!("{native} called outside a script driver frame; ignored");
    }
}

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder("game::load_level")
                .doc(
                    "Switch to a level (project-relative, `scene/menu.level`). It is read while \
                     the current level runs on; then the current level ends completely (its \
                     objects, roles, scripts and physics) and the new one loads a few \
                     milliseconds a frame. The game instance and its widgets stay. Loading the \
                     current level restarts it. A level that cannot be read leaves the current \
                     one as it is.",
                )
                .attr("category", "Game")
                .attr("display_name", "Load Level")
                .attr("pin_type.level", "LevelPath")
                .params(["level"])
                .build(|level: String| {
                    if level.trim().is_empty() {
                        tracing::warn!("game::load_level: no level was named; nothing loads");
                        return;
                    }
                    request("game::load_level", HostRequest::LoadLevel(level));
                })
        },
    }
}

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder("game::set_input_mode")
                .doc(
                    "`Game`: gameplay input; a click captures the cursor. `UI`: the cursor is \
                     shown and free, a click does not capture it, and gameplay input (look, \
                     move, actions, the free camera) is not read; widgets get the pointer, and \
                     scripts still hear raw key events. It stays across level loads.",
                )
                .attr("category", "Game")
                .attr("display_name", "Set Input Mode")
                .attr("pin_type.mode", "InputModeName")
                .params(["mode"])
                .build(|mode: String| match InputMode::parse(&mode) {
                    Some(mode) => request("game::set_input_mode", HostRequest::SetInputMode(mode)),
                    None => tracing::warn!(
                        mode = %mode,
                        "game::set_input_mode: the mode is `Game` or `UI`; unchanged"
                    ),
                })
        },
    }
}

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder("game::quit")
                .doc(
                    "End the game: the window closes, Play-in-Editor stops, a headless run \
                     ends. Every script gets its end_play; the game instance's on_shutdown \
                     runs last.",
                )
                .attr("category", "Game")
                .attr("display_name", "Quit Game")
                .build(|| request("game::quit", HostRequest::Quit))
        },
    }
}

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder("game::instance")
                .doc(
                    "The game instance's entity (send it events, call its trait methods); a \
                     null entity when the project has none.",
                )
                .side_effect_free()
                .attr("category", "Game")
                .attr("display_name", "Get Game Instance")
                .build(|host: &mut Host<'_>| {
                    host.world()
                        .query::<&GameInstanceEntity>()
                        .next()
                        .map_or(Entity::DANGLING, |(entity, _)| entity)
                })
        },
    }
}
