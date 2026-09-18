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