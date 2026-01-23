//! Event Handling Utilities
//!
//! This module provides utilities for converting and processing events from Winit to GPUI,
//! including mouse/keyboard input conversion, motion smoothing, and click detection.
//!
//! ## Components
//!
//! - `convert_mouse_button()` - Converts Winit mouse buttons to GPUI format
//! - `convert_modifiers()` - Converts Winit keyboard modifiers to GPUI format
//! - `SimpleClickState` - Tracks clicks for double-click detection
//! - `MotionSmoother` - Smooths mouse movement with interpolation
//!
//! ## Motion Smoothing
//!
//! The `MotionSmoother` provides client-side prediction-style interpolation for mouse
//! movement, reducing jitter and providing higher quality input:
//!
//! ```text
//! Raw Input → Update Target → Interpolate → Smoothed Position
//!     ↓            ↓              ↓              ↓
//!   Jumpy       Calculate       Apply          Smooth
//!   Motion      Velocity        Easing         Motion
//! ```

use gpui::*;
use std::collections::HashSet;
use std::time::{Duration, Instant};
use winit::event::MouseButton as WinitMouseButton;

/// Extension trait for converting Winit MouseButton to GPUI MouseButton
///
/// Provides idiomatic `.to_gpui()` method for type conversion.
pub trait ToGpuiMouseButton {
    fn to_gpui(self) -> MouseButton;
}

impl ToGpuiMouseButton for WinitMouseButton {
    fn to_gpui(self) -> MouseButton {
        match self {
            WinitMouseButton::Left => MouseButton::Left,
            WinitMouseButton::Right => MouseButton::Right,
            WinitMouseButton::Middle => MouseButton::Middle,
            WinitMouseButton::Back => MouseButton::Navigate(NavigationDirection::Back),
            WinitMouseButton::Forward => MouseButton::Navigate(NavigationDirection::Forward),
            WinitMouseButton::Other(_) => MouseButton::Left, // Fallback to left for unknown buttons
        }
    }
}

/// Extension trait for converting Winit modifiers to GPUI modifiers
///
/// Provides idiomatic `.to_gpui()` method for type conversion.
pub trait ToGpuiModifiers {
    fn to_gpui(&self) -> Modifiers;
}

impl ToGpuiModifiers for winit::keyboard::ModifiersState {
    fn to_gpui(&self) -> Modifiers {
        Modifiers {
            control: self.control_key(),
            alt: self.alt_key(),
            shift: self.shift_key(),
            platform: self.super_key(), // Windows key on Windows, Command on Mac
            function: false,            // Winit doesn't track function key separately
        }
    }
}

/// Convert Winit MouseButton to GPUI MouseButton
///
/// # Arguments
/// * `button` - Winit mouse button to convert
///
/// # Returns
/// Equivalent GPUI mouse button
///
/// # Note
/// Consider using the extension trait method `button.to_gpui()` for more idiomatic code.
pub fn convert_mouse_button(button: WinitMouseButton) -> MouseButton {
    button.to_gpui()
}

/// Convert Winit keyboard modifiers to GPUI modifiers
///
/// # Arguments
/// * `winit_mods` - Winit modifier state
///
/// # Returns
/// GPUI Modifiers struct
///
/// # Note
/// Consider using the extension trait method `winit_mods.to_gpui()` for more idiomatic code.
pub fn convert_modifiers(winit_mods: &winit::keyboard::ModifiersState) -> Modifiers {
    winit_mods.to_gpui()
}

/// Simple click state tracking for double-click detection
///
/// Tracks mouse button clicks to detect double-clicks and triple-clicks
/// based on time and distance thresholds.
///
/// # Example
///
/// ```rust,ignore
/// let mut click_state = SimpleClickState::new();
/// let click_count = click_state.update(MouseButton::Left, position);
/// if click_count == 2 {
///     tracing::debug!("Double click!");
/// }
/// ```
#[derive(Debug, Clone)]
pub struct SimpleClickState {
    /// Last button that was clicked
    pub last_button: MouseButton,
    /// Time of last click
    last_click_time: Instant,
    /// Position of last click
    last_click_position: Point<Pixels>,
    /// Current click count (1, 2, 3, etc.)
    pub current_count: usize,
    /// Maximum distance between clicks (in pixels)
    double_click_distance: f32,
    /// Maximum time between clicks
    double_click_duration: Duration,
}

impl SimpleClickState {
    /// Create a new click state tracker with standard double-click thresholds
    ///
    /// Uses industry-standard thresholds:
    /// - 4 pixels maximum distance
    /// - 500ms maximum time between clicks
    pub fn new() -> Self {
        Self {
            last_button: MouseButton::Left,
            last_click_time: Instant::now(),
            last_click_position: point(px(0.0), px(0.0)),
            current_count: 0,
            double_click_distance: 4.0,                     // Standard double-click tolerance
            double_click_duration: Duration::from_millis(500),
        }
    }

    /// Update click state and return the current click count
    ///
    /// Call this when a mouse button is pressed. It will determine if this
    /// is part of a multi-click sequence (double-click, triple-click, etc.)
    ///
    /// # Arguments
    /// * `button` - The mouse button that was clicked
    /// * `position` - The cursor position when clicked
    ///
    /// # Returns
    /// Click count: 1 for single click, 2 for double, 3 for triple, etc.
    pub fn update(&mut self, button: MouseButton, position: Point<Pixels>) -> usize {
        let now = Instant::now();

        // Calculate distance using pixel operations
        let dx = (position.x - self.last_click_position.x).abs();
        let dy = (position.y - self.last_click_position.y).abs();
        let distance = dx.max(dy); // Simple Manhattan distance

        // Check if this is part of a multi-click sequence
        if button == self.last_button
            && now.duration_since(self.last_click_time) < self.double_click_duration
            && distance < px(self.double_click_distance)
        {
            self.current_count += 1;
        } else {
            self.current_count = 1;
        }

        self.last_button = button;
        self.last_click_time = now;
        self.last_click_position = position;

        self.current_count
    }
}

impl Default for SimpleClickState {
    fn default() -> Self {
        Self::new()
    }
}

/// Motion smoothing system for interpolated mouse movement
///
/// Provides smooth mouse cursor movement through interpolation, similar to
/// client-side prediction in multiplayer games. This reduces jitter and
/// provides higher-quality input at the cost of minimal latency.
///
/// # Algorithm
///
/// Uses exponential smoothing with velocity tracking:
/// 1. Target position is updated from raw input
/// 2. Velocity is calculated from target position changes
/// 3. Interpolated position gradually moves toward target
/// 4. Events are only sent when movement exceeds threshold
///
/// # Example
///
/// ```rust,ignore
/// let mut smoother = MotionSmoother::new();
///
/// // On mouse move
/// smoother.update_target(new_position);
/// if smoother.should_send_event() {
///     let smooth_pos = smoother.interpolate();
///     send_mouse_event(smooth_pos);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct MotionSmoother {
    /// Current interpolated position
    interpolated_position: Point<Pixels>,
    /// Target position (from raw input)
    target_position: Point<Pixels>,
    /// Current velocity (for prediction)
    velocity: Point<Pixels>,
    /// Last update time
    last_update: Instant,
    /// Smoothing factor (0-1, higher = more smoothing)
    smoothing_factor: f32,
    /// Minimum movement delta to trigger event (pixels)
    min_delta: f32,
    /// Minimum time between events
    min_event_interval: Duration,
    /// Last event send time
    last_event_time: Instant,
}

impl MotionSmoother {
    /// Create a new motion smoother with default parameters
    ///
    /// Default configuration:
    /// - Smoothing factor: 0.35 (moderate smoothing)
    /// - Min delta: 0.5 pixels
    /// - Min event interval: ~144 Hz (6944 microseconds)
    pub fn new() -> Self {
        Self {
            interpolated_position: point(px(0.0), px(0.0)),
            target_position: point(px(0.0), px(0.0)),
            velocity: point(px(0.0), px(0.0)),
            last_update: Instant::now(),
            smoothing_factor: 0.35,
            min_delta: 0.5,
            min_event_interval: Duration::from_micros(6944), // ~144 Hz
            last_event_time: Instant::now(),
        }
    }

    /// Update the target position from raw input
    ///
    /// Call this whenever you receive a raw mouse movement event.
    /// The smoother will calculate velocity and update its internal state.
    ///
    /// # Arguments
    /// * `new_position` - New target position from input system
    pub fn update_target(&mut self, new_position: Point<Pixels>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32();

        // Calculate velocity for prediction
        if dt > 0.0 {
            let nx: f32 = new_position.x.into();
            let tx: f32 = self.target_position.x.into();
            let ny: f32 = new_position.y.into();
            let ty: f32 = self.target_position.y.into();

            self.velocity = point(px((nx - tx) / dt), px((ny - ty) / dt));
        }

        self.target_position = new_position;
        self.last_update = now;
    }

    /// Get the current interpolated position
    ///
    /// Applies exponential smoothing to move the interpolated position
    /// toward the target. The smoothing is framerate-independent.
    ///
    /// # Returns
    /// Smoothed cursor position
    pub fn interpolate(&mut self) -> Point<Pixels> {
        let now = Instant::now();
        let dt = now.duration_since(self.last_update).as_secs_f32().min(0.1);

        // Exponential smoothing with framerate compensation
        let alpha = 1.0 - (1.0 - self.smoothing_factor).powf(dt * 60.0);

        let x: f32 = self.interpolated_position.x.into();
        let tx: f32 = self.target_position.x.into();
        self.interpolated_position.x = px(x + (tx - x) * alpha);

        let y: f32 = self.interpolated_position.y.into();
        let ty: f32 = self.target_position.y.into();
        self.interpolated_position.y = px(y + (ty - y) * alpha);

        self.interpolated_position
    }

    /// Check if enough time has passed and distance traveled to warrant sending an event
    ///
    /// This prevents flooding the event system with too many mouse move events.
    ///
    /// # Returns
    /// `true` if an event should be sent, `false` otherwise
    pub fn should_send_event(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_event_time);

        // Check time threshold
        if elapsed < self.min_event_interval {
            return false;
        }

        // Check distance threshold
        let ix: f32 = self.interpolated_position.x.into();
        let tx: f32 = self.target_position.x.into();
        let iy: f32 = self.interpolated_position.y.into();
        let ty: f32 = self.target_position.y.into();

        let dx = (ix - tx).abs();
        let dy = (iy - ty).abs();

        if dx < self.min_delta && dy < self.min_delta {
            return false;
        }

        self.last_event_time = now;
        true
    }
}

impl Default for MotionSmoother {
    fn default() -> Self {
        Self::new()
    }
}
