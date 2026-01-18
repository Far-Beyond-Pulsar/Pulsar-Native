//! Lock-free input state management for viewport controls.
//!
//! This module provides atomic-based input state tracking with zero mutex contention,
//! enabling high-performance camera controls with latency tracking.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use super::components::camera_selector::CameraSpeedControl;

const MIN_MOVE_SPEED: f32 = 1.0;
const MAX_MOVE_SPEED: f32 = 100.0;

/// Lock-free input state using atomics - no mutex contention!
pub struct InputState {
    // Keyboard movement (atomic for lock-free access)
    pub forward: Arc<AtomicI32>,  // -1, 0, 1
    pub right: Arc<AtomicI32>,    // -1, 0, 1
    pub up: Arc<AtomicI32>,       // -1, 0, 1
    pub boost: Arc<AtomicBool>,

    // Mouse position (stored as i32 * 1000 for fractional precision)
    pub mouse_x: Arc<AtomicI32>,
    pub mouse_y: Arc<AtomicI32>,

    // Mouse deltas (stored as i32 * 1000 for fractional precision)
    pub mouse_delta_x: Arc<AtomicI32>,
    pub mouse_delta_y: Arc<AtomicI32>,
    pub pan_delta_x: Arc<AtomicI32>,
    pub pan_delta_y: Arc<AtomicI32>,
    pub zoom_delta: Arc<AtomicI32>,

    // Input latency tracking (measured on input thread)
    // Stores microseconds since last input was received, as i64
    pub input_latency_us: Arc<AtomicU64>,
    
    // Camera move speed (stored as u32 bits for atomic access)
    pub move_speed: Arc<AtomicU32>,
}

impl InputState {
    /// Create a new input state with default values.
    pub fn new() -> Self {
        Self {
            forward: Arc::new(AtomicI32::new(0)),
            right: Arc::new(AtomicI32::new(0)),
            up: Arc::new(AtomicI32::new(0)),
            boost: Arc::new(AtomicBool::new(false)),
            mouse_x: Arc::new(AtomicI32::new(0)),
            mouse_y: Arc::new(AtomicI32::new(0)),
            mouse_delta_x: Arc::new(AtomicI32::new(0)),
            mouse_delta_y: Arc::new(AtomicI32::new(0)),
            pan_delta_x: Arc::new(AtomicI32::new(0)),
            pan_delta_y: Arc::new(AtomicI32::new(0)),
            zoom_delta: Arc::new(AtomicI32::new(0)),
            input_latency_us: Arc::new(AtomicU64::new(0)),
            move_speed: Arc::new(AtomicU32::new(10.0_f32.to_bits())),
        }
    }

    /// Set mouse delta (converts f32 to i32 * 1000 for atomic storage).
    pub fn set_mouse_delta(&self, x: f32, y: f32) {
        self.mouse_delta_x
            .store((x * 1000.0) as i32, Ordering::Relaxed);
        self.mouse_delta_y
            .store((y * 1000.0) as i32, Ordering::Relaxed);
    }

    /// Set pan delta (converts f32 to i32 * 1000 for atomic storage).
    pub fn set_pan_delta(&self, x: f32, y: f32) {
        self.pan_delta_x
            .store((x * 1000.0) as i32, Ordering::Relaxed);
        self.pan_delta_y
            .store((y * 1000.0) as i32, Ordering::Relaxed);
    }

    /// Set zoom delta (converts f32 to i32 * 1000 for atomic storage).
    pub fn set_zoom_delta(&self, z: f32) {
        self.zoom_delta
            .store((z * 1000.0) as i32, Ordering::Relaxed);
    }

    /// Get forward movement state.
    pub fn get_forward(&self) -> i32 {
        self.forward.load(Ordering::Relaxed)
    }

    /// Get right movement state.
    pub fn get_right(&self) -> i32 {
        self.right.load(Ordering::Relaxed)
    }

    /// Get up movement state.
    pub fn get_up(&self) -> i32 {
        self.up.load(Ordering::Relaxed)
    }

    /// Get boost state.
    pub fn get_boost(&self) -> bool {
        self.boost.load(Ordering::Relaxed)
    }

    /// Get mouse delta and reset it.
    pub fn take_mouse_delta(&self) -> (f32, f32) {
        let x = self.mouse_delta_x.swap(0, Ordering::Relaxed) as f32 / 1000.0;
        let y = self.mouse_delta_y.swap(0, Ordering::Relaxed) as f32 / 1000.0;
        (x, y)
    }

    /// Get pan delta and reset it.
    pub fn take_pan_delta(&self) -> (f32, f32) {
        let x = self.pan_delta_x.swap(0, Ordering::Relaxed) as f32 / 1000.0;
        let y = self.pan_delta_y.swap(0, Ordering::Relaxed) as f32 / 1000.0;
        (x, y)
    }

    /// Get zoom delta and reset it.
    pub fn take_zoom_delta(&self) -> f32 {
        self.zoom_delta.swap(0, Ordering::Relaxed) as f32 / 1000.0
    }

    /// Get input latency in microseconds.
    pub fn get_input_latency_us(&self) -> u64 {
        self.input_latency_us.load(Ordering::Relaxed)
    }

    /// Set input latency in microseconds.
    pub fn set_input_latency_us(&self, latency: u64) {
        self.input_latency_us.store(latency, Ordering::Relaxed);
    }

    /// Set forward movement state.
    pub fn set_forward(&self, value: i32) {
        self.forward.store(value, Ordering::Relaxed);
    }

    /// Set right movement state.
    pub fn set_right(&self, value: i32) {
        self.right.store(value, Ordering::Relaxed);
    }

    /// Set up movement state.
    pub fn set_up(&self, value: i32) {
        self.up.store(value, Ordering::Relaxed);
    }

    /// Set boost state.
    pub fn set_boost(&self, value: bool) {
        self.boost.store(value, Ordering::Relaxed);
    }

    /// Get atomic references for direct access (useful for input threads).
    pub fn get_forward_atomic(&self) -> Arc<AtomicI32> {
        self.forward.clone()
    }

    pub fn get_right_atomic(&self) -> Arc<AtomicI32> {
        self.right.clone()
    }

    pub fn get_up_atomic(&self) -> Arc<AtomicI32> {
        self.up.clone()
    }

    pub fn get_boost_atomic(&self) -> Arc<AtomicBool> {
        self.boost.clone()
    }
}



impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

impl CameraSpeedControl for InputState {
    fn get_move_speed(&self) -> f32 {
        f32::from_bits(self.move_speed.load(Ordering::Relaxed))
    }

    fn adjust_move_speed(&self, delta: f32) {
        let current = f32::from_bits(self.move_speed.load(Ordering::Relaxed));
        let new_speed = (current + delta).max(MIN_MOVE_SPEED).min(MAX_MOVE_SPEED);
        self.move_speed.store(new_speed.to_bits(), Ordering::Relaxed);
        let verify = f32::from_bits(self.move_speed.load(Ordering::Relaxed));
        tracing::info!("[INPUT_STATE] 🔧 adjust_move_speed: current={:.2}, delta={:.2}, new={:.2}, verify={:.2}, ptr={:p}", 
            current, delta, new_speed, verify, &self.move_speed as *const _);
    }
}
