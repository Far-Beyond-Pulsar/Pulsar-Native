//! Game viewport — a dock tab that runs the embedded game for Play-In-Editor
//! (issue #243).
//!
//! Unlike the editor's `HelioViewport` (which renders the *scene* for editing),
//! this panel hosts the compiled game: it loads the `PieHost` once the background
//! build finishes, ticks it each frame, and blits the game's offscreen texture
//! into its own `WgpuSurface`. It captures keyboard/mouse when clicked and
//! releases on `Esc`, so the game — not the editor — receives input while you
//! play in this tab.
//!
//! The tab is created once at workspace init and self-manages off the shared
//! `play.pie` state set by `begin_pie`/`end_pie`: build → load → run → stop.

use std::sync::Arc;
use std::time::Instant;

use engine_backend::services::{PieBlit, PieHost};
use gpui::*;
use pulsar_pie_abi::{input_kind, InputEvent};
use rust_i18n::t;
use ui::{ActiveTheme as _, ContextModal as _, notification::Notification};

use crate::level_editor::state::LevelEditorState;

pub struct GameViewport {
    shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
    surface: Option<WgpuSurfaceHandle>,
    pie_host: Option<PieHost>,
    pie_blit: Option<PieBlit>,
    last_frame: Instant,
    /// Whether this tab currently owns input (click to capture, Esc to release).
    captured: bool,
    /// Track transitions for one-shot notifications.
    was_active: bool,
}

impl GameViewport {
    pub fn new(
        shared_state: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            shared_state,
            focus_handle: cx.focus_handle(),
            surface: None,
            pie_host: None,
            pie_blit: None,
            last_frame: Instant::now(),
            captured: false,
            was_active: false,
        }
    }

    fn forward(&self, ev: InputEvent) {
        if let Some(host) = &self.pie_host {
            host.input(&ev);
        }
    }

    /// Map a GPUI key name to a rough code the game can key off. The embed layer
    /// only consumes a subset today; this keeps the wire format stable.
    fn key_code(key: &str) -> u32 {
        let mut bytes = key.bytes();
        match (bytes.next(), key.len()) {
            (Some(b), 1) => b as u32,
            _ => 0,
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key.as_str() == "escape" {
            if self.captured {
                self.captured = false;
                cx.notify();
                cx.stop_propagation();
            }
            return;
        }
        if self.captured {
            self.forward(InputEvent {
                kind: input_kind::KEY,
                x: 0.0,
                y: 0.0,
                button_or_key: Self::key_code(event.keystroke.key.as_str()),
                pressed: 1,
                delta: 0.0,
            });
            cx.stop_propagation();
        }
    }

    fn on_key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.captured {
            self.forward(InputEvent {
                kind: input_kind::KEY,
                x: 0.0,
                y: 0.0,
                button_or_key: Self::key_code(event.keystroke.key.as_str()),
                pressed: 0,
                delta: 0.0,
            });
            cx.stop_propagation();
        }
    }

    fn on_mouse_down(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Click captures input and focuses the panel for keyboard events. While
        // captured, all mouse/keyboard events are swallowed (stop_propagation)
        // and forwarded to the game so the editor does not also react.
        self.captured = true;
        self.focus_handle.focus(window, cx);
        self.forward(InputEvent {
            kind: input_kind::MOUSE_BUTTON,
            x: 0.0,
            y: 0.0,
            button_or_key: 0,
            pressed: 1,
            delta: 0.0,
        });
        cx.stop_propagation();
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        _event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.captured {
            return;
        }
        // Forwarding pointer motion: the embed layer normalises against its own
        // render size. Kept minimal until the game input model needs cursor pos.
        self.forward(InputEvent {
            kind: input_kind::MOUSE_MOVE,
            x: 0.0,
            y: 0.0,
            button_or_key: 0,
            pressed: 0,
            delta: 0.0,
        });
        cx.stop_propagation();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.captured {
            return;
        }
        let delta = match event.delta {
            ScrollDelta::Lines(v) => v.y,
            ScrollDelta::Pixels(p) => f32::from(p.y),
        };
        self.forward(InputEvent {
            kind: input_kind::MOUSE_WHEEL,
            x: 0.0,
            y: 0.0,
            button_or_key: 0,
            pressed: 0,
            delta,
        });
        cx.stop_propagation();
    }

    /// Load a pending game, honour a stop request, tick + blit the active game.
    fn drive(&mut self, surface: &WgpuSurfaceHandle, view: &wgpu::TextureView, w: u32, h: u32) {
        let (stop, pending) = {
            let mut st = self.shared_state.write();
            let stop = std::mem::take(&mut st.play.pie.stop_requested);
            let pending = st.play.pie.pending_start.take();
            (stop, pending)
        };

        if stop {
            if let Some(mut host) = self.pie_host.take() {
                host.stop();
            }
            self.captured = false;
            self.shared_state.write().play.pie.active = false;
        }

        if let Some(req) = pending {
            let loaded = unsafe {
                PieHost::load(
                    &req.dylib_path,
                    surface.device(),
                    surface.queue(),
                    surface.format(),
                    w,
                    h,
                    &req.project_root,
                    Some(&req.scene_path),
                )
            };
            match loaded {
                Ok(host) => {
                    self.pie_host = Some(host);
                    self.last_frame = Instant::now();
                    let mut st = self.shared_state.write();
                    st.play.pie.active = true;
                    st.play.pie.last_error = None;
                }
                Err(e) => {
                    tracing::error!("PiE load failed: {e}");
                    self.shared_state.write().play.pie.last_error = Some(e);
                }
            }
        }

        if let Some(host) = self.pie_host.as_mut() {
            host.resize(w, h);
            let now = Instant::now();
            let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
            self.last_frame = now;
            host.tick(dt);
        }

        let format = surface.format();
        if self.pie_blit.as_ref().map(|b| b.format) != Some(format) {
            self.pie_blit = Some(PieBlit::new(surface.device(), format));
        }
        if let (Some(blit), Some(host)) = (self.pie_blit.as_ref(), self.pie_host.as_ref()) {
            if let Some(tex) = unsafe { host.out_texture() } {
                blit.blit(surface.device(), surface.queue(), tex, view);
            }
        }
    }

    /// Notify once when the game becomes active. Build progress + failures are
    /// surfaced by the level-editor panel (which always exists); this panel only
    /// exists while a game is starting/running.
    fn poll_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.shared_state.read().play.pie.active;
        if active && !self.was_active {
            window.push_notification(
                Notification::success(t!("Notification.Title.PlayInEditor").to_string())
                    .message(t!("Notification.Message.GameRunning").to_string()),
                cx,
            );
        }
        self.was_active = active;
    }
}

impl EventEmitter<ui::dock::PanelEvent> for GameViewport {}

ui_common::panel_boilerplate!(GameViewport);

impl ui::dock::Panel for GameViewport {
    fn panel_name(&self) -> &'static str {
        "game-viewport"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Game".into_any_element()
    }

    /// Never cached: `render` is what ticks and blits the game, and what asks
    /// for the next animation frame. A reused frame would run none of it.
    fn cacheable(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for GameViewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Keep the game ticking every frame while active.
        window.request_animation_frame();
        self.poll_status(window, cx);

        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        if self.surface.is_none() {
            self.surface = window.create_wgpu_surface(1280, 720, format);
        }

        // Load / tick / blit the game into the back buffer.
        let surface_handle = self.surface.clone();
        if let Some(surface) = surface_handle {
            if !surface.is_resize_pending() {
                if let Some((view, (w, h))) = surface.back_view_with_size() {
                    self.drive(&surface, &view, w, h);
                    drop(view);
                    surface.swap_buffers();
                }
            }
        }

        let running = self.pie_host.is_some();
        let building = self.shared_state.read().play.pie.building;
        let status: SharedString = if running {
            if self.captured {
                "Playing — press Esc to release the mouse".into()
            } else {
                "Click to control · Esc releases".into()
            }
        } else if building {
            "Building game…".into()
        } else {
            "Press Start Simulation to play".into()
        };

        let surface_el = if let Some(ref s) = self.surface {
            wgpu_surface(s.clone())
                .absolute()
                .inset_0()
                .into_any_element()
        } else {
            div().absolute().inset_0().into_any_element()
        };

        div()
            .track_focus(&self.focus_handle)
            .key_context("GameViewport")
            .size_full()
            .relative()
            .bg(gpui::rgb(0x000000))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_key_up(cx.listener(Self::on_key_up))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(surface_el)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right(px(12.0))
                    .bottom(px(12.0))
                    .child(
                        div()
                            .absolute()
                            .top_2()
                            .left_2()
                            .px_2()
                            .py_1()
                            .rounded(px(4.0))
                            .bg(gpui::rgba(0x000000a0))
                            .text_color(cx.theme().foreground)
                            .text_sm()
                            .child(status),
                    ),
            )
    }
}
