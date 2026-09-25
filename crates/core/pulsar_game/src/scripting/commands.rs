//! Deferred world changes requested by scripts: `world::spawn`,
//! `world::spawn_child` and `world::destroy` (#922).
//!
//! A native runs while the script phase holds the world, so these natives
//! never build or tear down objects themselves. They append a
//! [`WorldCommand`] to the queue of the [`ScriptDriver`](super::ScriptDriver)
//! frame that is running on this thread, and the driver applies the queue,
//! in order, at the end of its script phase:
//!
//! - **spawn** reserves the new object's entity at once (a bare entity with
//!   no components), so the script gets a usable `Entity` back. At the end
//!   of the phase the entity becomes a full class instance (scene object,
//!   `ClassInstance`, every prefab component); its script's `begin_play`
//!   runs at the next reconcile, in spawn order. Until then the entity has
//!   no components, so component access on it resolves to nothing.
//! - **destroy** runs the `end_play` of every script instance on the object
//!   and its descendants (while their components still exist), then
//!   despawns the object tree.
//!
//! # Naming a class
//!
//! `class` is a string: the class **GUID** (`class.json`'s `class_id`, what
//! levels store) or the class **name** (its directory under `src/classes`).
//! The GUID is tried first. A GUID survives renaming the class; a name is
//! what a person types. Unknown classes are logged and the reserved entity
//! is released.
//!
//! Outside a driver frame (a native called directly by a host that runs no
//! driver) there is no queue: spawn returns `entity::none()` and destroy
//! does nothing, both with a warning.

use std::cell::RefCell;

use pulsar_scenedb::Entity;
use pulsar_script_vm::{Host, NativeFn, NativeRegistration};

/// One deferred world change.
#[derive(Clone, Debug, PartialEq)]
pub enum WorldCommand {
    /// Make the reserved `entity` an instance of `class` (GUID or name).
    Spawn {
        entity: Entity,
        class: String,
        /// Parent object; `position` is then relative to it.
        parent: Option<Entity>,
        position: [f32; 3],
    },
    /// End the scripts on `entity` and its descendants, then despawn them.
    Destroy { entity: Entity },
}

thread_local! {
    /// The queue of the driver frame running on this thread.
    static QUEUE: RefCell<Option<Vec<WorldCommand>>> = const { RefCell::new(None) };
}

/// A driver frame's command queue, installed for this thread while the
/// scope lives. Scopes nest: the previous queue comes back on drop.
pub(crate) struct CommandScope {
    previous: Option<Vec<WorldCommand>>,
    finished: bool,
}

impl CommandScope {
    pub(crate) fn begin() -> Self {
        let previous = QUEUE.with(|q| q.borrow_mut().replace(Vec::new()));
        Self { previous, finished: false }
    }

    /// Everything queued so far; the scope keeps collecting afterwards.
    pub(crate) fn take(&mut self) -> Vec<WorldCommand> {
        QUEUE.with(|q| q.borrow_mut().as_mut().map(std::mem::take).unwrap_or_default())
    }

    /// End the scope, returning what is still queued.
    pub(crate) fn finish(mut self) -> Vec<WorldCommand> {
        let left = self.take();
        self.restore();
        left
    }

    fn restore(&mut self) {
        if !self.finished {
            self.finished = true;
            let previous = self.previous.take();
            QUEUE.with(|q| *q.borrow_mut() = previous);
        }
    }
}

impl Drop for CommandScope {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Queue `command` on this thread's driver frame. `false` when no driver
/// frame is running.
fn queue(command: WorldCommand) -> bool {
    QUEUE.with(|q| match q.borrow_mut().as_mut() {
        Some(queue) => {
            queue.push(command);
            true
        }
        None => false,
    })
}

fn has_queue() -> bool {
    QUEUE.with(|q| q.borrow().is_some())
}

fn spawn(host: &mut Host<'_>, parent: Option<Entity>, class: String, position: [f32; 3]) -> Entity {
    if !has_queue() {
        tracing::warn!(class = %class, "world::spawn called outside a script driver frame; nothing spawned");
        return Entity::DANGLING;
    }
    if let Some(parent) = parent {
        if parent == Entity::DANGLING || !host.world.is_alive(parent) {
            tracing::warn!(class = %class, "world::spawn_child: the parent is not live; nothing spawned");
            return Entity::DANGLING;
        }
    }
    let entity = host.world.spawn();
    queue(WorldCommand::Spawn { entity, class, parent, position });
    entity
}

fn position(x: f64, y: f64, z: f64) -> [f32; 3] {
    [x as f32, y as f32, z as f32]
}

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder("world::spawn")
                .doc(
                    "Spawn an instance of a class (its GUID or its name) at a world position. \
                     The entity is returned at once; the object and its components are built at \
                     the end of the script phase, and its script starts next frame.",
                )
                .attr("category", "World")
                .params(["class", "x", "y", "z"])
                .build(|host: &mut Host<'_>, class: String, x: f64, y: f64, z: f64| {
                    spawn(host, None, class, position(x, y, z))
                })
        },
    }
}

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder("world::spawn_child")
                .doc(
                    "Spawn an instance of a class (its GUID or its name) as a child of `parent`, \
                     at a position relative to it. Built at the end of the script phase, like \
                     world::spawn.",
                )
                .attr("category", "World")
                .params(["parent", "class", "x", "y", "z"])
                .build(|host: &mut Host<'_>, parent: Entity, class: String, x: f64, y: f64, z: f64| {
                    spawn(host, Some(parent), class, position(x, y, z))
                })
        },
    }
}

inventory::submit! {
    NativeRegistration {
        build: || {
            NativeFn::builder("world::destroy")
                .doc(
                    "Destroy an object and its children at the end of the script phase. Their \
                     scripts get end_play first.",
                )
                .attr("category", "World")
                .params(["entity"])
                .build(|entity: Entity| {
                    if entity == Entity::DANGLING {
                        return;
                    }
                    if !queue(WorldCommand::Destroy { entity }) {
                        tracing::warn!("world::destroy called outside a script driver frame; nothing destroyed");
                    }
                })
        },
    }
}
