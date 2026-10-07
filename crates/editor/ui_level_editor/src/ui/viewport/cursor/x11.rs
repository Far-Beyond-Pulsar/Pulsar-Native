//! X11 cursor control for the viewport's mouse-look: pointer grab, warp and
//! query, all on one shared X connection (see `SharedDisplay`).
use core::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub type Display = c_void;
pub type XID = core::ffi::c_ulong;
pub type Window = XID;
pub type Cursor = XID;
pub type Pixmap = XID;
pub type Drawable = XID;

const GRAB_MODE_ASYNC: core::ffi::c_int = 1;
const BUTTON_PRESS_MASK: u32 = 1 << 2;
const BUTTON_RELEASE_MASK: u32 = 1 << 3;
const POINTER_MOTION_MASK: u32 = 1 << 6;
const CW_OVERRIDE_REDIRECT: core::ffi::c_ulong = 1 << 9;

static CONFINE_WINDOW: AtomicU64 = AtomicU64::new(0);
static CONFINE_ACTIVE: AtomicBool = AtomicBool::new(false);

#[repr(C)]
struct XSetWindowAttributes {
    background_pixmap: XID,
    background_pixel: core::ffi::c_ulong,
    border_pixmap: XID,
    border_pixel: core::ffi::c_ulong,
    bit_gravity: core::ffi::c_int,
    win_gravity: core::ffi::c_int,
    backing_store: core::ffi::c_int,
    backing_planes: core::ffi::c_ulong,
    backing_pixel: core::ffi::c_ulong,
    save_under: core::ffi::c_int,
    event_mask: core::ffi::c_long,
    do_not_propagate_mask: core::ffi::c_long,
    override_redirect: core::ffi::c_int,
    colormap: XID,
    cursor: Cursor,
}

#[repr(C)]
struct XColor {
    pixel: core::ffi::c_ulong,
    red: u16,
    green: u16,
    blue: u16,
    flags: u8,
    pad: u8,
}

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(display_name: *const c_void) -> *mut Display;
    fn XDefaultRootWindow(display: *mut Display) -> Window;
    fn XGetInputFocus(
        display: *mut Display,
        focus_ret: *mut Window,
        revert_ret: *mut core::ffi::c_int,
    ) -> core::ffi::c_int;
    fn XFlush(display: *mut Display) -> core::ffi::c_int;

    fn XGrabPointer(
        display: *mut Display,
        grab_window: Window,
        owner_events: core::ffi::c_int,
        event_mask: u32,
        pointer_mode: core::ffi::c_int,
        keyboard_mode: core::ffi::c_int,
        confine_to: Window,
        cursor: Cursor,
        time: XID,
    ) -> core::ffi::c_int;
    fn XUngrabPointer(display: *mut Display, time: XID) -> core::ffi::c_int;

    fn XWarpPointer(
        display: *mut Display,
        src_w: Window,
        dest_w: Window,
        src_x: core::ffi::c_int,
        src_y: core::ffi::c_int,
        src_width: core::ffi::c_uint,
        src_height: core::ffi::c_uint,
        dest_x: core::ffi::c_int,
        dest_y: core::ffi::c_int,
    ) -> core::ffi::c_int;
    fn XTranslateCoordinates(
        display: *mut Display,
        src_w: Window,
        dest_w: Window,
        src_x: core::ffi::c_int,
        src_y: core::ffi::c_int,
        dest_x_ret: *mut core::ffi::c_int,
        dest_y_ret: *mut core::ffi::c_int,
        child_ret: *mut Window,
    ) -> core::ffi::c_int;

    fn XCreatePixmapCursor(
        display: *mut Display,
        source: Pixmap,
        mask: Pixmap,
        fg_color: *const XColor,
        bg_color: *const XColor,
        x: core::ffi::c_uint,
        y: core::ffi::c_uint,
    ) -> Cursor;
    fn XFreeCursor(display: *mut Display, cursor: Cursor) -> core::ffi::c_int;
    fn XFreePixmap(display: *mut Display, pixmap: Pixmap) -> core::ffi::c_int;
    fn XCreatePixmap(
        display: *mut Display,
        d: Drawable,
        width: core::ffi::c_uint,
        height: core::ffi::c_uint,
        depth: core::ffi::c_uint,
    ) -> Pixmap;

    fn XDefineCursor(display: *mut Display, window: Window, cursor: Cursor) -> core::ffi::c_int;
    fn XUndefineCursor(display: *mut Display, window: Window) -> core::ffi::c_int;
    fn XCreateSimpleWindow(
        display: *mut Display,
        parent: Window,
        x: core::ffi::c_int,
        y: core::ffi::c_int,
        width: core::ffi::c_uint,
        height: core::ffi::c_uint,
        border_width: core::ffi::c_uint,
        border: core::ffi::c_ulong,
        background: core::ffi::c_ulong,
    ) -> Window;
    fn XDestroyWindow(display: *mut Display, window: Window) -> core::ffi::c_int;
    fn XChangeWindowAttributes(
        display: *mut Display,
        window: Window,
        value_mask: core::ffi::c_ulong,
        attributes: *const XSetWindowAttributes,
    ) -> core::ffi::c_int;
    fn XQueryPointer(
        display: *mut Display,
        window: Window,
        root_ret: *mut Window,
        child_ret: *mut Window,
        root_x_ret: *mut core::ffi::c_int,
        root_y_ret: *mut core::ffi::c_int,
        win_x_ret: *mut core::ffi::c_int,
        win_y_ret: *mut core::ffi::c_int,
        mask_ret: *mut core::ffi::c_uint,
    ) -> core::ffi::c_int;
}

// ── The shared connection ────────────────────────────────────────────────────

/// The one X connection every function here uses.
///
/// Opened on first use and kept for the life of the process. Two reasons:
///
/// - **No leaks.** The input thread warps and queries the pointer on every
///   poll (~500 Hz) during mouse-look. Opening a display per call used to leak
///   one client per poll whenever a path forgot `XCloseDisplay`; the X server
///   allows ~256 clients, after which every `XOpenDisplay` in the process
///   (winit's included) fails with "Maximum number of clients reached" and the
///   editor's event loop dies. With a single connection there is nothing to
///   forget to close.
/// - **Grabs that last.** X releases a client's pointer grabs, defined cursors
///   and windows when that client disconnects. A grab made on a connection
///   that is then closed ends immediately; on the shared connection it lasts
///   until [`unlock_cursor`].
///
/// Xlib displays aren't thread-safe, so the connection is only touched while
/// the mutex is held (UI thread for lock/unlock, input thread for warps).
struct SharedDisplay(*mut Display);

// SAFETY: the pointer is only used by Xlib while `SHARED_DISPLAY`'s mutex is
// held, so no two threads use the connection at once.
unsafe impl Send for SharedDisplay {}

static SHARED_DISPLAY: std::sync::Mutex<Option<SharedDisplay>> = std::sync::Mutex::new(None);

/// Set once opening a display has failed (no X server): stops retrying, and
/// the warning, on every poll.
static DISPLAY_UNAVAILABLE: AtomicBool = AtomicBool::new(false);

/// Run `f` with the shared connection, opening it on first use. `None` when
/// no X display is available.
fn with_display<R>(f: impl FnOnce(*mut Display) -> R) -> Option<R> {
    if DISPLAY_UNAVAILABLE.load(Ordering::Relaxed) {
        return None;
    }
    let mut guard = SHARED_DISPLAY.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        let display = unsafe { XOpenDisplay(std::ptr::null()) };
        if display.is_null() {
            DISPLAY_UNAVAILABLE.store(true, Ordering::Relaxed);
            tracing::warn!("[VIEWPORT] X11: no X display; cursor control disabled");
            return None;
        }
        *guard = Some(SharedDisplay(display));
    }
    guard.as_ref().map(|shared| f(shared.0))
}

/// The X11 window behind a gpui window, if it is an X11 window.
fn x11_window(window: &gpui::Window) -> Option<Window> {
    let handle = raw_window_handle::HasWindowHandle::window_handle(window).ok()?;
    match handle.as_raw() {
        raw_window_handle::RawWindowHandle::Xlib(h) => Some(h.window as XID),
        raw_window_handle::RawWindowHandle::Xcb(h) => Some(h.window.get() as XID),
        _ => None,
    }
}

fn focused_window(display: *mut Display) -> Option<Window> {
    let mut window: Window = 0;
    let mut revert: core::ffi::c_int = 0;
    let status = unsafe { XGetInputFocus(display, &mut window, &mut revert) };
    if status == 0 || window == 0 {
        tracing::warn!("[VIEWPORT] X11: no focused window");
        None
    } else {
        Some(window)
    }
}

fn blank_cursor(display: *mut Display) -> Option<Cursor> {
    let pixmap = unsafe { XCreatePixmap(display, XDefaultRootWindow(display), 1, 1, 1) };
    if pixmap == 0 {
        return None;
    }
    let black = XColor {
        pixel: 0,
        red: 0,
        green: 0,
        blue: 0,
        flags: 0,
        pad: 0,
    };
    let cursor = unsafe { XCreatePixmapCursor(display, pixmap, pixmap, &black, &black, 0, 0) };
    unsafe { XFreePixmap(display, pixmap) };
    if cursor == 0 {
        None
    } else {
        Some(cursor)
    }
}

// ── Cursor control ───────────────────────────────────────────────────────────

pub fn hide_cursor() {
    with_display(|display| {
        let Some(win) = focused_window(display) else {
            return;
        };
        if let Some(cursor) = blank_cursor(display) {
            unsafe {
                XDefineCursor(display, win, cursor);
                XFreeCursor(display, cursor);
                XFlush(display);
            }
            tracing::debug!("[VIEWPORT] 👻 Cursor hidden (X11 blank cursor)");
        }
    });
}

pub fn show_cursor() {
    with_display(|display| {
        let Some(win) = focused_window(display) else {
            return;
        };
        unsafe {
            XUndefineCursor(display, win);
            XFlush(display);
        }
        tracing::debug!("[VIEWPORT] 👁️ Cursor shown (X11 undefine)");
    });
}

/// Grab the pointer to `window` until [`unlock_cursor`].
pub fn lock_cursor_to_window(window: &gpui::Window) {
    let Some(x11_window) = x11_window(window) else {
        tracing::warn!("[VIEWPORT] X11: not an X11 window handle");
        return;
    };
    with_display(|display| {
        let status = unsafe {
            XGrabPointer(
                display,
                x11_window,
                0,
                BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK | POINTER_MOTION_MASK,
                GRAB_MODE_ASYNC,
                GRAB_MODE_ASYNC,
                x11_window,
                0,
                0,
            )
        };
        unsafe { XFlush(display) };
        if status == 0 {
            tracing::debug!("[VIEWPORT] 🔒 Cursor locked to X11 window");
        } else {
            tracing::warn!("[VIEWPORT] X11: XGrabPointer failed (status={status})");
        }
    });
}

/// Confine the pointer to a square of `radius` around a screen point, via an
/// invisible override-redirect window, until [`unlock_cursor`].
pub fn lock_cursor_to_point(screen_x: i32, screen_y: i32, radius: i32) {
    with_display(|display| {
        let root = unsafe { XDefaultRootWindow(display) };
        let mut attrs: XSetWindowAttributes = unsafe { std::mem::zeroed() };
        attrs.override_redirect = 1;
        let confine_win = unsafe {
            XCreateSimpleWindow(
                display,
                root,
                screen_x - radius,
                screen_y - radius,
                (radius * 2) as core::ffi::c_uint,
                (radius * 2) as core::ffi::c_uint,
                0,
                0,
                0,
            )
        };
        if confine_win == 0 {
            tracing::warn!("[VIEWPORT] X11: failed to create confine window");
            return;
        }
        unsafe { XChangeWindowAttributes(display, confine_win, CW_OVERRIDE_REDIRECT, &attrs) };

        let status = unsafe {
            XGrabPointer(
                display,
                confine_win,
                0,
                BUTTON_PRESS_MASK | BUTTON_RELEASE_MASK | POINTER_MOTION_MASK,
                GRAB_MODE_ASYNC,
                GRAB_MODE_ASYNC,
                confine_win,
                0,
                0,
            )
        };
        if status == 0 {
            CONFINE_WINDOW.store(confine_win, Ordering::Relaxed);
            CONFINE_ACTIVE.store(true, Ordering::Relaxed);
            tracing::debug!(
                "[VIEWPORT] 🔒 Cursor confined to {}px radius around ({}, {}) via X11",
                radius,
                screen_x,
                screen_y
            );
        } else {
            tracing::warn!("[VIEWPORT] X11: XGrabPointer failed (status={status})");
            unsafe { XDestroyWindow(display, confine_win) };
        }
        unsafe { XFlush(display) };
    });
}

/// Release any grab from [`lock_cursor_to_window`] / [`lock_cursor_to_point`]
/// and destroy the confine window. Safe to call when nothing is locked.
pub fn unlock_cursor() {
    with_display(|display| {
        unsafe { XUngrabPointer(display, 0) };
        if CONFINE_ACTIVE.swap(false, Ordering::Relaxed) {
            let win = CONFINE_WINDOW.swap(0, Ordering::Relaxed);
            if win != 0 {
                unsafe { XDestroyWindow(display, win) };
            }
            tracing::debug!("[VIEWPORT] 🔓 Cursor unlocked (X11)");
        }
        unsafe { XFlush(display) };
    });
}

// ── Pointer position (called every input-thread poll) ────────────────────────

pub fn set_cursor_position(screen_x: i32, screen_y: i32) {
    with_display(|display| unsafe {
        let root = XDefaultRootWindow(display);
        XWarpPointer(display, 0, root, 0, 0, 0, 0, screen_x, screen_y);
        XFlush(display);
    });
}

pub fn get_cursor_position() -> Option<(i32, i32)> {
    with_display(|display| {
        let root = unsafe { XDefaultRootWindow(display) };
        let (mut root_x, mut root_y, mut win_x, mut win_y) = (0, 0, 0, 0);
        let mut mask: core::ffi::c_uint = 0;
        let (mut root_ret, mut child_ret): (Window, Window) = (0, 0);
        let status = unsafe {
            XQueryPointer(
                display,
                root,
                &mut root_ret,
                &mut child_ret,
                &mut root_x,
                &mut root_y,
                &mut win_x,
                &mut win_y,
                &mut mask,
            )
        };
        (status != 0).then_some((root_x as i32, root_y as i32))
    })
    .flatten()
}

/// Window-relative coordinates of `window` to root (screen) coordinates.
pub fn window_to_screen_position(
    window: &gpui::Window,
    window_x: f32,
    window_y: f32,
) -> Option<(i32, i32)> {
    let x11_window = x11_window(window)?;
    with_display(|display| {
        let root = unsafe { XDefaultRootWindow(display) };
        let (mut dest_x, mut dest_y): (core::ffi::c_int, core::ffi::c_int) = (0, 0);
        let mut child: XID = 0;
        let ok = unsafe {
            XTranslateCoordinates(
                display,
                x11_window,
                root,
                window_x as core::ffi::c_int,
                window_y as core::ffi::c_int,
                &mut dest_x,
                &mut dest_y,
                &mut child,
            )
        };
        (ok != 0).then_some((dest_x as i32, dest_y as i32))
    })
    .flatten()
}
