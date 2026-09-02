//! Platform-specific cursor and input handling.
//!
//! This module provides cross-platform cursor locking, hiding, and positioning
//! for viewport camera controls. Each platform (Windows, macOS, Linux) has its
//! own implementation using native APIs for precise cursor control.

use gpui::Window;

#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};

/// Lock cursor to window bounds (prevents cursor from leaving the window).
///
/// This is used to keep the cursor confined during camera rotation to prevent
/// accidental clicks outside the viewport.
#[cfg(target_os = "windows")]
pub fn lock_cursor_to_window(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winapi::shared::windef::{POINT, RECT};
    use winapi::um::winuser::{ClientToScreen, ClipCursor, GetClientRect};

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::Win32(win32_handle) => unsafe {
                let hwnd = win32_handle.hwnd.get() as *mut winapi::shared::windef::HWND__;

                let mut client_rect = RECT {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                };

                if GetClientRect(hwnd, &mut client_rect) != 0 {
                    let mut top_left = POINT {
                        x: client_rect.left,
                        y: client_rect.top,
                    };
                    let mut bottom_right = POINT {
                        x: client_rect.right,
                        y: client_rect.bottom,
                    };

                    ClientToScreen(hwnd, &mut top_left);
                    ClientToScreen(hwnd, &mut bottom_right);

                    let screen_rect = RECT {
                        left: top_left.x,
                        top: top_left.y,
                        right: bottom_right.x,
                        bottom: bottom_right.y,
                    };

                    ClipCursor(&screen_rect);
                    tracing::debug!("[VIEWPORT] 🔒 Cursor locked to window bounds");
                }
            },
            _ => {
                tracing::warn!("[VIEWPORT] Not a Win32 window handle");
            }
        },
        Err(e) => {
            tracing::error!("[VIEWPORT] Failed to get window handle: {:?}", e);
        }
    }
}

/// Lock cursor to a small area around a specific point.
///
/// This prevents the cursor from escaping during fast mouse movements,
/// which is critical for smooth camera rotation.
///
/// # Arguments
/// * `screen_x` - X coordinate in screen space
/// * `screen_y` - Y coordinate in screen space
/// * `radius` - Size of the confinement area in pixels
#[cfg(target_os = "windows")]
pub fn lock_cursor_to_point(screen_x: i32, screen_y: i32, radius: i32) {
    use winapi::shared::windef::RECT;
    use winapi::um::winuser::ClipCursor;

    unsafe {
        let screen_rect = RECT {
            left: screen_x - radius,
            top: screen_y - radius,
            right: screen_x + radius,
            bottom: screen_y + radius,
        };
        ClipCursor(&screen_rect);
        tracing::debug!(
            "[VIEWPORT] 🔒 Cursor confined to {}px radius around ({}, {})",
            radius,
            screen_x,
            screen_y
        );
    }
}

/// Release cursor confinement.
#[cfg(target_os = "windows")]
pub fn unlock_cursor() {
    use winapi::um::winuser::ClipCursor;

    unsafe {
        ClipCursor(std::ptr::null());
        tracing::debug!("[VIEWPORT] 🔓 Cursor unlocked");
    }
}

/// Hide the system cursor.
///
/// Windows uses a counter-based system, so we ensure the counter is negative.
#[cfg(target_os = "windows")]
pub fn hide_cursor() {
    use winapi::shared::minwindef::FALSE;
    use winapi::um::winuser::ShowCursor;

    unsafe {
        while ShowCursor(FALSE) >= 0 {}
        tracing::debug!("[VIEWPORT] 👻 Cursor hidden (Win32 ShowCursor)");
    }
}

/// Show the system cursor.
///
/// Windows uses a counter-based system, so we ensure the counter is non-negative.
#[cfg(target_os = "windows")]
pub fn show_cursor() {
    use winapi::um::winuser::ShowCursor;

    unsafe {
        while ShowCursor(1) < 0 {}
        tracing::debug!("[VIEWPORT] 👁️ Cursor shown (Win32 ShowCursor)");
    }
}

/// Set cursor to absolute screen position.
#[cfg(target_os = "windows")]
pub fn set_cursor_position(screen_x: i32, screen_y: i32) {
    use winapi::um::winuser::SetCursorPos;

    unsafe {
        SetCursorPos(screen_x, screen_y);
    }
}

#[cfg(target_os = "windows")]
pub fn get_cursor_position() -> Option<(i32, i32)> {
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::GetCursorPos;

    unsafe {
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) != 0 {
            Some((point.x, point.y))
        } else {
            None
        }
    }
}

/// Convert window-relative coordinates to screen coordinates.
///
/// # Returns
/// `Some((x, y))` if successful, `None` if the window handle is invalid.
#[cfg(target_os = "windows")]
pub fn window_to_screen_position(
    window: &Window,
    window_x: f32,
    window_y: f32,
) -> Option<(i32, i32)> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::ClientToScreen;

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::Win32(win32_handle) => unsafe {
                let hwnd = win32_handle.hwnd.get() as *mut winapi::shared::windef::HWND__;
                let mut point = POINT {
                    x: window_x as i32,
                    y: window_y as i32,
                };
                ClientToScreen(hwnd, &mut point);
                Some((point.x, point.y))
            },
            _ => None,
        },
        Err(_) => None,
    }
}

// macOS implementations
#[cfg(target_os = "macos")]
static ACCESSIBILITY_PROMPTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: *const core::ffi::c_void) -> bool;
}

#[cfg(target_os = "macos")]
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: *const core::ffi::c_void;
    static kCFBooleanFalse: *const core::ffi::c_void;
}

#[cfg(target_os = "macos")]
fn is_accessibility_trusted(prompt_if_missing: bool) -> bool {
    use core::ffi::c_void;
    use core_foundation_sys::base::{kCFAllocatorDefault, CFRelease};
    use core_foundation_sys::dictionary::{
        kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionaryCreate,
    };
    use core_foundation_sys::string::{kCFStringEncodingUTF8, CFStringCreateWithCString};

    unsafe {
        let key = CFStringCreateWithCString(
            kCFAllocatorDefault,
            b"AXTrustedCheckOptionPrompt\0".as_ptr().cast(),
            kCFStringEncodingUTF8,
        );

        if key.is_null() {
            return AXIsProcessTrusted();
        }

        let keys: [*const c_void; 1] = [key.cast()];
        let values: [*const c_void; 1] = [if prompt_if_missing {
            kCFBooleanTrue
        } else {
            kCFBooleanFalse
        }];

        let options = CFDictionaryCreate(
            kCFAllocatorDefault,
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        );

        let trusted = if options.is_null() {
            AXIsProcessTrusted()
        } else {
            let result = AXIsProcessTrustedWithOptions(options.cast());
            CFRelease(options.cast());
            result
        };

        CFRelease(key.cast());
        trusted
    }
}

#[cfg(target_os = "macos")]
fn request_accessibility_permission_once() {
    if ACCESSIBILITY_PROMPTED.swap(true, Ordering::AcqRel) {
        return;
    }

    let _ = is_accessibility_trusted(true);
}

#[cfg(target_os = "macos")]
fn ensure_accessibility(prompt_if_missing: bool) -> bool {
    if is_accessibility_trusted(false) {
        return true;
    }

    if prompt_if_missing {
        request_accessibility_permission_once();
        tracing::warn!(
            "[VIEWPORT] macOS Accessibility permission is required for relative mouse mode. Waiting for user approval."
        );
    }

    false
}

#[cfg(target_os = "macos")]
pub fn prepare_relative_mouse_mode() -> bool {
    ensure_accessibility(false)
}

#[cfg(not(target_os = "macos"))]
pub fn prepare_relative_mouse_mode() -> bool {
    true
}

#[cfg(target_os = "macos")]
pub fn set_cursor_position(screen_x: i32, screen_y: i32) {
    if !ensure_accessibility(false) {
        return;
    }

    // core-graphics2 is pulled in under the macos target; the crate alias
    // is `core_graphics2` (see Cargo.toml).  adjust imports accordingly.
    use core_graphics2::display::CGDisplay;
    use core_graphics2::event_source::CGEventSource;
    use core_graphics2::event_types::CGEventSourceStateID;

    unsafe {
        // warp the cursor using the geometry type from the new crate
        let _ = CGDisplay::main().warp_mouse_cursor_position(core_graphics2::geometry::CGPoint {
            x: screen_x as f64,
            y: screen_y as f64,
        });

        // Disassociate mouse and cursor position momentarily to prevent jumping.
        // the newer binding exposes this as an *instance* method; create a
        // temporary source and adjust the interval. failure to create the
        // source is non‑fatal, so ignore the result.
        if let Ok(src) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
            src.set_local_events_suppression_interval(0.0);
        }
    }
}

#[cfg(target_os = "macos")]
pub fn begin_relative_mouse_mode() {
    if !ensure_accessibility(false) {
        return;
    }

    use core_graphics2::direct_display::CGGetLastMouseDelta;
    use core_graphics2::display::CGDisplay;
    use core_graphics2::event_source::CGEventSource;
    use core_graphics2::event_types::CGEventSourceStateID;
    use core_graphics2::remote_operation::CGAssociateMouseAndMouseCursorPosition;

    unsafe {
        let _ = CGAssociateMouseAndMouseCursorPosition(0);
    }

    let _ = CGDisplay::main().hide_cursor();

    if let Ok(src) = CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
        src.set_local_events_suppression_interval(0.0);
    }

    let mut delta_x = 0;
    let mut delta_y = 0;
    unsafe {
        let _ = CGGetLastMouseDelta(&mut delta_x, &mut delta_y);
    }
}

#[cfg(target_os = "macos")]
pub fn end_relative_mouse_mode() {
    if !ensure_accessibility(false) {
        return;
    }

    use core_graphics2::display::CGDisplay;
    use core_graphics2::remote_operation::CGAssociateMouseAndMouseCursorPosition;

    unsafe {
        let _ = CGAssociateMouseAndMouseCursorPosition(1);
    }

    let _ = CGDisplay::main().show_cursor();
}

#[cfg(target_os = "macos")]
pub fn take_mouse_delta() -> (f32, f32) {
    if !ensure_accessibility(false) {
        return (0.0, 0.0);
    }

    use core_graphics2::direct_display::CGGetLastMouseDelta;

    let mut delta_x = 0;
    let mut delta_y = 0;
    unsafe {
        let _ = CGGetLastMouseDelta(&mut delta_x, &mut delta_y);
    }

    (delta_x as f32, delta_y as f32)
}

#[cfg(target_os = "macos")]
pub fn get_cursor_position() -> Option<(i32, i32)> {
    None
}

#[cfg(target_os = "macos")]
pub fn window_to_screen_position(
    window: &Window,
    window_x: f32,
    window_y: f32,
) -> Option<(i32, i32)> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSPoint;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    match HasWindowHandle::window_handle(window) {
        Ok(handle) => match handle.as_raw() {
            RawWindowHandle::AppKit(appkit_handle) => unsafe {
                // `RawWindowHandle::AppKit` exposes an `ns_view` pointer but not
                // the window directly. Query the view's window object at runtime.
                let ns_view = appkit_handle.ns_view.as_ptr() as *mut AnyObject;
                if ns_view.is_null() {
                    return None;
                }

                let ns_window: *mut AnyObject = msg_send![ns_view, window];
                if ns_window.is_null() {
                    return None;
                }

                let point = NSPoint {
                    x: window_x as f64,
                    y: window_y as f64,
                };
                let screen_point: NSPoint = msg_send![ns_window, convertPointToScreen: point];
                Some((screen_point.x as i32, screen_point.y as i32))
            },
            _ => None,
        },
        Err(_) => None,
    }
}

#[cfg(target_os = "macos")]
pub fn lock_cursor_to_window(_window: &Window) {
    // macOS doesn't support cursor confinement natively
    // We rely on relative mouse mode instead.
}

#[cfg(target_os = "macos")]
pub fn lock_cursor_to_point(_screen_x: i32, _screen_y: i32, _radius: i32) {
    // No-op on macOS
}

#[cfg(target_os = "macos")]
pub fn unlock_cursor() {
    // No-op on macOS
}

#[cfg(target_os = "macos")]
pub fn hide_cursor() {
    // macOS cursor hiding is typically handled through GPUI/window system
}

#[cfg(target_os = "macos")]
pub fn show_cursor() {
    // macOS cursor showing is typically handled through GPUI/window system
}

#[cfg(not(target_os = "macos"))]
pub fn begin_relative_mouse_mode() {}

#[cfg(not(target_os = "macos"))]
pub fn end_relative_mouse_mode() {}

#[cfg(not(target_os = "macos"))]
pub fn take_mouse_delta() -> (f32, f32) {
    (0.0, 0.0)
}

// ── Linux / X11 implementations ───────────────────────────────────────────
//
// Uses raw Xlib FFI linked to libX11 (present on every X11 desktop).
// Wayland compositors typically don't support pointer confinement via this
// path; cursor operations silently no-op on Wayland since XWayland is
// required for these to work and isn't guaranteed to be present.

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod x11 {
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
        fn XCloseDisplay(display: *mut Display) -> core::ffi::c_int;
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

        fn XDefineCursor(display: *mut Display, window: Window, cursor: Cursor)
            -> core::ffi::c_int;
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

    fn open_display() -> Option<*mut Display> {
        let d = unsafe { XOpenDisplay(std::ptr::null()) };
        if d.is_null() {
            tracing::warn!("[VIEWPORT] X11: failed to open display");
            None
        } else {
            Some(d)
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
        let fg = XColor {
            pixel: 0,
            red: 0,
            green: 0,
            blue: 0,
            flags: 0,
            pad: 0,
        };
        let bg = XColor {
            pixel: 0,
            red: 0,
            green: 0,
            blue: 0,
            flags: 0,
            pad: 0,
        };
        let cursor = unsafe { XCreatePixmapCursor(display, pixmap, pixmap, &fg, &bg, 0, 0) };
        unsafe { XFreePixmap(display, pixmap) };
        if cursor == 0 {
            None
        } else {
            Some(cursor)
        }
    }

    pub fn hide_cursor() {
        let Some(display) = open_display() else {
            return;
        };
        let Some(win) = focused_window(display) else {
            unsafe { XCloseDisplay(display) };
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
        unsafe {
            XCloseDisplay(display);
        }
    }

    pub fn show_cursor() {
        let Some(display) = open_display() else {
            return;
        };
        let Some(win) = focused_window(display) else {
            unsafe { XCloseDisplay(display) };
            return;
        };
        unsafe {
            XUndefineCursor(display, win);
            XFlush(display);
        }
        tracing::debug!("[VIEWPORT] 👁️ Cursor shown (X11 undefine)");
        unsafe {
            XCloseDisplay(display);
        }
    }

    pub fn lock_cursor_to_window(window: &gpui::Window) {
        let Some(display) = open_display() else {
            return;
        };
        let raw_handle = unsafe { raw_window_handle::HasWindowHandle::window_handle(window) };
        let x11_window = match raw_handle {
            Ok(handle) => match handle.as_raw() {
                raw_window_handle::RawWindowHandle::Xlib(h) => h.window as XID,
                raw_window_handle::RawWindowHandle::Xcb(h) => h.window.get() as XID,
                _ => {
                    tracing::warn!("[VIEWPORT] X11: not an X11 window handle");
                    unsafe { XCloseDisplay(display) };
                    return;
                }
            },
            Err(e) => {
                tracing::warn!("[VIEWPORT] X11: failed to get window handle: {:?}", e);
                unsafe { XCloseDisplay(display) };
                return;
            }
        };
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
        if status == 0 {
            tracing::debug!("[VIEWPORT] 🔒 Cursor locked to X11 window");
        } else {
            tracing::warn!("[VIEWPORT] X11: XGrabPointer failed (status={status})");
        }
        unsafe {
            XCloseDisplay(display);
        }
    }

    pub fn lock_cursor_to_point(screen_x: i32, screen_y: i32, radius: i32) {
        let Some(display) = open_display() else {
            return;
        };
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
            unsafe { XCloseDisplay(display) };
            return;
        }
        unsafe {
            XChangeWindowAttributes(display, confine_win, CW_OVERRIDE_REDIRECT, &attrs);
        }

        CONFINE_WINDOW.store(confine_win, Ordering::Relaxed);

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
            CONFINE_ACTIVE.store(true, Ordering::Relaxed);
            tracing::debug!(
                "[VIEWPORT] 🔒 Cursor confined to {}px radius around ({}, {}) via X11",
                radius,
                screen_x,
                screen_y
            );
        } else {
            tracing::warn!("[VIEWPORT] X11: XGrabPointer failed (status={status})");
            unsafe {
                XDestroyWindow(display, confine_win);
            }
            CONFINE_WINDOW.store(0, Ordering::Relaxed);
        }
        unsafe {
            XCloseDisplay(display);
        }
    }

    pub fn unlock_cursor() {
        let Some(display) = open_display() else {
            return;
        };
        unsafe {
            XUngrabPointer(display, 0);
            XFlush(display);
        }
        if CONFINE_ACTIVE.swap(false, Ordering::Relaxed) {
            let win = CONFINE_WINDOW.swap(0, Ordering::Relaxed);
            if win != 0 {
                unsafe {
                    XDestroyWindow(display, win);
                }
            }
            tracing::debug!("[VIEWPORT] 🔓 Cursor unlocked (X11)");
        }
        unsafe {
            XCloseDisplay(display);
        }
    }

    pub fn set_cursor_position(screen_x: i32, screen_y: i32) {
        let Some(display) = open_display() else {
            return;
        };
        let root = unsafe { XDefaultRootWindow(display) };
        unsafe {
            XWarpPointer(display, 0, root, 0, 0, 0, 0, screen_x, screen_y);
            XFlush(display);
        }
    }

    pub fn get_cursor_position() -> Option<(i32, i32)> {
        let display = open_display()?;
        let root = unsafe { XDefaultRootWindow(display) };
        let mut root_x: core::ffi::c_int = 0;
        let mut root_y: core::ffi::c_int = 0;
        let mut win_x: core::ffi::c_int = 0;
        let mut win_y: core::ffi::c_int = 0;
        let mut mask: core::ffi::c_uint = 0;
        let mut root_ret: Window = 0;
        let mut child_ret: Window = 0;
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
        unsafe { XCloseDisplay(display) };
        if status == 0 {
            None
        } else {
            Some((root_x as i32, root_y as i32))
        }
    }

    pub fn window_to_screen_position(
        window: &gpui::Window,
        window_x: f32,
        window_y: f32,
    ) -> Option<(i32, i32)> {
        let display = open_display()?;
        let raw_handle = unsafe { raw_window_handle::HasWindowHandle::window_handle(window) };
        let x11_window = match raw_handle {
            Ok(handle) => match handle.as_raw() {
                raw_window_handle::RawWindowHandle::Xlib(h) => h.window as XID,
                raw_window_handle::RawWindowHandle::Xcb(h) => h.window.get() as XID,
                _ => {
                    unsafe { XCloseDisplay(display) };
                    return None;
                }
            },
            Err(_) => {
                unsafe { XCloseDisplay(display) };
                return None;
            }
        };
        let root = unsafe { XDefaultRootWindow(display) };
        let mut dest_x: core::ffi::c_int = 0;
        let mut dest_y: core::ffi::c_int = 0;
        let mut child: XID = 0;
        unsafe {
            XTranslateCoordinates(
                display,
                x11_window,
                root,
                window_x as core::ffi::c_int,
                window_y as core::ffi::c_int,
                &mut dest_x,
                &mut dest_y,
                &mut child,
            );
            XCloseDisplay(display);
        }
        Some((dest_x as i32, dest_y as i32))
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub use x11::{
    get_cursor_position, hide_cursor, lock_cursor_to_point, lock_cursor_to_window,
    set_cursor_position, show_cursor, unlock_cursor, window_to_screen_position,
};
