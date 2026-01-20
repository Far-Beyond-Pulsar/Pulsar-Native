#![allow(warnings)]

//! # Pulsar Engine Backend
//!
//! This crate provides the backend functionalities for the Pulsar game engine, including
//! rendering, asset management, and core engine systems.
//! It is designed to be modular and extensible, allowing developers to
//! build high-performance games with ease.

pub mod subsystems;
pub mod gpu_interop;
pub mod services;

pub use tokio;
pub use subsystems::physics::PhysicsEngine;
pub use subsystems::game::{GameThread, GameState, GameObject};
pub use subsystems::render::{WgpuRenderer, BevyRenderer, Framebuffer as RenderFramebuffer};
pub use services::{GpuRenderer, GlobalRustAnalyzerCompletionProvider, RustAnalyzerManager};
pub use std::sync::Arc;

pub const ENGINE_THREADS: [&str; 8] = [
    "GameThread",
    "RenderThread",
    "AssetLoaderThread",
    "PhysicsThread",
    "AIThread",
    "AudioThread",
    "NetworkThread",
    "InputThread",
];

use tokio::sync::Mutex;

pub struct EngineBackend {
    physics_engine: Arc<Mutex<PhysicsEngine>>,
}

impl EngineBackend {
    pub async fn init() -> Self {
        profiling::profile_scope!("EngineBackend::Init");
        
        let physics_engine = Arc::new(Mutex::new(PhysicsEngine::new()));

        let physics_engine_clone = Arc::clone(&physics_engine);
        tokio::spawn(async move {
            profiling::set_thread_name("Physics Thread");
            let mut engine = physics_engine_clone.lock().await;
            engine.start().await;
        });

        EngineBackend { physics_engine }
    }
}
