//! Wayland input for the viewport's mouse-look, bypassing the UI thread.
//!
//! Wayland gives clients no global pointer position, no warping, and no
//! global key state, so the X11 approach (query the pointer, warp it back,
//! `XQueryKeymap`) can't work. Instead this module talks Wayland itself, on
//! the *same* connection winit uses (no extra client), with its own event
//! queue dispatched on its own thread:
//!
//! - `zwp_relative_pointer_v1` delivers pointer deltas, which accumulate
//!   into [`take_delta`] -- the same shape as the macOS relative mode.
//! - `zwp_pointer_constraints_v1` locks the pointer in place while dragging.
//! - Its own `wl_keyboard` tracks held keys for [`held_keys`]; the compositor
//!   sends key events to every keyboard object of the focused client.
//!
//! The input thread keeps polling these at its own rate, as on every other
//! platform; nothing here waits on the UI thread. Hiding the cursor is left
//! to the viewport's `CursorStyle::None`, which winit applies and restores.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use device_query::Keycode;
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum, delegate_noop};
use wayland_protocols::wp::pointer_constraints::zv1::client::{
    zwp_locked_pointer_v1::ZwpLockedPointerV1,
    zwp_pointer_constraints_v1::{self, ZwpPointerConstraintsV1},
};
use wayland_protocols::wp::relative_pointer::zv1::client::{
    zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1,
    zwp_relative_pointer_v1::{self, ZwpRelativePointerV1},
};

/// State shared between the dispatch thread and the viewport's calls.
struct Shared {
    conn: Connection,
    qh: QueueHandle<State>,
    constraints: Option<ZwpPointerConstraintsV1>,
    pointer: Mutex<Option<wl_pointer::WlPointer>>,
    locked: Mutex<Option<ZwpLockedPointerV1>>,
    /// Pointer motion since the last [`take_delta`], in surface pixels.
    delta: Mutex<(f64, f64)>,
    /// evdev codes of the keys held while our surface has keyboard focus.
    keys: Mutex<HashSet<u32>>,
}

/// Dispatch-thread state: the objects only it creates.
struct State {
    shared: &'static Shared,
    seat: wl_seat::WlSeat,
    relative_manager: Option<ZwpRelativePointerManagerV1>,
    relative: Option<ZwpRelativePointerV1>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
}

/// Set up on the first [`lock`] with a Wayland window; `None` if the window
/// isn't Wayland or setup failed (then the X11 path is used).
static WAYLAND: OnceLock<Option<&'static Shared>> = OnceLock::new();

/// Whether the Wayland path is in use.
pub fn is_active() -> bool {
    matches!(WAYLAND.get(), Some(Some(_)))
}

fn shared() -> Option<&'static Shared> {
    WAYLAND.get().copied().flatten()
}

/// Whether `window` is a Wayland window.
pub fn is_wayland_window(window: &gpui::Window) -> bool {
    HasWindowHandle::window_handle(window)
        .map(|h| matches!(h.as_raw(), RawWindowHandle::Wayland(_)))
        .unwrap_or(false)
}

/// The window's `wl_surface` pointer.
fn surface_ptr(window: &gpui::Window) -> Option<*mut std::ffi::c_void> {
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Wayland(h) => Some(h.surface.as_ptr()),
        _ => None,
    }
}

fn init(window: &gpui::Window) -> Option<&'static Shared> {
    let display = match HasDisplayHandle::display_handle(window).ok()?.as_raw() {
        RawDisplayHandle::Wayland(h) => h.display.as_ptr(),
        _ => return None,
    };
    // SAFETY: winit owns this `wl_display` for the life of the process; we
    // only add our own event queue to it and never disconnect it.
    let backend = unsafe { Backend::from_foreign_display(display.cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, mut queue) = match registry_queue_init::<State>(&conn) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("[VIEWPORT] Wayland: registry init failed: {e}");
            return None;
        }
    };
    let qh = queue.handle();
    let seat: wl_seat::WlSeat = match globals.bind(&qh, 1..=5, ()) {
        Ok(seat) => seat,
        Err(e) => {
            tracing::warn!("[VIEWPORT] Wayland: no wl_seat: {e}");
            return None;
        }
    };
    let constraints: Option<ZwpPointerConstraintsV1> = globals.bind(&qh, 1..=1, ()).ok();
    let relative_manager: Option<ZwpRelativePointerManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    if constraints.is_none() {
        tracing::warn!("[VIEWPORT] Wayland: compositor lacks pointer-constraints; the pointer won't be locked while dragging");
    }
    if relative_manager.is_none() {
        tracing::warn!("[VIEWPORT] Wayland: compositor lacks relative-pointer; mouse-look is unavailable");
    }

    let shared: &'static Shared = Box::leak(Box::new(Shared {
        conn,
        qh,
        constraints,
        pointer: Mutex::new(None),
        locked: Mutex::new(None),
        delta: Mutex::new((0.0, 0.0)),
        keys: Mutex::new(HashSet::new()),
    }));
    let mut state = State {
        shared,
        seat,
        relative_manager,
        relative: None,
        keyboard: None,
    };
    // Receive the seat's capabilities and create our pointer and keyboard
    // before the first lock needs them.
    if let Err(e) = queue.roundtrip(&mut state) {
        tracing::warn!("[VIEWPORT] Wayland: roundtrip failed: {e}");
        return None;
    }

    let spawned = std::thread::Builder::new()
        .name("Wayland Input".into())
        .spawn(move || {
            loop {
                if let Err(e) = queue.blocking_dispatch(&mut state) {
                    tracing::warn!("[VIEWPORT] Wayland input dispatch ended: {e}");
                    return;
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("[VIEWPORT] Wayland: could not start input thread: {e}");
        return None;
    }
    tracing::info!("[VIEWPORT] Wayland input bypass active");
    Some(shared)
}

/// Lock the pointer to `window` until [`unlock`]. Sets up the Wayland path on
/// first use. Returns `false` when `window` isn't a Wayland window.
pub fn lock(window: &gpui::Window) -> bool {
    if !is_wayland_window(window) {
        return false;
    }
    let Some(shared) = *WAYLAND.get_or_init(|| init(window)) else {
        return false;
    };
    let (Some(constraints), Some(pointer), Some(raw_surface)) = (
        shared.constraints.as_ref(),
        shared.pointer.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        surface_ptr(window),
    ) else {
        return true;
    };
    // SAFETY: the surface belongs to a live window for the duration of this
    // call; the proxy only borrows it for the lock request.
    let surface_id = unsafe { ObjectId::from_ptr(wl_surface::WlSurface::interface(), raw_surface.cast()) };
    let Ok(surface) = surface_id.and_then(|id| wl_surface::WlSurface::from_id(&shared.conn, id)) else {
        tracing::warn!("[VIEWPORT] Wayland: could not wrap the window's wl_surface");
        return true;
    };
    let mut locked = shared.locked.lock().unwrap_or_else(|p| p.into_inner());
    if locked.is_none() {
        *locked = Some(constraints.lock_pointer(
            &surface,
            &pointer,
            None,
            zwp_pointer_constraints_v1::Lifetime::Persistent,
            &shared.qh,
            (),
        ));
        let _ = shared.conn.flush();
    }
    true
}

/// Release the pointer lock. Safe when nothing is locked.
pub fn unlock() {
    let Some(shared) = shared() else {
        return;
    };
    if let Some(locked) = shared.locked.lock().unwrap_or_else(|p| p.into_inner()).take() {
        locked.destroy();
        let _ = shared.conn.flush();
    }
}

/// Drain the pointer motion accumulated since the last call.
pub fn take_delta() -> (f32, f32) {
    let Some(shared) = shared() else {
        return (0.0, 0.0);
    };
    let mut delta = shared.delta.lock().unwrap_or_else(|p| p.into_inner());
    let (dx, dy) = std::mem::take(&mut *delta);
    (dx as f32, dy as f32)
}

/// Discard accumulated motion (start of a drag).
pub fn reset_delta() {
    let _ = take_delta();
}

/// The camera keys currently held, as `device_query` keycodes.
pub fn held_keys() -> Vec<Keycode> {
    let Some(shared) = shared() else {
        return Vec::new();
    };
    let keys = shared.keys.lock().unwrap_or_else(|p| p.into_inner());
    keys.iter().filter_map(|&code| evdev_to_keycode(code)).collect()
}

/// evdev key codes (`linux/input-event-codes.h`) of the keys the camera reads.
fn evdev_to_keycode(code: u32) -> Option<Keycode> {
    Some(match code {
        16 => Keycode::Q,
        17 => Keycode::W,
        18 => Keycode::E,
        29 => Keycode::LControl,
        30 => Keycode::A,
        31 => Keycode::S,
        32 => Keycode::D,
        42 => Keycode::LShift,
        54 => Keycode::RShift,
        57 => Keycode::Space,
        97 => Keycode::RControl,
        _ => return None,
    })
}

// ── Event handling (dispatch thread) ─────────────────────────────────────────

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(caps),
        } = event
        else {
            return;
        };
        let mut pointer = state.shared.pointer.lock().unwrap_or_else(|p| p.into_inner());
        if caps.contains(wl_seat::Capability::Pointer) && pointer.is_none() {
            let new_pointer = state.seat.get_pointer(qh, ());
            if let Some(manager) = &state.relative_manager {
                state.relative = Some(manager.get_relative_pointer(&new_pointer, qh, ()));
            }
            *pointer = Some(new_pointer);
        }
        if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
            state.keyboard = Some(state.seat.get_keyboard(qh, ()));
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut keys = state.shared.keys.lock().unwrap_or_else(|p| p.into_inner());
        match event {
            wl_keyboard::Event::Enter { keys: held, .. } => {
                keys.clear();
                keys.extend(
                    held.chunks_exact(4)
                        .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]])),
                );
            }
            // Focus left the window: nothing it sees is held any more.
            wl_keyboard::Event::Leave { .. } => keys.clear(),
            wl_keyboard::Event::Key { key, state: key_state, .. } => match key_state {
                WEnum::Value(wl_keyboard::KeyState::Pressed) => {
                    keys.insert(key);
                }
                _ => {
                    keys.remove(&key);
                }
            },
            // The keymap fd is dropped (closed) here; codes are read raw.
            _ => {}
        }
    }
}

impl Dispatch<ZwpRelativePointerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpRelativePointerV1,
        event: zwp_relative_pointer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Accelerated deltas, matching the pointer-position deltas the X11
        // and Windows paths measure.
        if let zwp_relative_pointer_v1::Event::RelativeMotion { dx, dy, .. } = event {
            let mut delta = state.shared.delta.lock().unwrap_or_else(|p| p.into_inner());
            delta.0 += dx;
            delta.1 += dy;
        }
    }
}

delegate_noop!(State: ignore wl_pointer::WlPointer);
delegate_noop!(State: ignore ZwpLockedPointerV1);
delegate_noop!(State: ZwpPointerConstraintsV1);
delegate_noop!(State: ZwpRelativePointerManagerV1);
