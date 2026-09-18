use std::sync::atomic::{AtomicBool, Ordering};
use gpui::Window;

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
