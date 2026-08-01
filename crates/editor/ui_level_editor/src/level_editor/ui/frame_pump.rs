//! A per-frame heartbeat that does not force a redraw.
//!
//! The level editor keeps a lot of its state in
//! `Arc<parking_lot::RwLock<LevelEditorState>>` rather than in GPUI entities.
//! GPUI cannot see writes to it, so `AnyView::cached` has no way to know a
//! cached panel went stale — something has to poll the state's revision
//! counters and call `cx.notify()` on the affected views.
//!
//! Historically that polling rode along on the fact that the whole panel tree
//! re-rendered every frame anyway. That is exactly the cost we are trying to
//! remove, so the poll needs its own drive.
//!
//! [`Window::on_next_frame`] callbacks are drained unconditionally at the top of
//! the platform's `on_request_frame`, *before* the `invalidator.is_dirty()`
//! check that decides whether to draw at all. A callback that re-registers
//! itself therefore runs once per platform frame whether or not anything is
//! dirty, and — unlike [`Window::request_animation_frame`] — marks nothing
//! dirty by merely running. Only the `cx.notify()` calls the poll chooses to
//! make cost anything.

use gpui::{App, Context, Entity, WeakEntity, Window};

/// Run `poll` once per platform frame for as long as `entity` is alive.
///
/// The pump stops on its own when the entity is dropped, so callers only need
/// to guard against starting it twice (see the `pump_started` flags at the call
/// sites).
///
/// `poll` should be cheap and should call `cx.notify()` only when it has
/// actually observed a change — an unconditional `notify` here reproduces the
/// full-tree invalidation this module exists to avoid, because
/// `Window::mark_view_dirty` also marks every ancestor view dirty.
pub fn spawn_frame_pump<V, F>(entity: &Entity<V>, window: &mut Window, poll: F)
where
    V: 'static,
    F: FnMut(&mut V, &mut Window, &mut Context<V>) + 'static,
{
    schedule(entity.downgrade(), poll, window);
}

fn schedule<V, F>(weak: WeakEntity<V>, mut poll: F, window: &mut Window)
where
    V: 'static,
    F: FnMut(&mut V, &mut Window, &mut Context<V>) + 'static,
{
    window.on_next_frame(move |window: &mut Window, cx: &mut App| {
        let Some(entity) = weak.upgrade() else {
            // View is gone; stop pumping.
            return;
        };
        entity.update(cx, |view, cx| poll(view, window, cx));
        schedule(weak, poll, window);
    });
}
