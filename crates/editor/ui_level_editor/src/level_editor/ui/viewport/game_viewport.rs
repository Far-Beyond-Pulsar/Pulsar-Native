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
//!
//! **Events panel (#924).** The "Events" chip opens an overlay listing the
//! game's recently flushed events (frame, flush point, name, channel,
//! payload summary, subscribers on that channel) and the events with
//! global subscribers, from the game's event hub debug tap
//! (`PieHost::events_snapshot`, polled a few times a second).

use std::sync::Arc;
use std::time::Instant;

use engine_backend::services::{PieBlit, PieHost};
use gpui::*;
use gpui::prelude::FluentBuilder as _;
use pulsar_pie_abi::{input_kind, InputEvent};
use rust_i18n::t;
use ui::{notification::Notification, ActiveTheme as _, ContextModal as _};

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
    /// The events overlay is open.
    events_open: bool,
    /// The game's last event hub snapshot, and when it was taken.
    events: Option<pulsar_events::EventsSnapshot>,
    events_polled: Instant,
}

/// How often the events overlay re-reads the game's event tap.
const EVENTS_POLL: std::time::Duration = std::time::Duration::from_millis(250);
/// Rows the events overlay shows.
const EVENTS_ROWS: usize = 24;

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
            events_open: false,
            events: None,
            events_polled: Instant::now(),
        }
    }

    /// Refresh the events snapshot (on the thread that ticks the game).
    fn poll_events(&mut self) {
        if !self.events_open || self.events_polled.elapsed() < EVENTS_POLL {
            return;
        }
        self.events_polled = Instant::now();
        self.events = self.pie_host.as_ref().and_then(PieHost::events_snapshot);
    }

    fn events_overlay(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let row = |text: String| div().text_xs().whitespace_nowrap().overflow_hidden().child(text);
        let mut list = div().flex().flex_col().gap(px(2.0));
        match &self.events {
            None => {
                list = list.child(row("No event data (game not running, or built without the events tap).".into()));
            }
            Some(snapshot) => {
                list = list.child(row(format!(
                    "frame {} · {} queued · {} recent",
                    snapshot.frame,
                    snapshot.queued,
                    snapshot.recent.len()
                )));
                for r in snapshot.recent.iter().rev().take(EVENTS_ROWS) {
                    list = list.child(row(format!(
                        "#{} f{} [{}] {} @ {} ({} subs) {}",
                        r.seq, r.frame, r.point, r.name, r.channel, r.subscribers, r.summary
                    )));
                }
                let listened: Vec<String> = snapshot
                    .events
                    .iter()
                    .filter(|e| e.global_subscribers > 0)
                    .map(|e| format!("{} ×{}", e.name, e.global_subscribers))
                    .collect();
                if !listened.is_empty() {
                    list = list.child(row(format!("global subscribers: {}", listened.join(", "))));
                }
            }
        }
        div()
            .absolute()
            .top(px(36.0))
            .right(px(8.0))
            .w(px(520.0))
            .max_h(px(420.0))
            .overflow_hidden()
            .p_2()
            .rounded(px(4.0))
            .bg(gpui::rgba(0x000000c0))
            .text_color(theme.foreground)
            .child(list)
            .into_any_element()
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

    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            let mut state = self.shared_state.write();
            state.play.pie.active = false;
            state.play.pie.pending_asset_updates.clear();
            state.play.pie.loaded_artifact = None;
            state.play.pie.supports_control = false;
            // The game is gone: now restore the editor world (#925).
            crate::level_editor::ui::panel::pie::finish_stop(&mut state, true);
        }

        // Play again with only script changes (#833): keep the running game
        // and reload every class in it; no new library.
        let pending = match pending {
            Some(req) if req.scripts_only && self.pie_host.is_some() => {
                let events = crate::level_editor::ui::panel::pie::class_reload_events(&req.project_root);
                tracing::info!(classes = events.len(), "PiE: script-only change; reloading classes in the running game");
                self.shared_state.write().play.pie.pending_asset_updates.extend(events);
                None
            }
            other => other,
        };

        if let Some(req) = pending {
            // Native hot reload (#653): a pending start while a game is
            // already running means "swap the library". Stop the OLD host
            // first — its actors die with it, but the world survives because
            // it lives HERE, in the editor's shared store — then load the new
            // build against that same store. The guest's RELOAD session flag
            // makes project setup re-bind actor registrations to their
            // existing entities instead of spawning duplicates.
            let was_running = self.pie_host.is_some();
            if let Some(mut old) = self.pie_host.take() {
                old.stop();
            }

            // ABI v2 (#635): hand the guest THE editor's world -- one count is
            // transferred for the session; the editor keeps its own. The
            // guest adopts it, so mid-session edits and gameplay mutations
            // meet in one world.
            let shared_world = self.shared_state.read().scene.shared_scene();
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
                    shared_world,
                    req.reload,
                )
            };
            match loaded {
                Ok(host) => {
                    let mut st = self.shared_state.write();
                    st.play.pie.active = true;
                    st.play.pie.last_error = None;
                    st.play.pie.supports_control = host.has_simulation_control();
                    st.play.pie.paused = false;
                    st.play.pie.loaded_artifact = crate::level_editor::ui::panel::pie::artifact_mtime(&req.dylib_path)
                        .map(|mtime| (req.dylib_path.clone(), mtime));
                    drop(st);
                    self.pie_host = Some(host);
                    self.last_frame = Instant::now();
                }
                Err(e) => {
                    tracing::error!("PiE load failed: {e}");
                    // On a failed reload the previous game is already gone;
                    // surface why instead of leaving a dead viewport silent.
                    let mut st = self.shared_state.write();
                    st.play.pie.active = false;
                    st.play.pie.last_error = Some(if was_running {
                        format!("Hot reload failed — game stopped. {e}")
                    } else {
                        e
                    });
                }
            }
        }

        if let Some(host) = self.pie_host.as_mut() {
            // Class edits made while playing (#921): the game reloads them
            // at its next frame, no rebuild (#833).
            let (pending, pause, step) = {
                let mut st = self.shared_state.write();
                (
                    std::mem::take(&mut st.play.pie.pending_asset_updates),
                    st.play.pie.pause_request.take(),
                    std::mem::take(&mut st.play.pie.step_request),
                )
            };
            for event in &pending {
                host.asset_updated(event);
            }
            // Pause / step (#925), wired to the game's TickLoop.
            match pause {
                Some(true) => {
                    host.pause();
                }
                Some(false) => {
                    host.resume();
                }
                None => {}
            }
            if step > 0 {
                host.step(step);
            }
            host.resize(w, h);
            let now = Instant::now();
            let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.1);
            self.last_frame = now;
            host.tick(dt);
            Self::report_problems(host, &self.shared_state);
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

    /// Forward the game's script problems (#854, #868) to the editor: the
    /// problems panel and the Blueprint editor hear them on the host bus.
    /// With the "pause on script error" setting, an error pauses the game
    /// before its next frame.
    fn report_problems(host: &PieHost, shared_state: &Arc<parking_lot::RwLock<LevelEditorState>>) {
        let problems = host.take_problems();
        if problems.is_empty() {
            let paused = host.is_paused().unwrap_or(false);
            let mut st = shared_state.write();
            if st.play.pie.paused != paused {
                st.play.pie.paused = paused;
            }
            return;
        }
        let pause = pause_on_script_error()
            && problems.iter().any(|p| p.severity == pulsar_events::ProblemSeverity::Error);
        for problem in &problems {
            match problem.severity {
                pulsar_events::ProblemSeverity::Error => tracing::error!("Script error: {}", problem.summary()),
                pulsar_events::ProblemSeverity::Warning => tracing::warn!("Script warning: {}", problem.summary()),
            }
            pulsar_events::publish_script_problem(problem.clone());
        }
        if pause {
            host.pause();
        }
        let paused = host.is_paused().unwrap_or(false);
        let mut st = shared_state.write();
        st.play.pie.paused = paused;
        st.play.pie.problems.extend(problems);
        let excess = st
            .play
            .pie
            .problems
            .len()
            .saturating_sub(crate::level_editor::state::MAX_PIE_PROBLEMS);
        st.play.pie.problems.drain(..excess);
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

        self.poll_events();
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
            .child(
                div()
                    .id("pie-events-toggle")
                    .absolute()
                    .top_2()
                    .right_2()
                    .px_2()
                    .py_1()
                    .rounded(px(4.0))
                    .bg(gpui::rgba(0x000000a0))
                    .text_color(cx.theme().foreground)
                    .text_sm()
                    .cursor_pointer()
                    .child(if self.events_open { "Events ▾" } else { "Events ▸" })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.events_open = !this.events_open;
                            // Poll right away when opened.
                            this.events_polled = Instant::now() - EVENTS_POLL;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .when(self.events_open, |el| el.child(self.events_overlay(cx)))
    }
}

/// The editor setting `editor.debugger.pie_on_script_error`: `"pause"`
/// pauses Play-in-Editor on a script error, `"continue"` (the default) keeps
/// it running and only reports the error.
pub(crate) fn pause_on_script_error() -> bool {
    matches!(
        engine_state::settings::global_config().get(
            engine_state::settings::NS_EDITOR,
            "debugger",
            "pie_on_script_error",
        ),
        Ok(engine_state::settings::ConfigValue::String(ref value)) if value == "pause"
    )
}
