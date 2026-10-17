use gpui::Window;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

static RAW_MOUSE_READY: OnceLock<bool> = OnceLock::new();
static RAW_MOUSE_CAPTURED: AtomicBool = AtomicBool::new(false);
static RAW_MOUSE_DELTA_X: AtomicI64 = AtomicI64::new(0);
static RAW_MOUSE_DELTA_Y: AtomicI64 = AtomicI64::new(0);

/// Start a message-only window that receives unaccelerated relative mouse
/// reports. Cursor-position polling remains available as a fallback if Windows
/// cannot register Raw Input on this machine.
#[cfg(target_os = "windows")]
pub fn prepare_relative_mouse_mode() -> bool {
    let _ = RAW_MOUSE_READY.get_or_init(|| {
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || unsafe {
            use std::mem::{size_of, zeroed};
            use std::ptr::{null, null_mut};
            use winapi::shared::minwindef::{ATOM, LPARAM, LRESULT, UINT, WPARAM};
            use winapi::shared::windef::HWND;
            use winapi::um::libloaderapi::GetModuleHandleW;
            use winapi::um::winuser::{
                CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
                RAWINPUTDEVICE, RIDEV_INPUTSINK, RegisterClassW, RegisterRawInputDevices,
                TranslateMessage, WM_INPUT, WNDCLASSW,
            };

            const CLASS_NAME: &[u16] = &[
                b'P' as u16,
                b'u' as u16,
                b'l' as u16,
                b's' as u16,
                b'a' as u16,
                b'r' as u16,
                b'R' as u16,
                b'a' as u16,
                b'w' as u16,
                b'M' as u16,
                b'o' as u16,
                b'u' as u16,
                b's' as u16,
                b'e' as u16,
                0,
            ];

            unsafe extern "system" fn raw_mouse_window_proc(
                hwnd: HWND,
                message: UINT,
                wparam: WPARAM,
                lparam: LPARAM,
            ) -> LRESULT {
                if message == WM_INPUT {
                    read_raw_mouse_input(lparam as winapi::um::winuser::HRAWINPUT);
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }

            let instance = GetModuleHandleW(null());
            let class = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(raw_mouse_window_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: null_mut(),
                hCursor: null_mut(),
                hbrBackground: null_mut(),
                lpszMenuName: null(),
                lpszClassName: CLASS_NAME.as_ptr(),
            };
            let atom: ATOM = RegisterClassW(&class);
            if atom == 0 {
                let _ = ready_tx.send(false);
                return;
            }

            let hwnd = CreateWindowExW(
                0,
                CLASS_NAME.as_ptr(),
                CLASS_NAME.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                null_mut(),
                instance,
                null_mut(),
            );
            if hwnd.is_null() {
                let _ = ready_tx.send(false);
                return;
            }

            let device = RAWINPUTDEVICE {
                usUsagePage: 0x01,
                usUsage: 0x02,
                dwFlags: RIDEV_INPUTSINK,
                hwndTarget: hwnd,
            };
            if RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as UINT) == 0 {
                let _ = ready_tx.send(false);
                return;
            }

            if ready_tx.send(true).is_err() {
                return;
            }

            let mut message: MSG = zeroed();
            while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        });

        ready_rx.recv().unwrap_or(false)
    });
    // Preserve the confined-cursor fallback if Raw Input is unavailable.
    true
}

#[cfg(target_os = "windows")]
pub fn begin_relative_mouse_mode() {
    RAW_MOUSE_DELTA_X.store(0, Ordering::Release);
    RAW_MOUSE_DELTA_Y.store(0, Ordering::Release);
    RAW_MOUSE_CAPTURED.store(true, Ordering::Release);
    if uses_raw_mouse_input() {
        hide_cursor();
    }
}

#[cfg(target_os = "windows")]
pub fn end_relative_mouse_mode() {
    RAW_MOUSE_CAPTURED.store(false, Ordering::Release);
    RAW_MOUSE_DELTA_X.store(0, Ordering::Release);
    RAW_MOUSE_DELTA_Y.store(0, Ordering::Release);
    if uses_raw_mouse_input() {
        show_cursor();
    }
}

#[cfg(target_os = "windows")]
pub fn uses_raw_mouse_input() -> bool {
    RAW_MOUSE_READY.get().copied().unwrap_or(false)
}

#[cfg(target_os = "windows")]
pub fn take_mouse_delta() -> (f32, f32) {
    let x = RAW_MOUSE_DELTA_X.swap(0, Ordering::AcqRel) as f32;
    let y = RAW_MOUSE_DELTA_Y.swap(0, Ordering::AcqRel) as f32;
    (x, y)
}

#[cfg(target_os = "windows")]
unsafe fn read_raw_mouse_input(input: winapi::um::winuser::HRAWINPUT) {
    use std::mem::size_of;
    use winapi::shared::minwindef::UINT;
    use winapi::um::winuser::{
        GetRawInputData, MOUSE_MOVE_ABSOLUTE, RAWINPUT, RAWINPUTHEADER, RID_INPUT, RIM_TYPEMOUSE,
    };

    if !RAW_MOUSE_CAPTURED.load(Ordering::Acquire) {
        return;
    }

    let mut byte_count: UINT = 0;
    let header_size = size_of::<RAWINPUTHEADER>() as UINT;
    if GetRawInputData(
        input,
        RID_INPUT,
        std::ptr::null_mut(),
        &mut byte_count,
        header_size,
    ) == u32::MAX
        || byte_count < size_of::<RAWINPUT>() as UINT
    {
        return;
    }

    // Vec<u64> provides the alignment required by RAWINPUT while allowing the
    // OS-reported size to include any trailing device-specific data.
    let mut storage = vec![0u64; (byte_count as usize).div_ceil(size_of::<u64>())];
    let read = GetRawInputData(
        input,
        RID_INPUT,
        storage.as_mut_ptr().cast(),
        &mut byte_count,
        header_size,
    );
    if read == u32::MAX {
        return;
    }

    let raw = &*(storage.as_ptr().cast::<RAWINPUT>());
    let mouse = raw.data.mouse();
    if raw.header.dwType != RIM_TYPEMOUSE || mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
        return;
    }

    RAW_MOUSE_DELTA_X.fetch_add(mouse.lLastX as i64, Ordering::Relaxed);
    RAW_MOUSE_DELTA_Y.fetch_add(mouse.lLastY as i64, Ordering::Relaxed);
}

/// Lock cursor to window bounds (prevents cursor from leaving the window).
///
/// This is used to keep the cursor confined during camera rotation to prevent
/// accidental clicks outside the viewport.
#[cfg(target_os = "windows")]
pub fn lock_cursor_to_window(window: &Window) -> bool {
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

                    let locked = ClipCursor(&screen_rect) != 0;
                    tracing::debug!("[VIEWPORT] 🔒 Cursor locked to window bounds");
                    return locked;
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
    false
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
pub fn lock_cursor_to_point(screen_x: i32, screen_y: i32, radius: i32) -> bool {
    use winapi::shared::windef::RECT;
    use winapi::um::winuser::ClipCursor;

    // RECT's right and bottom edges are exclusive. A zero radius still needs
    // a 1x1 rectangle so ClipCursor anchors the pointer at this exact pixel.
    let radius = radius.max(0);
    unsafe {
        let screen_rect = RECT {
            left: screen_x - radius,
            top: screen_y - radius,
            right: screen_x + radius + 1,
            bottom: screen_y + radius + 1,
        };
        let locked = ClipCursor(&screen_rect) != 0;
        tracing::debug!(
            "[VIEWPORT] 🔒 Cursor confinement to {}px radius around ({}, {}): {}",
            radius,
            screen_x,
            screen_y,
            locked
        );
        locked
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
