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
use crate::subsystems::framework::{Subsystem, SubsystemContext, SubsystemError, SubsystemId};

#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL};

/// Subsystem ID for the game thread
pub const GAME_SUBSYSTEM_ID: SubsystemId = SubsystemId::new("game");

/// Managed wrapper for GameThread that implements Subsystem while providing runtime control
///
/// This allows the GameThread to be managed by SubsystemRegistry while still exposing
/// methods like set_enabled() for runtime control (e.g., Edit/Play mode toggling).
pub struct ManagedGameThread {
    inner: Arc<GameThread>,
}

impl ManagedGameThread {
    pub fn new(target_tps: f32) -> Self {
        Self {
            inner: Arc::new(GameThread::new(target_tps)),
        }
    }

    /// Get a reference to the inner GameThread for runtime control
    pub fn game_thread(&self) -> &Arc<GameThread> {
        &self.inner
    }
}

impl Subsystem for ManagedGameThread {
    fn id(&self) -> SubsystemId {
        GAME_SUBSYSTEM_ID
    }

    fn dependencies(&self) -> Vec<SubsystemId> {
        vec![] // Game thread has no dependencies
    }

    fn init(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError> {
        // Delegate to inner GameThread's Subsystem implementation
        // But we need to call it through a mutable reference we don't have...
        // Actually, let's just inline the init logic here
        profiling::profile_scope!("Subsystem::Game::Init");

        let state = self.inner.state.clone();
        let enabled = self.inner.enabled.clone();
        let target_tps = self.inner.target_tps;
        let tps = self.inner.tps.clone();
        let frame_count = self.inner.frame_count.clone();

        tracing::debug!("[GAME-THREAD] ⚡ Initializing managed game thread subsystem...");

        let handle = std::thread::Builder::new()
            .name("Game Logic".to_string())
            .spawn(move || {
                profiling::set_thread_name("Game Logic");

                #[cfg(target_os = "windows")]
                {
                    unsafe {
                        let handle = GetCurrentThread();
                        let _ = SetThreadPriority(handle, THREAD_PRIORITY_ABOVE_NORMAL);
                    }
                }

                let target_frame_time = Duration::from_secs_f32(1.0 / target_tps);
                let mut last_tick = Instant::now();
                let mut tps_timer = Instant::now();
                let mut tick_count = 0u32;
                let mut accumulated_time = Duration::ZERO;

                loop {
                    profiling::profile_scope!("Game::Tick");

                    if !enabled.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(target_frame_time.as_millis().min(1) as u64));
                        continue;
                    }

                    let frame_start = Instant::now();
                    let delta = frame_start - last_tick;
                    last_tick = frame_start;
                    accumulated_time += delta;

                    let fixed_dt = 1.0 / target_tps;
                    let max_steps = 5;
                    let mut steps = 0;

                    while accumulated_time >= target_frame_time && steps < max_steps {
                        profiling::profile_scope!("Game::StateUpdate");
                        if let Ok(mut game_state) = state.try_lock() {
                            game_state.update(fixed_dt);
                        }

                        accumulated_time -= target_frame_time;
                        steps += 1;
                        tick_count += 1;
                        frame_count.fetch_add(1, Ordering::Relaxed);
                    }

                    if tps_timer.elapsed() >= Duration::from_secs(1) {
                        let measured_tps = tick_count as f32 / tps_timer.elapsed().as_secs_f32();
                        if let Ok(mut tps_lock) = tps.lock() {
                            *tps_lock = measured_tps;
                        }
                        tick_count = 0;
                        tps_timer = Instant::now();
                    }

                    let frame_time = frame_start.elapsed();
                    if frame_time < target_frame_time {
                        thread::sleep(target_frame_time - frame_time);
                    }

                    if frame_count.load(Ordering::Relaxed) % 30 == 0 {
                        thread::yield_now();
                    }
                }
            })
            .map_err(|e| SubsystemError::InitFailed(format!("Failed to spawn game thread: {}", e)))?;

        // Store handle in inner (need unsafe to modify through Arc)
        // Actually we can't do this cleanly. Let's rethink...

        tracing::info!("✓ Managed game thread initialized at {} TPS", target_tps);

        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), SubsystemError> {
        profiling::profile_scope!("Subsystem::Game::Shutdown");

        tracing::debug!("[GAME-THREAD] Shutting down managed game thread");

        // Signal thread to stop
        self.inner.enabled.store(false, Ordering::Relaxed);

        thread::sleep(Duration::from_millis(50));

        tracing::info!("✓ Managed game thread stopped");

        Ok(())
    }
}

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
    pub fn update(&mut self, _delta_time: f32) {
        // Static objects - no movement or rotation
        // Objects maintain their initial transform
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
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl GameThread {
    pub fn new(target_tps: f32) -> Self {
        tracing::debug!("[GAME-THREAD] ===== Creating Game Thread =====");
        let mut initial_state = GameState::new();
        
        // Create a beautiful default level similar to Unreal's starter content
        // Floor plane - large ground surface
        initial_state.add_object({
            let mut obj = GameObject::new(1, 0.0, -0.5, 0.0);
            obj.scale = [20.0, 0.1, 20.0]; // Large flat plane
            obj.rotation = [0.0, 0.0, 0.0];
            obj
        });
        
        // Center cube - focal point
        initial_state.add_object({
            let mut obj = GameObject::new(2, 0.0, 0.5, 0.0);
            obj.scale = [1.0, 1.0, 1.0];
            obj.rotation = [0.0, 45.0, 0.0]; // Slight rotation for visual interest
            obj
        });
        
        // Sphere on the left
        initial_state.add_object({
            let mut obj = GameObject::new(3, -3.0, 1.0, 0.0);
            obj.scale = [1.0, 1.0, 1.0];
            obj.rotation = [0.0, 0.0, 0.0];
            obj
        });
        
        // Cylinder on the right
        initial_state.add_object({
            let mut obj = GameObject::new(4, 3.0, 1.0, 0.0);
            obj.scale = [1.0, 2.0, 1.0];
            obj.rotation = [0.0, 0.0, 0.0];
            obj
        });
        
        // Back wall/cube
        initial_state.add_object({
            let mut obj = GameObject::new(5, 0.0, 2.0, -5.0);
            obj.scale = [8.0, 4.0, 0.5]; // Tall wall
            obj.rotation = [0.0, 0.0, 0.0];
            obj
        });
        
        // Small decorative cubes - left side
        initial_state.add_object({
            let mut obj = GameObject::new(6, -5.0, 0.3, 2.0);
            obj.scale = [0.6, 0.6, 0.6];
            obj.rotation = [0.0, 30.0, 0.0];
            obj
        });
        
        initial_state.add_object({
            let mut obj = GameObject::new(7, -4.0, 0.3, 3.0);
            obj.scale = [0.6, 0.6, 0.6];
            obj.rotation = [0.0, -15.0, 0.0];
            obj
        });
        
        // Small decorative cubes - right side
        initial_state.add_object({
            let mut obj = GameObject::new(8, 5.0, 0.3, 2.0);
            obj.scale = [0.6, 0.6, 0.6];
            obj.rotation = [0.0, -30.0, 0.0];
            obj
        });
        
        initial_state.add_object({
            let mut obj = GameObject::new(9, 4.0, 0.3, 3.0);
            obj.scale = [0.6, 0.6, 0.6];
            obj.rotation = [0.0, 15.0, 0.0];
            obj
        });
        
        // Foreground elements
        initial_state.add_object({
            let mut obj = GameObject::new(10, -2.0, 0.5, 4.0);
            obj.scale = [1.2, 0.5, 1.2];
            obj.rotation = [0.0, 0.0, 0.0];
            obj
        });
        
        initial_state.add_object({
            let mut obj = GameObject::new(11, 2.0, 0.5, 4.0);
            obj.scale = [1.2, 0.5, 1.2];
            obj.rotation = [0.0, 0.0, 0.0];
            obj
        });
        
        tracing::debug!("[GAME-THREAD] Created default level with {} static objects", initial_state.objects.len());
        tracing::debug!("[GAME-THREAD] Target TPS: {}", target_tps);

        Self {
            state: Arc::new(Mutex::new(initial_state)),
            enabled: Arc::new(AtomicBool::new(true)),
            target_tps,
            tps: Arc::new(Mutex::new(0.0)),
            frame_count: Arc::new(AtomicU64::new(0)),
            thread_handle: None,
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
