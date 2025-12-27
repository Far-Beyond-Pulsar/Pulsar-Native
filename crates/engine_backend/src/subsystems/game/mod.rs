//! # Game Subsystem
//!
//! This module manages the game logic thread, including object updates, game state management,
//! and tick-based simulation. The game thread runs independently from the render thread,
//! providing consistent simulation updates at a target tick rate (TPS - Ticks Per Second).
//!
//! # Design
//! - **Independent Game Thread**: Runs at a fixed tick rate (default 60 TPS) for deterministic simulation
//! - **Object Management**: Updates positions, velocities, and other game state
//! - **Performance Monitoring**: Tracks TPS and provides metrics for debugging
//! - **Thread Synchronization**: Uses Arc/Mutex for thread-safe state sharing
//!
//! # Features
//! - Fixed timestep game loop for consistent simulation
//! - TPS monitoring and adaptive throttling
//! - Object movement and transformation updates
//! - Integration with physics and world systems
//! - Performance profiling and diagnostics

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};

/// Represents a game object with position, velocity, and other properties
#[derive(Debug, Clone)]
pub struct GameObject {
    pub id: u64,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub rotation: [f32; 3],
    pub scale: [f32; 3],
    pub active: bool,
}

impl GameObject {
    pub fn new(id: u64, x: f32, y: f32, z: f32) -> Self {
        Self {
            id,
            position: [x, y, z],
            velocity: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            scale: [1.0, 1.0, 1.0],
            active: true,
        }
    }

    pub fn with_velocity(mut self, vx: f32, vy: f32, vz: f32) -> Self {
        self.velocity = [vx, vy, vz];
        self
    }

    /// Update object position based on velocity and delta time
    pub fn update(&mut self, delta_time: f32) {
        if !self.active {
            return;
        }

        self.position[0] += self.velocity[0] * delta_time;
        self.position[1] += self.velocity[1] * delta_time;
        self.position[2] += self.velocity[2] * delta_time;

        // Update rotation - FAST and OBVIOUS rotation for great visual feedback
        self.rotation[0] += 120.0 * delta_time; // X-axis rotation (120 degrees/sec)
        self.rotation[1] += 80.0 * delta_time;  // Y-axis rotation (80 degrees/sec)
        self.rotation[2] += 60.0 * delta_time;  // Z-axis rotation (60 degrees/sec)

        // Keep rotation values in 0-360 range
        for i in 0..3 {
            if self.rotation[i] >= 360.0 {
                self.rotation[i] -= 360.0;
            }
        }

        // Simple bounce logic for demo (bounce off boundaries)
        for i in 0..3 {
            if self.position[i] < -10.0 || self.position[i] > 10.0 {
                self.velocity[i] = -self.velocity[i];
                self.position[i] = self.position[i].clamp(-10.0, 10.0);
            }
        }
    }
}

/// Game state containing all game objects and world data
#[derive(Debug)]
pub struct GameState {
    pub objects: Vec<GameObject>,
    pub tick_count: u64,
    pub game_time: f64,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            objects: Vec::new(),
            tick_count: 0,
            game_time: 0.0,
        }
    }

    pub fn add_object(&mut self, object: GameObject) {
        self.objects.push(object);
    }

    pub fn update(&mut self, delta_time: f32) {
        self.tick_count += 1;
        self.game_time += delta_time as f64;

        // Update all active objects
        for object in &mut self.objects {
            object.update(delta_time);
        }
    }

    pub fn get_object(&self, id: u64) -> Option<&GameObject> {
        self.objects.iter().find(|obj| obj.id == id)
    }

    pub fn get_object_mut(&mut self, id: u64) -> Option<&mut GameObject> {
        self.objects.iter_mut().find(|obj| obj.id == id)
    }
}

/// Game thread manager - runs the game loop at a fixed tick rate
pub struct GameThread {
    state: Arc<Mutex<GameState>>,
    enabled: Arc<AtomicBool>,
    target_tps: f32,
    tps: Arc<Mutex<f32>>,
    frame_count: Arc<AtomicU64>,
}

impl GameThread {
    pub fn new(target_tps: f32) -> Self {
        tracing::info!("[GAME-THREAD] ===== Creating Game Thread =====");
        let mut initial_state = GameState::new();
        
        // Add some demo objects with different velocities and starting rotations
        // FAST, OBVIOUS MOVEMENT for easy visibility
        initial_state.add_object({
            let mut obj = GameObject::new(1, 0.0, 0.0, 0.0).with_velocity(2.0, 0.0, 1.5);
            obj.rotation = [0.0, 0.0, 0.0];
            obj
        });
        initial_state.add_object({
            let mut obj = GameObject::new(2, -2.0, 0.0, 0.0).with_velocity(1.5, 0.0, -1.0);
            obj.rotation = [45.0, 90.0, 0.0];
            obj
        });
        initial_state.add_object({
            let mut obj = GameObject::new(3, 2.0, 0.0, 0.0).with_velocity(-1.0, 0.0, 1.5);
            obj.rotation = [90.0, 0.0, 45.0];
            obj
        });
        initial_state.add_object({
            let mut obj = GameObject::new(4, 0.0, 0.0, -2.0).with_velocity(-1.5, 0.0, 2.0);
            obj.rotation = [180.0, 45.0, 90.0];
            obj
        });
        
        tracing::info!("[GAME-THREAD] Added {} demo objects (with rotation)", initial_state.objects.len());
        tracing::info!("[GAME-THREAD] Target TPS: {}", target_tps);

        Self {
            state: Arc::new(Mutex::new(initial_state)),
            enabled: Arc::new(AtomicBool::new(true)),
            target_tps,
            tps: Arc::new(Mutex::new(0.0)),
            frame_count: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn get_state(&self) -> Arc<Mutex<GameState>> {
        self.state.clone()
    }

    pub fn get_tps(&self) -> f32 {
        *self.tps.lock().unwrap()
    }

    pub fn get_tick_count(&self) -> u64 {
        self.frame_count.load(Ordering::Relaxed)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn toggle(&self) {
        let current = self.enabled.load(Ordering::Relaxed);
        self.enabled.store(!current, Ordering::Relaxed);
    }

    /// Start the game thread with fixed timestep game loop
    pub fn start(&self) {
        let state = self.state.clone();
        let enabled = self.enabled.clone();
        let target_tps = self.target_tps;
        let tps = self.tps.clone();
        let frame_count = self.frame_count.clone();

        tracing::info!("[GAME-THREAD] ⚡ start() method called - about to spawn thread...");
        
        thread::spawn(move || {
            tracing::info!("[GAME-THREAD] 🚀 Thread spawned successfully!");
            
            // Set thread priority for game logic
            #[cfg(target_os = "windows")]
            {
                unsafe {
                    let handle = GetCurrentThread();
                    let _ = SetThreadPriority(handle, THREAD_PRIORITY_ABOVE_NORMAL);
                }
                tracing::info!("[GAME-THREAD] Started with high priority on Windows");
            }

            #[cfg(not(target_os = "windows"))]
            {
                tracing::info!("[GAME-THREAD] Started (priority control not available on this platform)");
            }

            let target_frame_time = Duration::from_secs_f32(1.0 / target_tps);
            let mut last_tick = Instant::now();
            let mut tps_timer = Instant::now();
            let mut tick_count = 0u32;
            let mut accumulated_time = Duration::ZERO;

            tracing::info!("[GAME-THREAD] Starting game loop at target {} TPS", target_tps);
            tracing::info!("[GAME-THREAD] Target frame time: {:?}", target_frame_time);

            loop {
                // Check if thread is disabled - if so, sleep and skip this iteration
                if !enabled.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                let frame_start = Instant::now();
                let delta = frame_start - last_tick;
                last_tick = frame_start;
                accumulated_time += delta;

                // Fixed timestep update
                let fixed_dt = 1.0 / target_tps;
                let max_steps = 5; // Prevent spiral of death
                let mut steps = 0;

                while accumulated_time >= target_frame_time && steps < max_steps {
                    // Update game state
                    if let Ok(mut game_state) = state.try_lock() {
                        game_state.update(fixed_dt);
                    }

                    accumulated_time -= target_frame_time;
                    steps += 1;
                    tick_count += 1;
                    frame_count.fetch_add(1, Ordering::Relaxed);
                }

                // Calculate TPS every second
                if tps_timer.elapsed() >= Duration::from_secs(1) {
                    let measured_tps = tick_count as f32 / tps_timer.elapsed().as_secs_f32();
                    if let Ok(mut tps_lock) = tps.lock() {
                        *tps_lock = measured_tps;
                    }

                    tick_count = 0;
                    tps_timer = Instant::now();
                }

                // Sleep to maintain target TPS with some CPU throttling
                let frame_time = frame_start.elapsed();
                if frame_time < target_frame_time {
                    let sleep_time = target_frame_time - frame_time;
                    thread::sleep(sleep_time);
                }

                // Periodic yield for system responsiveness
                if frame_count.load(Ordering::Relaxed) % 30 == 0 {
                    thread::yield_now();
                }
            }

            tracing::info!("[GAME-THREAD] Stopped");
        });
        
        tracing::info!("[GAME-THREAD] ✅ Thread spawn completed, returning from start()");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_game_object_creation() {
        let obj = GameObject::new(1, 1.0, 2.0, 3.0);
        assert_eq!(obj.id, 1);
        assert_eq!(obj.position, [1.0, 2.0, 3.0]);
        assert!(obj.active);
    }

    #[test]
    fn test_game_object_update() {
        let mut obj = GameObject::new(1, 0.0, 0.0, 0.0).with_velocity(1.0, 2.0, 3.0);
        obj.update(1.0);
        assert_eq!(obj.position, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn test_game_state() {
        let mut state = GameState::new();
        state.add_object(GameObject::new(1, 0.0, 0.0, 0.0));
        assert_eq!(state.objects.len(), 1);
        assert!(state.get_object(1).is_some());
        assert!(state.get_object(999).is_none());
    }
}
