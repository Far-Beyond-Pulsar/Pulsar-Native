//! The editor's `construct` pump: a [`SessionKind::Editor`] session over the
//! editor's own scene.
//!
//! # Where it runs, and why there
//!
//! The pump runs once per viewport frame in `SceneRenderer::upload`, right
//! before [`super::end_change_window`] closes the frame's change window
//! (`.agents/SCENEDB_MIGRATION.md`, "Observers"). That is the one place every
//! frame's writes (a gizmo drag, an inspector slider, undo) have landed and
//! that runs once per frame whatever the number of writes. The pump reads its
//! own change-journal cursors, which the window close does not touch, so its
//! position relative to the close only decides *when*, not *whether*, a write
//! is seen.
//!
//! # World ownership (doc 2.4)
//!
//! A Play-in-Editor game runs on this same world, with a session of its own
//! that delivers `construct` itself. While it does, this pump is
//! **suspended** ([`EditorConstructPump::suspend_for_play`]); on Stop the
//! editor restores its snapshot and calls
//! [`EditorConstructPump::resume_after_play`], which resumes with one
//! rescan: every live instance constructs once, however many components the
//! restore touched.

use super::SharedScene;
use parking_lot::Mutex;
use pulsar_events::EventHub;
use pulsar_scenedb::Entity;
use pulsar_world_registry::{ConstructStats, Session, SessionKind};
use std::sync::Arc;

struct Inner {
    session: Session,
    events: EventHub,
    /// The world revision after the last pump, so an idle frame (nothing
    /// written, including by this pump's own `construct`s) costs a read
    /// lock and no more.
    last_revision: Option<u64>,
}

/// A handle to the editor's construct pump. Clones share one session.
#[derive(Clone)]
pub struct EditorConstructPump(Arc<Mutex<Inner>>);

impl Default for EditorConstructPump {
    fn default() -> Self {
        Self::new()
    }
}

impl EditorConstructPump {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Inner {
            session: Session::new(SessionKind::Editor),
            events: EventHub::new(),
            last_revision: None,
        })))
    }

    /// Deliver the `construct`s of what was written to `scene` since the last
    /// pump. Returns how many ran. Cheap when nothing changed.
    pub fn pump(&self, scene: &SharedScene) -> usize {
        let mut inner = self.0.lock();
        if inner.session.construct_suspended() {
            return 0;
        }
        let revision = scene.read().world.revision();
        if inner.last_revision == Some(revision) && !inner.session.rescan_pending() {
            return 0;
        }
        let Inner {
            session,
            events,
            last_revision,
        } = &mut *inner;
        let mut store = scene.write();
        let delivered = pulsar_world_registry::pump_construct(&mut store.world, events, session);
        // After the pump: its own constructs' writes are not new work.
        *last_revision = Some(store.world.revision());
        delivered
    }

    /// A play session now owns the world: deliver nothing until
    /// [`Self::resume_after_play`].
    pub fn suspend_for_play(&self) {
        self.0.lock().session.set_construct_suspended(true);
    }

    /// The play session ended and the editor restored its world: resume,
    /// with one rescan.
    pub fn resume_after_play(&self) {
        self.0.lock().session.set_construct_suspended(false);
    }

    /// The scene's content was replaced wholesale (a snapshot restore, a
    /// level load): scan once instead of reading each change.
    pub fn request_rescan(&self) {
        self.0.lock().session.request_rescan();
    }

    /// An editor write that knows which property it changed says so; the
    /// instance's `construct` carries the name.
    pub fn note_changed(&self, instance: Entity, property: impl Into<String>) {
        self.0.lock().session.note_changed(instance, property);
    }

    /// Whether the pump is suspended (a play session owns the world).
    pub fn is_suspended(&self) -> bool {
        self.0.lock().session.construct_suspended()
    }

    /// What the pump did so far.
    pub fn stats(&self) -> ConstructStats {
        self.0.lock().session.construct_stats()
    }

    /// The editor session's kind: always [`SessionKind::Editor`].
    pub fn kind(&self) -> SessionKind {
        self.0.lock().session.kind()
    }

    /// Run `f` on the editor session (resources a plugin keeps for the
    /// editor, tests).
    pub fn with_session<R>(&self, f: impl FnOnce(&mut Session) -> R) -> R {
        f(&mut self.0.lock().session)
    }
}
