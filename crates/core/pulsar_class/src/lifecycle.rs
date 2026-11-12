//! The `ClassInstance` lifecycle: Blueprint instances as a normal component.
//!
//! `ClassInstance` implements [`WorldActor`] natively and registers its table
//! with the engine ([`EngineActor`]; the engine has no plugin root). The
//! lifecycle events do not run scripts themselves (a callback holds the
//! world mid-frame, and the VM's `begin_play` is deliberately queued to the
//! script phase): they post a [`ClassEvent`] to the session's [`ClassInbox`],
//! a session resource the script driver registers when it attaches. The
//! driver drains it at the start of its phase, so a script starts, restarts
//! and ends exactly when the component lifecycle says so.
//!
//! No inbox in the session (the editor's, or a game without scripting) means
//! the events are dropped: nothing queues up unread.
//!
//! This is no Blueprint surface: the callbacks are table entries, not
//! reflected methods (`pulsar_component_lifecycle`).

use std::sync::{Arc, Mutex};

use pulsar_component_lifecycle::{ActorCx, ActorTable, EngineActor, WorldActor};
use pulsar_scenedb::{Entity, World};

use crate::component::ClassInstance;

/// What the component lifecycle told the script driver about one placed class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClassEvent {
    /// The instance is live in the session: start its script.
    Begin { object: Entity, instance: Entity },
    /// The instance was added or rewritten: a different class restarts the
    /// script, the same class binds its component slots again.
    Construct { object: Entity, instance: Entity },
    /// The instance stopped being live (disabled, removed, its object
    /// despawned): end its script. `instance` is the component entity, whose
    /// component may be gone.
    End {
        object: Entity,
        instance: Option<Entity>,
    },
}

/// The session resource the lifecycle posts [`ClassEvent`]s to. Clones share
/// one queue, so the driver keeps a handle across a session's end (which
/// drops the session's resources after the ends ran).
#[derive(Clone, Default)]
pub struct ClassInbox(Arc<Mutex<Vec<ClassEvent>>>);

impl ClassInbox {
    pub fn push(&self, event: ClassEvent) {
        self.lock().push(event);
    }

    /// Everything posted since the last call, in order.
    pub fn take(&self) -> Vec<ClassEvent> {
        std::mem::take(&mut *self.lock())
    }

    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ClassEvent>> {
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn post(cx: &ActorCx<'_>, event: ClassEvent) {
    if let Some(inbox) = cx.session.resource::<ClassInbox>() {
        inbox.push(event);
    }
}

impl WorldActor for ClassInstance {
    fn construct(_: &mut World, cx: &mut ActorCx<'_>, instance: Entity) {
        let object = cx.entity;
        post(cx, ClassEvent::Construct { object, instance });
    }

    fn begin_play(_: &mut World, cx: &mut ActorCx<'_>, instance: Entity) {
        let object = cx.entity;
        post(cx, ClassEvent::Begin { object, instance });
    }

    fn end_play(_: &mut World, cx: &mut ActorCx<'_>) {
        let event = ClassEvent::End {
            object: cx.entity,
            instance: cx.instance,
        };
        post(cx, event);
    }
}

fn table() -> ActorTable {
    ActorTable {
        // The script driver ticks the scripts, in the script phase.
        tick: None,
        ..ActorTable::of_world::<ClassInstance>()
    }
}

inventory::submit! {
    EngineActor {
        class_name: crate::CLASS_INSTANCE,
        table,
    }
}
