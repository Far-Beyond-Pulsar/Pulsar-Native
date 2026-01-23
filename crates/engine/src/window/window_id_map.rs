//! Safe WindowId to u64 mapping
//!
//! This module provides safe conversion between Winit's WindowId and u64,
//! avoiding the need for unsafe transmute operations.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use winit::window::WindowId;

/// Thread-safe mapping between WindowId and u64
pub struct WindowIdMap {
    next_id: AtomicU64,
    forward_map: Mutex<HashMap<WindowId, u64>>,
    reverse_map: Mutex<HashMap<u64, WindowId>>,
}

impl WindowIdMap {
    /// Create a new empty WindowIdMap
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1), // Start from 1, reserve 0 for "none"
            forward_map: Mutex::new(HashMap::new()),
            reverse_map: Mutex::new(HashMap::new()),
        }
    }

    /// Register a new WindowId and return its u64 identifier
    /// If the WindowId is already registered, returns the existing u64
    pub fn register(&self, window_id: WindowId) -> u64 {
        let mut forward = self.forward_map.lock().unwrap();

        // Check if already registered
        if let Some(&id) = forward.get(&window_id) {
            return id;
        }

        // Allocate new ID
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        forward.insert(window_id, id);

        let mut reverse = self.reverse_map.lock().unwrap();
        reverse.insert(id, window_id);

        id
    }

    /// Get the u64 for a WindowId if it exists
    pub fn get_id(&self, window_id: &WindowId) -> Option<u64> {
        self.forward_map.lock().unwrap().get(window_id).copied()
    }

    /// Get the WindowId for a u64 if it exists
    pub fn get_window_id(&self, id: u64) -> Option<WindowId> {
        self.reverse_map.lock().unwrap().get(&id).copied()
    }

    /// Remove a WindowId from the mapping
    pub fn remove(&self, window_id: &WindowId) -> Option<u64> {
        let mut forward = self.forward_map.lock().unwrap();
        if let Some(id) = forward.remove(window_id) {
            let mut reverse = self.reverse_map.lock().unwrap();
            reverse.remove(&id);
            Some(id)
        } else {
            None
        }
    }
}

impl Default for WindowIdMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::window::WindowId;

    // Note: WindowId cannot be constructed in tests without an actual window,
    // so these tests are limited. In real usage, the mapping is tested
    // through integration tests.
}
