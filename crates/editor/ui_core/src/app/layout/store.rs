//! The per-project owner of `layout.json`: window slots.
//!
//! Every editor window of a project owns one *slot* in the file (see
//! [`super::format`]). The store hands out slots, serialises writes to the
//! file and decides what happens to a slot when its window closes.
//!
//! **Allocation.** A window claims the lowest ordinal not held by a live window
//! of the same project root, so the first window is slot [`PRIMARY`] (0), the
//! next 1, and an ordinal is reused once its window is gone. A window opened to
//! restore a recorded slot (startup reopen) *reserves* that slot first; the
//! reservation is consumed by the next claim and cleared by the opener whether
//! or not the window came up.
//!
//! **Writes.** `save_slot` / `remove_slot` are read-modify-write on the whole
//! file under one in-process lock, finished with the atomic rename of
//! [`format::write_slot`], so windows of one process cannot clobber each
//! other's slots and every other slot is carried over verbatim.
//!
//! **Across processes** there is no file lock: two engine processes on one
//! project are last-writer-wins *per slot* (each write re-reads the file, so
//! the other process's slots survive unless both write in the same instant).
//! That is the accepted trade-off; slots in two processes can also collide on
//! the same ordinal, in which case the later writer's layout wins.
//!
//! **Closing.** [`LayoutStore::release_slot`] runs when a window is gone:
//! - the app is quitting, or it was the last live window of the project, or it
//!   was the primary window: every slot is kept (that is "app quit"; the next
//!   start restores them all);
//! - otherwise it was a secondary window closed on its own while others
//!   remain: its slot is removed so it is not reopened next time.
//!
//! Quitting is learnt from `App::on_app_quit` ([`LayoutStore::mark_quitting`]),
//! which runs before windows are torn down, so a quit that closes secondaries
//! before the primary cannot drop their slots.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::format::{self, layout_path, SavedWindowLayout};

/// Ordinal of the primary window's slot.
pub(crate) const PRIMARY: u32 = 0;
/// Most slots reopened at startup, a bound against a runaway file.
pub(crate) const MAX_REOPENED: usize = 8;

/// What became of a window's slot when it was released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotFate {
    /// The slot stays in the file for the next start.
    Kept,
    /// The window was closed on its own; its slot was removed from the file.
    Dropped,
}

#[derive(Default)]
struct Inner {
    /// Slots held by live windows, per project root.
    live: HashMap<PathBuf, BTreeSet<u32>>,
    /// Slots a window is about to be opened for, per project root, in order.
    reserved: HashMap<PathBuf, VecDeque<u32>>,
    /// Roots whose recorded secondary windows were already reopened.
    reopened: HashSet<PathBuf>,
}

impl Inner {
    /// The slot the next claim for `root` would take.
    fn next_slot(&self, root: &Path) -> u32 {
        let live = self.live.get(root);
        let is_live = |slot: u32| live.is_some_and(|set| set.contains(&slot));
        if let Some(slot) = self
            .reserved
            .get(root)
            .and_then(|queue| queue.iter().copied().find(|slot| !is_live(*slot)))
        {
            return slot;
        }
        (0..).find(|slot| !is_live(*slot)).unwrap_or(u32::MAX)
    }
}

pub(crate) struct LayoutStore {
    inner: Mutex<Inner>,
    quitting: AtomicBool,
}

impl LayoutStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            quitting: AtomicBool::new(false),
        }
    }

    /// The process-wide store.
    pub fn global() -> &'static LayoutStore {
        static STORE: OnceLock<LayoutStore> = OnceLock::new();
        STORE.get_or_init(LayoutStore::new)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock cannot leave `Inner` inconsistent.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    // ── Slot allocation ──────────────────────────────────────────────────────

    /// Take a slot for a new window of `root`: a reserved one, else the lowest
    /// free ordinal.
    pub fn claim_slot(&self, root: &Path) -> u32 {
        let mut inner = self.lock();
        let slot = inner.next_slot(root);
        if let Some(queue) = inner.reserved.get_mut(root) {
            queue.retain(|reserved| *reserved != slot);
        }
        inner
            .live
            .entry(root.to_path_buf())
            .or_default()
            .insert(slot);
        slot
    }

    /// The slot the next window of `root` will get, without taking it. Used to
    /// choose the window's geometry before it exists.
    pub fn peek_slot(&self, root: &Path) -> u32 {
        self.lock().next_slot(root)
    }

    /// Say that the next window opened for `root` restores `slot`.
    pub fn reserve_slot(&self, root: &Path, slot: u32) {
        let mut inner = self.lock();
        let queue = inner.reserved.entry(root.to_path_buf()).or_default();
        if !queue.contains(&slot) {
            queue.push_back(slot);
        }
    }

    /// Forget unused reservations for `root` (the window did not come up).
    pub fn clear_reservations(&self, root: &Path) {
        self.lock().reserved.remove(root);
    }

    pub fn is_live(&self, root: &Path, slot: u32) -> bool {
        self.lock()
            .live
            .get(root)
            .is_some_and(|set| set.contains(&slot))
    }

    /// First call per project root returns true: the primary window reopens
    /// the other recorded slots exactly once per process.
    pub fn begin_reopen(&self, root: &Path) -> bool {
        self.lock().reopened.insert(root.to_path_buf())
    }

    // ── Lifecycle ────────────────────────────────────────────────────────────

    /// The application is quitting: closing windows from now on keep their slots.
    pub fn mark_quitting(&self) {
        self.quitting.store(true, Ordering::SeqCst);
    }

    /// A window holding `slot` of `root` is gone. Decides, and applies, the
    /// fate of its slot (see the module docs).
    pub fn release_slot(&self, root: &Path, slot: u32) -> SlotFate {
        let mut inner = self.lock();
        let remaining = match inner.live.get_mut(root) {
            Some(set) => {
                set.remove(&slot);
                set.len()
            }
            None => 0,
        };
        if remaining == 0 {
            inner.live.remove(root);
        }
        if self.quitting.load(Ordering::SeqCst) || remaining == 0 || slot == PRIMARY {
            return SlotFate::Kept;
        }
        // Still under the lock: no save can interleave with the removal.
        if let Err(error) = format::remove_slot(&layout_path(root), &slot_key(slot)) {
            tracing::warn!("could not drop layout slot {slot}: {error}");
        }
        SlotFate::Dropped
    }

    // ── File access ──────────────────────────────────────────────────────────

    /// Write window `slot`, keeping every other slot as it is.
    pub fn save_slot(
        &self,
        root: &Path,
        slot: u32,
        layout: &SavedWindowLayout,
    ) -> std::io::Result<()> {
        let _guard = self.lock();
        format::write_slot(&layout_path(root), &slot_key(slot), layout)
    }

    /// The recorded layout of `slot`, if readable.
    pub fn read_slot(&self, root: &Path, slot: u32) -> Option<SavedWindowLayout> {
        let _guard = self.lock();
        format::read_slot(&layout_path(root), &slot_key(slot))
    }

    /// Remove `slot` from the file.
    #[allow(dead_code)]
    pub fn remove_slot(&self, root: &Path, slot: u32) -> std::io::Result<()> {
        let _guard = self.lock();
        format::remove_slot(&layout_path(root), &slot_key(slot))
    }

    /// Ordinals of the slots recorded for `root`, ascending. Keys that are not
    /// ordinals (written by something else) are ignored here and preserved in
    /// the file.
    pub fn slots(&self, root: &Path) -> Vec<u32> {
        let _guard = self.lock();
        format::slot_keys(&layout_path(root))
            .iter()
            .filter_map(|key| key.parse().ok())
            .collect()
    }
}

fn slot_key(slot: u32) -> String {
    slot.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui::dock::PanelState;

    fn layout_with(file: &str) -> SavedWindowLayout {
        let mut layout = SavedWindowLayout::default();
        layout.dock.center = PanelState::default().with_file(file);
        layout
    }

    fn center_file(layout: &SavedWindowLayout) -> Option<String> {
        layout.dock.center.file().map(|f| f.to_string())
    }

    #[test]
    fn slots_are_the_lowest_free_ordinal_per_project() {
        let store = LayoutStore::new();
        let a = Path::new("/a");
        let b = Path::new("/b");
        assert_eq!(store.claim_slot(a), 0);
        assert_eq!(store.claim_slot(a), 1);
        assert_eq!(store.claim_slot(a), 2);
        // Another project has its own ordinals.
        assert_eq!(store.claim_slot(b), 0);
        // Ordinals are reused once free.
        store.release_slot(a, 1);
        assert_eq!(store.claim_slot(a), 1);
        assert_eq!(store.peek_slot(a), 3);
        assert_eq!(store.peek_slot(a), 3, "peeking takes nothing");
    }

    #[test]
    fn a_reserved_slot_is_claimed_first_and_only_once() {
        let store = LayoutStore::new();
        let root = Path::new("/p");
        assert_eq!(store.claim_slot(root), 0);
        store.reserve_slot(root, 3);
        assert_eq!(store.peek_slot(root), 3);
        assert_eq!(store.claim_slot(root), 3);
        assert_eq!(store.claim_slot(root), 1, "the reservation was consumed");
        // A reservation for a live slot is skipped, not handed out twice.
        store.reserve_slot(root, 0);
        assert_eq!(store.claim_slot(root), 2);
        store.clear_reservations(root);
        assert_eq!(store.peek_slot(root), 4);
    }

    #[test]
    fn saving_a_slot_keeps_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = LayoutStore::new();
        store.save_slot(root, 0, &layout_with("main.rs")).unwrap();
        store.save_slot(root, 2, &layout_with("two.rs")).unwrap();
        store.save_slot(root, 0, &layout_with("main2.rs")).unwrap();

        assert_eq!(store.slots(root), vec![0, 2]);
        assert_eq!(
            center_file(&store.read_slot(root, 0).unwrap()).as_deref(),
            Some("main2.rs")
        );
        assert_eq!(
            center_file(&store.read_slot(root, 2).unwrap()).as_deref(),
            Some("two.rs")
        );
        assert!(store.read_slot(root, 1).is_none());
    }

    #[test]
    fn unknown_slots_survive_a_save_verbatim() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = layout_path(root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version":2,"windows":{"7":{"future":{"x":1},"dock":null},"extra":{"a":[1,2]}}}"#,
        )
        .unwrap();

        let store = LayoutStore::new();
        store.save_slot(root, 0, &layout_with("a.rs")).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["windows"]["7"]["future"]["x"], 1);
        assert_eq!(value["windows"]["extra"]["a"][1], 2);
        // Only ordinal keys are listed as slots.
        assert_eq!(store.slots(root), vec![0, 7]);
    }

    #[test]
    fn removing_a_slot_keeps_the_others_and_tolerates_absence() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = LayoutStore::new();
        // No file at all.
        store.remove_slot(root, 1).unwrap();
        store.save_slot(root, 0, &layout_with("a.rs")).unwrap();
        store.save_slot(root, 1, &layout_with("b.rs")).unwrap();
        store.remove_slot(root, 1).unwrap();
        store.remove_slot(root, 5).unwrap();
        assert_eq!(store.slots(root), vec![0]);
        assert!(store.read_slot(root, 0).is_some());
    }

    #[test]
    fn a_secondary_closed_on_its_own_drops_its_slot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = LayoutStore::new();
        let primary = store.claim_slot(root);
        let secondary = store.claim_slot(root);
        store
            .save_slot(root, primary, &layout_with("a.rs"))
            .unwrap();
        store
            .save_slot(root, secondary, &layout_with("b.rs"))
            .unwrap();

        assert_eq!(store.release_slot(root, secondary), SlotFate::Dropped);
        assert_eq!(store.slots(root), vec![0]);
        assert!(!store.is_live(root, secondary));
    }

    #[test]
    fn closing_the_primary_or_the_last_window_keeps_every_slot() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = LayoutStore::new();
        let primary = store.claim_slot(root);
        let secondary = store.claim_slot(root);
        store
            .save_slot(root, primary, &layout_with("a.rs"))
            .unwrap();
        store
            .save_slot(root, secondary, &layout_with("b.rs"))
            .unwrap();

        // The primary goes first: that is app quit, nothing is dropped, and
        // neither is the secondary that closes after it (it is the last).
        assert_eq!(store.release_slot(root, primary), SlotFate::Kept);
        assert_eq!(store.release_slot(root, secondary), SlotFate::Kept);
        assert_eq!(store.slots(root), vec![0, 1]);
    }

    #[test]
    fn quitting_keeps_secondaries_that_close_before_the_primary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store = LayoutStore::new();
        let primary = store.claim_slot(root);
        let secondary = store.claim_slot(root);
        store
            .save_slot(root, primary, &layout_with("a.rs"))
            .unwrap();
        store
            .save_slot(root, secondary, &layout_with("b.rs"))
            .unwrap();

        store.mark_quitting();
        assert_eq!(store.release_slot(root, secondary), SlotFate::Kept);
        assert_eq!(store.release_slot(root, primary), SlotFate::Kept);
        assert_eq!(store.slots(root), vec![0, 1]);
    }

    #[test]
    fn the_startup_reopen_happens_once_per_project() {
        let store = LayoutStore::new();
        assert!(store.begin_reopen(Path::new("/p")));
        assert!(!store.begin_reopen(Path::new("/p")));
        assert!(store.begin_reopen(Path::new("/q")));
    }
}
