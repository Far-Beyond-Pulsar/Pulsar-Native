//! Event loop and windowing setup for Pulsar Engine
//
// This module encapsulates event loop and window creation logic.

use winit::event_loop::{EventLoop, ControlFlow};
use crate::window::WinitGpuiApp;
use crate::engine_state::{EngineContext, WindowRequest};
use std::sync::mpsc::Receiver;

/// Run the main event loop with EngineContext.
pub fn run_event_loop(engine_context: EngineContext, window_rx: Receiver<WindowRequest>) {
    profiling::profile_scope!("Engine::EventLoop");

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut app = WinitGpuiApp::new(engine_context, window_rx);
    event_loop.run_app(&mut app).expect("Failed to run event loop");
}
