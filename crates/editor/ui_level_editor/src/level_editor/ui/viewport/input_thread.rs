//! Viewport input processing thread: lock-free input polling and camera/tool
//! dispatch.

use super::*;

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

        let input_state = self.input_state.clone();
        let camera_input = gpu_engine
            .lock()
            .ok()
            .and_then(|engine| engine.camera_input());
        let mouse_right_captured = self.mouse_right_captured.clone();
        let mouse_middle_captured = self.mouse_middle_captured.clone();
        let locked_cursor_screen_x = self.locked_cursor_screen_x.clone();
        let locked_cursor_screen_y = self.locked_cursor_screen_y.clone();
        let stop_flag = self.input_thread_stop.clone();

        self.input_thread_handle = Some(std::thread::spawn(move || {
            profiling::set_thread_name("Input Thread");
            tracing::debug!("[INPUT-THREAD] 🚀 Dedicated RAW INPUT processing thread started");
            let device_state = DeviceState::new();
            let mut _last_mouse_pos: Option<(i32, i32)> = None;
            let mut was_capturing = false;

            loop {
                if stop_flag.load(Ordering::Acquire) {
                    tracing::debug!("[INPUT-THREAD] shutdown requested, exiting");
                    return;
                }

                let capture =
                    ViewportCursorCapture::load(&mouse_right_captured, &mouse_middle_captured);
                let is_rotating = capture == ViewportCursorCapture::Rotate;
                let is_panning = capture == ViewportCursorCapture::Pan;
                let capturing = is_rotating || is_panning;

                // While capturing, poll at ~500Hz so keypresses and cursor deltas
                // reach the camera within ~2ms instead of up to 8ms; idle, drop to
                // ~120Hz so we don't burn CPU.
                std::thread::sleep(std::time::Duration::from_millis(if capturing {
                    2
                } else {
                    8
                }));
                profiling::profile_scope!("input_poll");
                let input_start = std::time::Instant::now();

                if !capturing {
                    // Not active - clear state
                    _last_mouse_pos = None;
                    input_state.set_forward(0);
                    input_state.set_right(0);
                    input_state.set_up(0);
                    input_state.set_boost(false);

                    if was_capturing {
                        if let Some(cam) = &camera_input {
                            if let Ok(mut input) = cam.lock() {
                                input.forward = 0.0;
                                input.right = 0.0;
                                input.up = 0.0;
                                input.boost = false;
                                input.clear_transient_deltas();
                            }
                        }
                        was_capturing = false;
                    }

                    continue;
                }

                // Detect the transition into capture so we can discard the first
                // (stale) mouse delta and avoid a jump on the first pan/rotate frame.
                let just_activated = !was_capturing;
                was_capturing = true;

                // Poll keyboard
                {
                    profiling::profile_scope!("keyboard_poll");
                    let keys: Vec<Keycode> = device_state.get_keys();
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

                    input_state.set_forward(forward);
                    input_state.set_right(right);
                    input_state.set_up(up);
                    input_state.set_boost(boost);

                    // Write WASD/boost directly into CameraInput. Mouse deltas already
                    // bypass the UI thread; keyboard must too, otherwise key state only
                    // reaches the camera when GPUI happens to repaint (`send_input_to_gpu`),
                    // which lags by a full UI frame — the "mushy / gets behind" feel.
                    if let Some(cam) = &camera_input {
                        if let Ok(mut input) = cam.lock() {
                            input.forward = forward as f32;
                            input.right = right as f32;
                            input.up = up as f32;
                            input.boost = boost;
                        }
                    }
                }

                // Poll mouse and calculate delta
                {
                    profiling::profile_scope!("mouse_poll");
                    #[cfg(target_os = "windows")]
                    {
                        let locked_screen_x = locked_cursor_screen_x.load(Ordering::Relaxed);
                        let locked_screen_y = locked_cursor_screen_y.load(Ordering::Relaxed);

                        if locked_screen_x > 0 && locked_screen_y > 0 {
                            use winapi::shared::windef::POINT;
                            use winapi::um::winuser::GetCursorPos;

                            unsafe {
                                let mut point = POINT { x: 0, y: 0 };
                                GetCursorPos(&mut point);

                                // Calculate delta from locked position (not last position)
                                let dx = point.x - locked_screen_x;
                                let dy = point.y - locked_screen_y;

                                if dx != 0 || dy != 0 {
                                    // On the first captured frame, discard the delta
                                    // (`just_activated`) to avoid a stale jump, but still
                                    // reset the cursor so the next delta is relative.
                                    if !just_activated {
                                        // Accumulate deltas so multiple input samples between render frames
                                        // are preserved instead of being overwritten.
                                        if let Some(cam) = &camera_input {
                                            if let Ok(mut input) = cam.lock() {
                                                if is_rotating {
                                                    input.accumulate_look_delta(
                                                        dx as f32, dy as f32,
                                                    );
                                                } else if is_panning {
                                                    input
                                                        .accumulate_pan_delta(dx as f32, dy as f32);
                                                }
                                            }
                                        }

                                        // Also update atomics for UI feedback (optional)
                                        if is_rotating {
                                            input_state.set_mouse_delta(dx as f32, dy as f32);
                                        } else if is_panning {
                                            input_state.set_pan_delta(dx as f32, dy as f32);
                                        }
                                    }

                                    // Reset cursor to locked position
                                    cursor::set_cursor_position(locked_screen_x, locked_screen_y);
                                }
                            }
                        }
                    }

                    #[cfg(target_os = "macos")]
                    {
                        // Always drain the accumulated delta. On the first frame of a
                        // capture we discard it (`just_activated`) so movement that piled
                        // up before the drag doesn't snap the camera on the first frame.
                        let (dx, dy) = cursor::take_mouse_delta();

                        if !just_activated && (dx != 0.0 || dy != 0.0) {
                            if let Some(cam) = &camera_input {
                                if let Ok(mut input) = cam.lock() {
                                    if is_rotating {
                                        input.accumulate_look_delta(dx, dy);
                                    } else if is_panning {
                                        input.accumulate_pan_delta(dx, dy);
                                    }
                                }
                            }

                            if is_rotating {
                                input_state.set_mouse_delta(dx, dy);
                            } else if is_panning {
                                input_state.set_pan_delta(dx, dy);
                            }
                        }
                    }

                    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
                    {
                        let locked_screen_x = locked_cursor_screen_x.load(Ordering::Relaxed);
                        let locked_screen_y = locked_cursor_screen_y.load(Ordering::Relaxed);

                        if locked_screen_x > 0 && locked_screen_y > 0 {
                            if let Some((cx, cy)) = cursor::get_cursor_position() {
                                let dx = cx - locked_screen_x;
                                let dy = cy - locked_screen_y;

                                if dx != 0 || dy != 0 {
                                    if !just_activated {
                                        if let Some(cam) = &camera_input {
                                            if let Ok(mut input) = cam.lock() {
                                                if is_rotating {
                                                    input.accumulate_look_delta(
                                                        dx as f32, dy as f32,
                                                    );
                                                } else if is_panning {
                                                    input
                                                        .accumulate_pan_delta(dx as f32, dy as f32);
                                                }
                                            }
                                        }

                                        if is_rotating {
                                            input_state.set_mouse_delta(dx as f32, dy as f32);
                                        } else if is_panning {
                                            input_state.set_pan_delta(dx as f32, dy as f32);
                                        }
                                    }

                                    cursor::set_cursor_position(locked_screen_x, locked_screen_y);
                                }
                            }
                        }
                    }
                }

                // Track latency
                let latency_us = input_start.elapsed().as_micros() as u64;
                input_state.set_input_latency_us(latency_us);
            }
        }));
    }
}