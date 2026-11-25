//! Input sampled where the OS delivers it, consumed where the simulation reads it.
//!
//! The event loop that owns the window also draws it, so an input event that
//! arrives while a frame is being recorded waits for that frame (several
//! milliseconds) before the loop sees it, and the simulation tick that would
//! consume it has already been scheduled by then. Two things follow here:
//!
//! 1. **A raw-input thread** (Windows: a message-only window registered for raw
//!    mouse and keyboard input, `RIDEV_INPUTSINK`) takes each event the moment
//!    the OS delivers it. It adds mouse motion to an accumulator and stamps the
//!    arrival on the latency probe's clock ([`crate::latency`]), so the chain's
//!    first stamp is the OS and the event loop's own wait is measured
//!    separately (`Latency::os_to_handler`).
//! 2. **A sample point.** The simulation tick drains the accumulator
//!    ([`PointerSampler::drain`]) at the start of the tick, where input is
//!    consumed, and publishes the sum as one `MouseMoved`: the delta is as
//!    fresh as the tick, not as old as the last frame, and a thousand events a
//!    second cost one publish per tick.
//!
//! No other platform has a raw-input thread yet: there the event loop's
//! device events feed the same accumulator ([`PointerSampler::add_motion`]) and
//! only the stamp is late.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;

/// Motion is kept in 1/1024 of a device count: a raw count is whole, a window
/// system's delta may not be.
const SUBCOUNT: f64 = 1024.0;

/// What the tick drained: the motion since the last sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerSample {
    pub dx: f64,
    pub dy: f64,
    /// The cursor position last reported by the window (what `MouseMoved`
    /// carries while the cursor is captured).
    pub x: f64,
    pub y: f64,
}

/// The accumulator between the raw-input thread and the tick. Cheap to share:
/// plain atomics, no lock on either side.
pub struct PointerSampler {
    dx: AtomicI64,
    dy: AtomicI64,
    /// The same motion for the event loop's own consumers (the free camera),
    /// drained by [`Self::take_view_motion`].
    view_dx: AtomicI64,
    view_dy: AtomicI64,
    /// Whole counts of motion added since the last drain; 0 means nothing.
    pending: AtomicU64,
    /// The cursor is captured by the game window: motion counts.
    captured: AtomicBool,
    /// The game window has focus: keys count, and so does captured motion.
    focused: AtomicBool,
    /// A raw-input thread feeds the sampler (the event loop's device events
    /// then only measure their own wait).
    raw_feed: AtomicBool,
    /// Oldest motion / key the raw thread saw that the event loop has not
    /// handed over yet (the probe's clock; 0 = none).
    unhandled_motion: AtomicU64,
    unhandled_key: AtomicU64,
    cursor_x: AtomicU64,
    cursor_y: AtomicU64,
}

impl PointerSampler {
    /// A sampler with no feed. [`start_raw_input`](Self::start_raw_input)
    /// attaches the OS thread.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            dx: AtomicI64::new(0),
            dy: AtomicI64::new(0),
            view_dx: AtomicI64::new(0),
            view_dy: AtomicI64::new(0),
            pending: AtomicU64::new(0),
            captured: AtomicBool::new(false),
            focused: AtomicBool::new(true),
            raw_feed: AtomicBool::new(false),
            unhandled_motion: AtomicU64::new(0),
            unhandled_key: AtomicU64::new(0),
            cursor_x: AtomicU64::new(0f64.to_bits()),
            cursor_y: AtomicU64::new(0f64.to_bits()),
        })
    }

    /// Whether a raw-input thread is feeding the sampler.
    pub fn has_raw_feed(&self) -> bool {
        self.raw_feed.load(Ordering::Acquire)
    }

    /// The game window captured or released the cursor.
    pub fn set_captured(&self, captured: bool) {
        self.captured.store(captured, Ordering::Release);
        if !captured {
            // Motion from before the release must not turn the view later.
            self.dx.store(0, Ordering::Release);
            self.dy.store(0, Ordering::Release);
            self.view_dx.store(0, Ordering::Release);
            self.view_dy.store(0, Ordering::Release);
            self.pending.store(0, Ordering::Release);
        }
    }

    /// The game window gained or lost focus.
    pub fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Release);
    }

    /// The window's cursor position (the position `MouseMoved` carries).
    pub fn set_cursor(&self, x: f64, y: f64) {
        self.cursor_x.store(x.to_bits(), Ordering::Relaxed);
        self.cursor_y.store(y.to_bits(), Ordering::Relaxed);
    }

    /// Relative motion, in device counts. From the raw-input thread (or the
    /// event loop where there is none); counted only while the cursor is
    /// captured and the window focused. `at` is the arrival stamp
    /// ([`crate::latency::stamp`]).
    pub fn add_motion(&self, dx: f64, dy: f64, at: u64) {
        if !self.captured.load(Ordering::Acquire) || !self.focused.load(Ordering::Acquire) {
            return;
        }
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        self.dx.fetch_add((dx * SUBCOUNT).round() as i64, Ordering::AcqRel);
        self.dy.fetch_add((dy * SUBCOUNT).round() as i64, Ordering::AcqRel);
        self.pending.fetch_add(1, Ordering::AcqRel);
        self.view_dx.fetch_add((dx * SUBCOUNT).round() as i64, Ordering::AcqRel);
        self.view_dy.fetch_add((dy * SUBCOUNT).round() as i64, Ordering::AcqRel);
        crate::latency::note_input_at(at);
        let _ = self
            .unhandled_motion
            .compare_exchange(0, at, Ordering::AcqRel, Ordering::Relaxed);
    }

    /// A key went down or up (from the raw-input thread): only the arrival is
    /// kept, for the chain's first stamp and the event loop's wait.
    pub fn note_key(&self, at: u64) {
        if !self.focused.load(Ordering::Acquire) {
            return;
        }
        crate::latency::note_input_at(at);
        let _ = self
            .unhandled_key
            .compare_exchange(0, at, Ordering::AcqRel, Ordering::Relaxed);
    }

    /// The motion since the last call, for the event loop's own consumers (the
    /// free camera), which no longer receive the OS's raw mouse events: this
    /// process's raw-input registration belongs to the sampler's thread.
    pub fn take_view_motion(&self) -> Option<(f64, f64)> {
        let dx = self.view_dx.swap(0, Ordering::AcqRel);
        let dy = self.view_dy.swap(0, Ordering::AcqRel);
        (dx != 0 || dy != 0).then(|| (dx as f64 / SUBCOUNT, dy as f64 / SUBCOUNT))
    }

    /// The event loop got to a mouse motion: report how long the raw
    /// thread's copy of it had waited.
    pub fn handled_motion(&self) {
        let at = self.unhandled_motion.swap(0, Ordering::AcqRel);
        if at != 0 {
            crate::latency::note_handler_delay(at);
        }
    }

    /// The event loop handed over a key event: report its wait.
    pub fn handled_key(&self) {
        let at = self.unhandled_key.swap(0, Ordering::AcqRel);
        if at != 0 {
            crate::latency::note_handler_delay(at);
        }
    }

    /// The motion since the last call, or `None` when there was none. The
    /// sample point: the tick calls it where it reads input.
    pub fn drain(&self) -> Option<PointerSample> {
        if self.pending.load(Ordering::Acquire) == 0 {
            return None;
        }
        self.pending.store(0, Ordering::Release);
        let dx = self.dx.swap(0, Ordering::AcqRel);
        let dy = self.dy.swap(0, Ordering::AcqRel);
        if dx == 0 && dy == 0 {
            return None;
        }
        Some(PointerSample {
            dx: dx as f64 / SUBCOUNT,
            dy: dy as f64 / SUBCOUNT,
            x: f64::from_bits(self.cursor_x.load(Ordering::Relaxed)),
            y: f64::from_bits(self.cursor_y.load(Ordering::Relaxed)),
        })
    }

    /// Start the raw-input thread (Windows). Returns whether one is feeding the
    /// sampler; where it could not start (or on another platform) the event
    /// loop's device events feed it instead.
    pub fn start_raw_input(self: &Arc<Self>) -> bool {
        #[cfg(windows)]
        {
            if self.raw_feed.load(Ordering::Acquire) {
                return true;
            }
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let sampler = Arc::clone(self);
            let spawned = editor_task_queue::spawn_thread(
                editor_task_queue::ThreadSpec::new(
                    "pulsar-raw-input",
                    editor_task_queue::ThreadRole::Io,
                )
                .owner("pulsar_game"),
                move || raw::run(sampler, ready_tx),
            );
            if let Err(error) = spawned {
                tracing::warn!("raw input thread not started: {error}");
                return false;
            }
            let ok = ready_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap_or(false);
            self.raw_feed.store(ok, Ordering::Release);
            ok
        }
        #[cfg(not(windows))]
        {
            false
        }
    }
}

#[cfg(windows)]
mod raw {
    //! The Win32 side: a message-only window that receives `WM_INPUT`. Direct
    //! `user32` declarations, as `engine_fs` does for `kernel32`, so no
    //! `windows` feature set changes (and with it every crate's feature hash).

    use super::PointerSampler;
    use std::sync::Arc;

    type Hwnd = isize;

    const WM_INPUT: u32 = 0x00FF;
    const HWND_MESSAGE: Hwnd = -3;
    const RIDEV_INPUTSINK: u32 = 0x0000_0100;
    const RID_INPUT: u32 = 0x1000_0003;
    const RIM_TYPEMOUSE: u32 = 0;
    const RIM_TYPEKEYBOARD: u32 = 1;
    const MOUSE_MOVE_ABSOLUTE: u16 = 0x0001;
    const RI_KEY_BREAK: u16 = 0x0001;

    #[repr(C)]
    struct WndClassW {
        style: u32,
        wnd_proc: unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize,
        cls_extra: i32,
        wnd_extra: i32,
        instance: isize,
        icon: isize,
        cursor: isize,
        background: isize,
        menu_name: *const u16,
        class_name: *const u16,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt_x: i32,
        pt_y: i32,
        private: u32,
    }

    #[repr(C)]
    struct RawInputDevice {
        usage_page: u16,
        usage: u16,
        flags: u32,
        target: Hwnd,
    }

    #[link(name = "user32")]
    extern "system" {
        fn RegisterClassW(class: *const WndClassW) -> u16;
        fn CreateWindowExW(
            ex_style: u32,
            class_name: *const u16,
            window_name: *const u16,
            style: u32,
            x: i32,
            y: i32,
            width: i32,
            height: i32,
            parent: Hwnd,
            menu: isize,
            instance: isize,
            param: *const core::ffi::c_void,
        ) -> Hwnd;
        fn DestroyWindow(hwnd: Hwnd) -> i32;
        fn DefWindowProcW(hwnd: Hwnd, message: u32, wparam: usize, lparam: isize) -> isize;
        fn GetMessageW(msg: *mut Msg, hwnd: Hwnd, min: u32, max: u32) -> i32;
        fn DispatchMessageW(msg: *const Msg) -> isize;
        fn RegisterRawInputDevices(devices: *const RawInputDevice, count: u32, size: u32) -> i32;
        fn GetRawInputData(
            raw: isize,
            command: u32,
            data: *mut core::ffi::c_void,
            size: *mut u32,
            header_size: u32,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> isize;
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The thread body: create the window, register for raw input, report
    /// whether that worked, then pump messages until the process ends.
    pub(super) fn run(sampler: Arc<PointerSampler>, ready: std::sync::mpsc::Sender<bool>) {
        // SAFETY: plain Win32 calls with valid, NUL-terminated names and
        // structures that live across the calls; the window belongs to this
        // thread, which is also the only one that pumps its messages.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class_name = wide("PulsarRawInput");
            let class = WndClassW {
                style: 0,
                wnd_proc: DefWindowProcW,
                cls_extra: 0,
                wnd_extra: 0,
                instance,
                icon: 0,
                cursor: 0,
                background: 0,
                menu_name: std::ptr::null(),
                class_name: class_name.as_ptr(),
            };
            // A second window of the process finds the class registered.
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                class_name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                0,
                instance,
                std::ptr::null(),
            );
            if hwnd == 0 {
                tracing::warn!("raw input window could not be created");
                let _ = ready.send(false);
                return;
            }
            let devices = [
                // Generic desktop: mouse, keyboard. Sink: delivered while the
                // game is not the foreground window too; the sampler's focus
                // flag decides what counts.
                RawInputDevice {
                    usage_page: 0x01,
                    usage: 0x02,
                    flags: RIDEV_INPUTSINK,
                    target: hwnd,
                },
                RawInputDevice {
                    usage_page: 0x01,
                    usage: 0x06,
                    flags: RIDEV_INPUTSINK,
                    target: hwnd,
                },
            ];
            if RegisterRawInputDevices(
                devices.as_ptr(),
                devices.len() as u32,
                std::mem::size_of::<RawInputDevice>() as u32,
            ) == 0
            {
                tracing::warn!("raw input devices could not be registered");
                DestroyWindow(hwnd);
                let _ = ready.send(false);
                return;
            }
            let _ = ready.send(true);

            let mut msg: Msg = std::mem::zeroed();
            // 0 is WM_QUIT, -1 an error: either ends the thread.
            while GetMessageW(&mut msg, 0, 0, 0) > 0 {
                if msg.message == WM_INPUT {
                    let at = crate::latency::stamp();
                    read(&sampler, msg.lparam, at);
                }
                // `DefWindowProc` also releases the raw input's buffer.
                DispatchMessageW(&msg);
            }
            DestroyWindow(hwnd);
        }
    }

    /// Decode one `RAWINPUT` (24-byte header, then the mouse or keyboard
    /// record): relative mouse motion and key edges.
    unsafe fn read(sampler: &PointerSampler, handle: isize, at: u64) {
        const HEADER: u32 = 24;
        // Header plus the largest record this reads (mouse, 24 bytes).
        let mut buffer = [0u64; 8];
        let mut size = std::mem::size_of_val(&buffer) as u32;
        let copied = GetRawInputData(
            handle,
            RID_INPUT,
            buffer.as_mut_ptr().cast(),
            &mut size,
            HEADER,
        );
        if copied == u32::MAX || copied < HEADER {
            return;
        }
        let bytes: &[u8] = std::slice::from_raw_parts(buffer.as_ptr().cast(), size as usize);
        let u16_at = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
        let i32_at = |offset: usize| {
            i32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        let kind = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        match kind {
            RIM_TYPEMOUSE if bytes.len() >= 44 => {
                // RAWMOUSE: flags u16 at 24; lLastX and lLastY at 36 and 40.
                if u16_at(24) & MOUSE_MOVE_ABSOLUTE == 0 {
                    sampler.add_motion(f64::from(i32_at(36)), f64::from(i32_at(40)), at);
                }
            }
            RIM_TYPEKEYBOARD if bytes.len() >= 40 => {
                // RAWKEYBOARD: flags u16 at 26. An edge, not a repeat's
                // stream: only a make or a break starts a chain.
                let _is_break = u16_at(26) & RI_KEY_BREAK != 0;
                sampler.note_key(at);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_counts_only_while_captured_and_focused_and_drains_as_one_sample() {
        let sampler = PointerSampler::new();
        sampler.add_motion(5.0, 5.0, crate::latency::stamp());
        assert!(sampler.drain().is_none(), "not captured: ignored");

        sampler.set_captured(true);
        sampler.set_cursor(10.0, 20.0);
        sampler.add_motion(3.0, -2.0, crate::latency::stamp());
        sampler.add_motion(4.0, 1.0, crate::latency::stamp());
        let sample = sampler.drain().expect("two events, one sample");
        assert_eq!((sample.dx, sample.dy), (7.0, -1.0));
        assert_eq!((sample.x, sample.y), (10.0, 20.0));
        assert!(sampler.drain().is_none(), "drained once");

        sampler.set_focused(false);
        sampler.add_motion(9.0, 9.0, crate::latency::stamp());
        assert!(sampler.drain().is_none(), "unfocused: ignored");
    }

    #[test]
    fn the_view_takes_the_same_motion_the_tick_does_each_on_its_own_schedule() {
        let sampler = PointerSampler::new();
        sampler.set_captured(true);
        sampler.add_motion(3.0, 1.0, crate::latency::stamp());
        assert_eq!(sampler.take_view_motion(), Some((3.0, 1.0)));
        sampler.add_motion(2.0, 0.0, crate::latency::stamp());
        // The tick still has everything since its own last sample.
        let sample = sampler.drain().expect("motion");
        assert_eq!((sample.dx, sample.dy), (5.0, 1.0));
        assert_eq!(sampler.take_view_motion(), Some((2.0, 0.0)));
        assert_eq!(sampler.take_view_motion(), None);
    }

    #[test]
    fn releasing_the_cursor_drops_the_motion_waiting() {
        let sampler = PointerSampler::new();
        sampler.set_captured(true);
        sampler.add_motion(8.0, 8.0, crate::latency::stamp());
        sampler.set_captured(false);
        sampler.set_captured(true);
        assert!(sampler.drain().is_none());
    }

    #[test]
    fn opposite_motion_that_cancels_is_not_a_sample() {
        let sampler = PointerSampler::new();
        sampler.set_captured(true);
        sampler.add_motion(6.0, 0.0, crate::latency::stamp());
        sampler.add_motion(-6.0, 0.0, crate::latency::stamp());
        assert!(sampler.drain().is_none());
    }
}
