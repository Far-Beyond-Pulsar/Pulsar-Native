//! Viewport input processing thread: lock-free input polling and camera/tool
//! dispatch.

use super::*;

/// Keys the viewport camera reads, with their Windows virtual-key codes.
#[cfg(target_os = "windows")]
const CAMERA_KEYS: [(i32, Keycode); 11] = [
    (0x57, Keycode::W),
    (0x41, Keycode::A),
    (0x53, Keycode::S),
    (0x44, Keycode::D),
    (0x45, Keycode::E),
    (0x51, Keycode::Q),
    (0x20, Keycode::Space),
    (0xA2, Keycode::LControl),
    (0xA3, Keycode::RControl),
    (0xA0, Keycode::LShift),
    (0xA1, Keycode::RShift),
];

/// Which of the camera's keys are held right now.
///
/// `DeviceState::get_keys()` on Windows queries all 256 virtual keys with a
/// `GetAsyncKeyState` syscall each (plus two `Vec` allocations), every poll. The
/// camera only needs eleven. The syscalls go into win32k, which the UI thread
/// shares, so the full sweep made this poll bimodal: ~0.1 ms usually but 2-6 ms
/// on a fifth of polls, on a thread that runs every ~2 ms while the camera is
/// captured -- each slow poll delays key state reaching the camera.
///
/// `device_state` is `None` on Linux without an X display (pure Wayland);
/// there, and on Wayland generally, keys come from the Wayland bypass.
fn held_camera_keys(device_state: Option<&DeviceState>) -> Vec<Keycode> {
    #[cfg(target_os = "windows")]
    {
        use winapi::um::winuser::GetAsyncKeyState;
        let _ = device_state;
        let mut held = Vec::with_capacity(4);
        for (vk, keycode) in CAMERA_KEYS {
            // High bit set == key is down.
            // SAFETY: `GetAsyncKeyState` takes a plain virtual-key code by value,
            // reads global input state and has no pointer or lifetime
            // requirements, so it is sound to call from any thread.
            if unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000 != 0 {
                held.push(keycode);
            }
        }
        held
    }
    #[cfg(not(target_os = "windows"))]
    {
        // A Wayland window's keys aren't visible to X queries; the Wayland
        // bypass tracks them from its own keyboard object.
        #[cfg(not(target_os = "macos"))]
        {
            if let Some(keys) = cursor::held_keys() {
                return keys;
            }
        }
        device_state
            .map(<DeviceState as device_query::DeviceQuery>::get_keys)
            .unwrap_or_default()
    }
}

/// The shared state the input thread reads and writes.
struct InputPoller {
    input_state: Arc<InputState>,
    camera_input: Option<Arc<Mutex<engine_backend::subsystems::render::CameraInput>>>,
    mouse_right_captured: Arc<AtomicBool>,
    mouse_middle_captured: Arc<AtomicBool>,
    right_mouse_dragged: Arc<AtomicBool>,
    // macOS reads relative pointer deltas and never needs the lock point.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    locked_cursor_screen_x: Arc<AtomicI32>,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    locked_cursor_screen_y: Arc<AtomicI32>,
}

/// The keyboard state source: `DeviceState::new` panics without an X display
/// (pure Wayland), so on Linux keys then come from the Wayland bypass instead.
fn open_device_state() -> Option<DeviceState> {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        Some(DeviceState::new())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        DeviceState::checked_new()
    }
}

impl InputPoller {
    /// The thread body: poll the keyboard and pointer while the camera is
    /// captured, until `stop_flag` is set.
    fn run(self, ctx: &editor_task_queue::TaskContext, stop_flag: &AtomicBool) {
        profiling::set_thread_name("Input Thread");
        // Lets the render thread wake this thread for a fresh sample right
        // before it reads the camera (see `input_latch`).
        let latch = super::input_latch::register_input_thread();
        tracing::debug!("[INPUT-THREAD] 🚀 Dedicated RAW INPUT processing thread started");
        let device_state = open_device_state();
        let mut was_capturing = false;

        loop {
            if stop_flag.load(Ordering::Acquire) {
                tracing::debug!("[INPUT-THREAD] shutdown requested, exiting");
                super::input_latch::unregister_input_thread(&latch);
                return;
            }

            ctx.heartbeat();
            let capture = ViewportCursorCapture::load(
                &self.mouse_right_captured,
                &self.mouse_middle_captured,
            );
            let is_rotating = capture.is_rotating();
            let is_panning = capture.is_panning();
            let capturing = is_rotating || is_panning;

            // While capturing, poll at ~500Hz so keypresses and cursor deltas
            // reach the camera within ~2ms instead of up to 8ms; idle, drop to
            // ~120Hz so we don't burn CPU.
            latch.set_capturing(capturing);
            // `park_timeout`, not `sleep`: the render thread can `unpark` this
            // thread to request a fresh sample right before a frame reads the
            // camera. Without a request it behaves exactly like the sleep.
            std::thread::park_timeout(std::time::Duration::from_millis(if capturing {
                2
            } else {
                8
            }));

            let capture_still_valid = self.capture_still_valid(capture, capturing);

            // Signals "one iteration finished" on every path out of this
            // iteration body, including the early `continue` below.
            let _poll_done = super::input_latch::PollDone(latch.clone());
            profiling::profile_scope!("input_poll");
            let input_start = std::time::Instant::now();

            if !capturing || !capture_still_valid {
                self.clear_input(&mut was_capturing);
                continue;
            }

            // Detect the transition into capture so we can discard the first
            // (stale) mouse delta and avoid a jump on the first pan/rotate frame.
            let just_activated = !was_capturing;
            was_capturing = true;

            self.poll_keyboard(is_rotating, device_state.as_ref());
            self.poll_mouse(is_rotating, is_panning, just_activated);

            let latency_us = input_start.elapsed().as_micros() as u64;
            self.input_state.set_input_latency_us(latency_us);
        }
    }

    /// Whether the capture still owns the pointer.
    ///
    /// The raw-input window is registered with INPUTSINK so it can receive
    /// deltas regardless of focus. Ownership is validated on every poll and
    /// capture is torn down if focus or the initiating button was lost, even
    /// if GPUI never delivered mouse-up.
    #[cfg(target_os = "windows")]
    fn capture_still_valid(&self, capture: ViewportCursorCapture, capturing: bool) -> bool {
        if !capturing {
            return true;
        }
        let anchor_x = self.locked_cursor_screen_x.load(Ordering::Acquire);
        let anchor_y = self.locked_cursor_screen_y.load(Ordering::Acquire);
        let button_vk = capture.initiating_button_vk();
        if anchor_x == i32::MIN
            || anchor_y == i32::MIN
            || !cursor::maintain_relative_mouse_mode(anchor_x, anchor_y, button_vk)
        {
            tracing::debug!("[VIEWPORT] Ending cursor capture after focus or button loss");
            ViewportCursorCapture::Released
                .store(&self.mouse_right_captured, &self.mouse_middle_captured);
            cursor::end_relative_mouse_mode();
            cursor::unlock_cursor();
            if anchor_x != i32::MIN && anchor_y != i32::MIN {
                cursor::set_cursor_position(anchor_x, anchor_y);
            }
            self.locked_cursor_screen_x
                .store(i32::MIN, Ordering::Release);
            self.locked_cursor_screen_y
                .store(i32::MIN, Ordering::Release);
            return false;
        }
        true
    }

    /// Capture ownership is only checked on Windows.
    #[cfg(not(target_os = "windows"))]
    fn capture_still_valid(&self, _capture: ViewportCursorCapture, _capturing: bool) -> bool {
        true
    }

    /// Not capturing: clear the movement state, and the camera's once when a
    /// capture just ended.
    fn clear_input(&self, was_capturing: &mut bool) {
        self.input_state.set_forward(0);
        self.input_state.set_right(0);
        self.input_state.set_up(0);
        self.input_state.set_boost(false);

        if *was_capturing {
            if let Some(cam) = &self.camera_input {
                if let Ok(mut input) = cam.lock() {
                    input.forward = 0.0;
                    input.right = 0.0;
                    input.up = 0.0;
                    input.boost = false;
                    input.clear_transient_deltas();
                }
            }
            *was_capturing = false;
        }
    }

    /// Read the fly keys and write them to the input state and the camera.
    fn poll_keyboard(&self, is_rotating: bool, device_state: Option<&DeviceState>) {
        profiling::profile_scope!("keyboard_poll");
        // Fly only during RMB look. Middle-mouse / Shift+RMB pan
        // must not also translate from the keyboard.
        let keys = if is_rotating {
            held_camera_keys(device_state)
        } else {
            Vec::new()
        };
        let forward = if keys.contains(&Keycode::W) {
            1
        } else if keys.contains(&Keycode::S) {
            -1
        } else {
            0
        };
        let right = if keys.contains(&Keycode::D) {
            1
        } else if keys.contains(&Keycode::A) {
            -1
        } else {
            0
        };
        let up = if keys.contains(&Keycode::E) || keys.contains(&Keycode::Space) {
            1
        } else if keys.contains(&Keycode::Q)
            || keys.contains(&Keycode::LControl)
            || keys.contains(&Keycode::RControl)
        {
            -1
        } else {
            0
        };
        // Shift is the speed boost modifier (no longer doubles as descend).
        let boost = keys.contains(&Keycode::LShift) || keys.contains(&Keycode::RShift);

        self.input_state.set_forward(forward);
        self.input_state.set_right(right);
        self.input_state.set_up(up);
        self.input_state.set_boost(boost);

        // Write WASD/boost directly into CameraInput. Mouse deltas already
        // bypass the UI thread; keyboard must too, otherwise key state only
        // reaches the camera when GPUI happens to repaint (`send_input_to_gpu`),
        // which lags by a full UI frame — the "mushy / gets behind" feel.
        if let Some(cam) = &self.camera_input {
            if let Ok(mut input) = cam.lock() {
                input.forward = forward as f32;
                input.right = right as f32;
                input.up = up as f32;
                input.boost = boost;
            }
        }
    }

    /// Mark a right-button capture as a drag, so its release does not open
    /// the context menu.
    fn note_right_drag(&self) {
        if self.mouse_right_captured.load(Ordering::Acquire) {
            self.right_mouse_dragged.store(true, Ordering::Release);
        }
    }

    /// Hand a pointer delta to the camera accumulator and the input state, as
    /// a look while rotating or a pan while panning.
    fn forward_pointer_delta(&self, dx: f32, dy: f32, is_rotating: bool, is_panning: bool) {
        if let Some(cam) = &self.camera_input {
            if let Ok(mut input) = cam.lock() {
                if is_rotating {
                    input.accumulate_look_delta(dx, dy);
                } else if is_panning {
                    input.accumulate_pan_delta(dx, dy);
                }
            }
        }
        if is_rotating {
            self.input_state.set_mouse_delta(dx, dy);
        } else if is_panning {
            self.input_state.set_pan_delta(dx, dy);
        }
    }

    /// Poll the pointer on Windows: Raw Input when registered, otherwise the
    /// confined cursor's offset from the lock point.
    #[cfg(target_os = "windows")]
    fn poll_mouse(&self, is_rotating: bool, is_panning: bool, just_activated: bool) {
        profiling::profile_scope!("mouse_poll");
        if cursor::uses_raw_mouse_input() {
            // Raw Input delivers device-relative reports without cursor
            // acceleration or recentering. Deltas are accumulated until this
            // poll, then forwarded to the same render-thread camera
            // accumulator as other OSes.
            let (dx, dy) = cursor::take_mouse_delta();
            if dx != 0.0 || dy != 0.0 {
                self.note_right_drag();
                self.forward_pointer_delta(dx, dy, is_rotating, is_panning);
            }
        } else {
            // Fallback for systems where Raw Input registration fails: derive
            // deltas from the confined cursor.
            self.poll_confined_cursor(is_rotating, is_panning, just_activated);
        }
    }

    /// Windows fallback: the cursor's offset from the lock point, then warp
    /// it back.
    #[cfg(target_os = "windows")]
    fn poll_confined_cursor(&self, is_rotating: bool, is_panning: bool, just_activated: bool) {
        use winapi::shared::windef::POINT;
        use winapi::um::winuser::GetCursorPos;

        let locked_screen_x = self.locked_cursor_screen_x.load(Ordering::Relaxed);
        let locked_screen_y = self.locked_cursor_screen_y.load(Ordering::Relaxed);
        if locked_screen_x == i32::MIN || locked_screen_y == i32::MIN {
            return;
        }

        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: `GetCursorPos` writes one `POINT` through the pointer we
        // pass; `point` is a live, initialized local that outlives the call.
        // The call has no thread affinity.
        unsafe {
            GetCursorPos(&mut point);
        }
        let dx = point.x - locked_screen_x;
        let dy = point.y - locked_screen_y;

        if dx != 0 || dy != 0 {
            self.note_right_drag();
            if !just_activated {
                self.forward_pointer_delta(dx as f32, dy as f32, is_rotating, is_panning);
            }
            cursor::set_cursor_position(locked_screen_x, locked_screen_y);
        }
    }

    /// Poll the pointer on macOS: always drain the accumulated delta. On the
    /// first frame of a capture it is discarded (`just_activated`) so movement
    /// that piled up before the drag doesn't snap the camera.
    #[cfg(target_os = "macos")]
    fn poll_mouse(&self, is_rotating: bool, is_panning: bool, just_activated: bool) {
        profiling::profile_scope!("mouse_poll");
        self.poll_relative_deltas(is_rotating, is_panning, just_activated);
    }

    /// Poll the pointer on Linux: relative deltas from the Wayland bypass, or
    /// the X11 pointer's offset from the lock point.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn poll_mouse(&self, is_rotating: bool, is_panning: bool, just_activated: bool) {
        profiling::profile_scope!("mouse_poll");
        // Wayland: relative deltas from the Wayland bypass (no global pointer
        // position or warping), like macOS.
        if cursor::uses_relative_deltas() {
            self.poll_relative_deltas(is_rotating, is_panning, just_activated);
        }

        // X11: measure the pointer's offset from the lock point, then warp it
        // back (one shared X connection).
        if !cursor::uses_relative_deltas() {
            self.poll_warped_pointer(is_rotating, is_panning, just_activated);
        }
    }

    /// Forward the pointer's accumulated relative delta, except on the first
    /// frame of a capture.
    #[cfg(not(target_os = "windows"))]
    fn poll_relative_deltas(&self, is_rotating: bool, is_panning: bool, just_activated: bool) {
        let (dx, dy) = cursor::take_mouse_delta();

        if !just_activated && (dx != 0.0 || dy != 0.0) {
            self.note_right_drag();
            self.forward_pointer_delta(dx, dy, is_rotating, is_panning);
        }
    }

    /// X11: the pointer's offset from the lock point, then warp it back.
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    fn poll_warped_pointer(&self, is_rotating: bool, is_panning: bool, just_activated: bool) {
        let locked_screen_x = self.locked_cursor_screen_x.load(Ordering::Relaxed);
        let locked_screen_y = self.locked_cursor_screen_y.load(Ordering::Relaxed);

        if locked_screen_x != i32::MIN && locked_screen_y != i32::MIN {
            if let Some((cx, cy)) = cursor::get_cursor_position() {
                let dx = cx - locked_screen_x;
                let dy = cy - locked_screen_y;

                if dx != 0 || dy != 0 {
                    if !just_activated {
                        self.note_right_drag();
                        self.forward_pointer_delta(dx as f32, dy as f32, is_rotating, is_panning);
                    }

                    cursor::set_cursor_position(locked_screen_x, locked_screen_y);
                }
            }
        }
    }
}

impl ViewportPanel {
    /// Spawn the input processing thread (only once).
    pub(super) fn spawn_input_thread_once(
        &mut self,
        gpu_engine: &Arc<Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    ) {
        if self.input_thread_spawned.load(Ordering::Relaxed) {
            return;
        }

        self.input_thread_spawned.store(true, Ordering::Relaxed);

        let camera_input = gpu_engine
            .lock()
            .ok()
            .and_then(|engine| engine.camera_input());
        let poller = InputPoller {
            input_state: self.input_state.clone(),
            camera_input,
            mouse_right_captured: self.mouse_right_captured.clone(),
            mouse_middle_captured: self.mouse_middle_captured.clone(),
            right_mouse_dragged: self.right_mouse_dragged.clone(),
            locked_cursor_screen_x: self.locked_cursor_screen_x.clone(),
            locked_cursor_screen_y: self.locked_cursor_screen_y.clone(),
        };
        let stop_flag = self.input_thread_stop.clone();

        self.input_thread_handle = Some(
            editor_task_queue::spawn_service(
                editor_task_queue::ServiceDescription::new(
                    "Viewport input loop",
                    "input",
                    "Input Thread",
                    editor_task_queue::ThreadRole::Io,
                )
                .owner("level editor viewport")
                .heartbeat(std::time::Duration::from_secs(3)),
                move |ctx| poller.run(ctx, &stop_flag),
            )
            .expect("failed to start the viewport input thread"),
        );
    }
}
