//! The window: creates the game and feeds it winit events.

use crate::game::Game;
use crate::gfx::Gfx;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize, Size};
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

#[derive(Default)]
pub struct App {
    game: Option<Game>,
}

impl App {
    fn capture(game: &mut Game, capture: bool) {
        let window = &game.gfx.window;
        if capture {
            let ok = window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined))
                .is_ok();
            window.set_cursor_visible(!ok);
            game.input.captured = ok;
        } else {
            let _ = window.set_cursor_grab(CursorGrabMode::None);
            window.set_cursor_visible(true);
            game.input.captured = false;
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.game.is_some() {
            return;
        }
        event_loop.set_control_flow(ControlFlow::Poll);
        // `VOXEL_CRAFT_SIZE=<w>x<h>`: the window's size in pixels (measurements at a
        // known resolution); otherwise 1600 x 900 logical.
        let size = std::env::var("VOXEL_CRAFT_SIZE")
            .ok()
            .and_then(|v| {
                v.split_once('x')
                    .and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?)))
            })
            .map_or(Size::Logical(LogicalSize::new(1600.0, 900.0)), |(w, h)| {
                Size::Physical(PhysicalSize::new(w, h))
            });
        let window: Arc<Window> = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("Voxel Craft: building the world...")
                        .with_inner_size(size),
                )
                .expect("window"),
        );
        let mut game = Game::new(|planet| Gfx::new(window.clone(), planet));
        // A tour takes no input (its shots compare), so it leaves the cursor
        // to whoever is at the machine.
        if !game.touring() && !game.gfx.vr_active() {
            Self::capture(&mut game, true);
        }
        self.game = Some(game);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(game) = &mut self.game else { return };
        let input = !game.touring();
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => game.gfx.resize(size.width, size.height),
            WindowEvent::KeyboardInput { event, .. } if input => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    if code == KeyCode::Escape && event.state == ElementState::Pressed {
                        if game.input.captured {
                            Self::capture(game, false);
                        } else {
                            event_loop.exit();
                        }
                    } else {
                        game.input.key(code, event.state);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } if input => {
                if !game.gfx.vr_active()
                    && !game.input.captured
                    && state == ElementState::Pressed
                    && button == MouseButton::Left
                {
                    Self::capture(game, true);
                } else {
                    game.input.button(button, state);
                }
            }
            WindowEvent::MouseWheel { delta, .. } if input => game.input.wheel(delta),
            WindowEvent::Focused(false) => {
                game.input.release_all();
                Self::capture(game, false);
            }
            WindowEvent::RedrawRequested => {
                game.frame();
                if game.quit {
                    event_loop.exit();
                }
                game.gfx.window.request_redraw();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let (Some(game), DeviceEvent::MouseMotion { delta: (dx, dy) }) = (&mut self.game, event)
        {
            if !game.touring() {
                game.input.mouse_motion(dx, dy);
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(game) = &self.game {
            game.gfx.window.request_redraw();
        }
    }
}
