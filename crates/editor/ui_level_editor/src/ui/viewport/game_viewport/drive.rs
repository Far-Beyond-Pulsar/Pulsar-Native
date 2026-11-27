//! Per-frame driving of the embedded game: honouring stop and start requests,
//! starting it (in the editor, or by loading the game library), ticking it
//! and blitting its output.
use std::time::Instant;

use engine_backend::services::{PieBlit, PieHost};
use gpui::WgpuSurfaceHandle;
use pulsar_game::embed::{InEditorGame, InEditorStart};

use super::running::{EditorHosted, RunningGame};
use super::GameViewport;
use crate::state::PieStartRequest;

impl GameViewport {
    /// Load a pending game, honour a stop request, tick + blit the active game.
    pub(super) fn drive(
        &mut self,
        surface: &WgpuSurfaceHandle,
        view: &wgpu::TextureView,
        w: u32,
        h: u32,
    ) {
        let (stop, pending) = self.take_play_requests();
        if stop {
            self.stop_game();
        }
        let pending = self.drop_start_in_edit_mode(pending);
        let pending = self.reload_classes_in_running_game(pending);
        if let Some(req) = pending {
            self.load_game(surface, req, w, h);
        }
        self.tick_game(w, h);
        self.blit_game(surface, view);
    }

    /// Take the stop flag and the pending start from the shared play state.
    fn take_play_requests(&self) -> (bool, Option<PieStartRequest>) {
        let mut st = self.shared_state.write();
        let stop = std::mem::take(&mut st.play.pie.stop_requested);
        let pending = st.play.pie.pending_start.take();
        (stop, pending)
    }

    fn stop_game(&mut self) {
        if let Some(mut game) = self.game.take() {
            game.stop();
        }
        self.loaded = None;
        self.captured = false;
        let mut state = self.shared_state.write();
        state.play.pie.active = false;
        state.play.pie.pending_asset_updates.clear();
        state.play.pie.loaded_artifact = None;
        state.play.pie.supports_control = false;
        // The game is gone: now restore the editor world.
        crate::ui::panel::pie::finish_stop(&mut state, true);
    }

    /// A start that arrived after a Stop (the build outran it, or the
    /// stop above just restored the editor world): the scene is in edit
    /// mode, so a game loaded now would run against the live level with
    /// nothing left to restore. Drop it.
    fn drop_start_in_edit_mode(&self, pending: Option<PieStartRequest>) -> Option<PieStartRequest> {
        pending.filter(|_| {
            let in_play = self.shared_state.read().scene.is_play_mode();
            if !in_play {
                tracing::info!("PiE: dropping a start request; the scene is in edit mode");
            }
            in_play
        })
    }

    /// Play again with only script changes: keep the running game
    /// and reload every class in it; no new game.
    fn reload_classes_in_running_game(
        &self,
        pending: Option<PieStartRequest>,
    ) -> Option<PieStartRequest> {
        let Some(game) = &self.game else {
            return pending;
        };
        match pending {
            Some(req) if req.scripts_only && game.is_in_editor() == req.library.is_none() => {
                let events = crate::ui::panel::pie::class_reload_events(&req.project_root);
                tracing::info!(
                    classes = events.len(),
                    "PiE: script-only change; reloading classes in the running game"
                );
                if game.is_in_editor() {
                    // The game listens on the editor's own bus.
                    events
                        .into_iter()
                        .for_each(pulsar_events::publish_asset_updated);
                } else {
                    self.shared_state
                        .write()
                        .play
                        .pie
                        .pending_asset_updates
                        .extend(events);
                }
                None
            }
            other => other,
        }
    }

    fn load_game(&mut self, surface: &WgpuSurfaceHandle, req: PieStartRequest, w: u32, h: u32) {
        // A pending start while a game is already running replaces it. Stop
        // the old one first — its actors die with it, but the world survives
        // because it lives HERE, in the editor's shared store — then start
        // the new one against that same store. A library's RELOAD session
        // flag makes its setup re-bind actor registrations to their existing
        // entities instead of spawning duplicates.
        let was_running = self.game.is_some();
        if let Some(mut old) = self.game.take() {
            old.stop();
        }

        let started = match &req.library {
            Some(library) => self.load_library(surface, &req, library, w, h),
            None => self.start_in_editor(surface, &req, w, h),
        };
        match started {
            Ok(game) => {
                let mut st = self.shared_state.write();
                st.play.pie.active = true;
                st.play.pie.last_error = None;
                st.play.pie.supports_control = game.has_simulation_control();
                st.play.pie.paused = false;
                st.play.pie.loaded_artifact = req.library.as_ref().and_then(|library| {
                    crate::ui::panel::pie::artifact_mtime(library)
                        .map(|mtime| (library.clone(), mtime))
                });
                drop(st);
                self.game = Some(game);
                self.loaded = Some(req.clone());
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

    /// Start the game in the editor's process, on the editor's world and the
    /// viewport's GPU device.
    fn start_in_editor(
        &self,
        surface: &WgpuSurfaceHandle,
        req: &PieStartRequest,
        w: u32,
        h: u32,
    ) -> Result<RunningGame, String> {
        let start = InEditorStart {
            device: surface.device().clone(),
            queue: surface.queue().clone(),
            color_format: surface.format(),
            width: w,
            height: h,
            world: self.shared_state.read().scene.shared_scene(),
            project_root: req.project_root.clone(),
            scene_path: req.scene_path.clone(),
            level_name: req.level_name.clone(),
            view: req
                .view
                .as_ref()
                .map(|view| (view.position, view.yaw, view.pitch)),
        };
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| InEditorGame::start(start)))
            .map_err(|_| "The game panicked while starting".to_string())?
            .map(|game| RunningGame::Editor(Box::new(EditorHosted::new(game))))
    }

    /// Load the project's compiled game library.
    fn load_library(
        &self,
        surface: &WgpuSurfaceHandle,
        req: &PieStartRequest,
        library: &std::path::Path,
        w: u32,
        h: u32,
    ) -> Result<RunningGame, String> {
        // ABI v2: hand the guest THE editor's world -- one count is
        // transferred for the session; the editor keeps its own. The
        // guest adopts it, so mid-session edits and gameplay mutations
        // meet in one world.
        let shared_world = self.shared_state.read().scene.shared_scene();
        // SAFETY: `PieHost::load` requires `device`/`queue` to be valid for
        // the call and the game library to share the editor's `wgpu`
        // version. Both handles are borrowed from `surface`, which outlives
        // this call; the game library is the project's own `cdylib`, built
        // by the editor's play-in-editor build step against this workspace's
        // `wgpu`. The ABI-version check inside `load` guards the struct
        // contract. `shared_world` is an owned `SharedScene`, so the count
        // handed to the guest is ours to transfer.
        unsafe {
            PieHost::load(
                library,
                surface.device(),
                surface.queue(),
                surface.format(),
                w,
                h,
                &req.project_root,
                Some(&req.scene_path),
                shared_world,
                req.reload,
                req.view.as_ref().map(|view| {
                    engine_backend::subsystems::render::EditorCameraState {
                        position: view.position,
                        yaw: view.yaw,
                        pitch: view.pitch,
                    }
                }),
                req.level_name.as_deref(),
            )
        }
        .map(RunningGame::Library)
    }

    /// Feed the running game its queued asset updates and pause/step
    /// requests, then advance it one frame.
    fn tick_game(&mut self, w: u32, h: u32) {
        let Some(host) = self.game.as_mut() else {
            return;
        };
        // Class edits made while playing: the game reloads them
        // at its next frame, no rebuild.
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
        // Pause / step, wired to the game's TickLoop.
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
        // Quit Game ends Play, as Stop does; the next frame shuts the game
        // down and restores the editor's world.
        if host.quit_requested() {
            tracing::info!("PiE: the game quit; stopping Play");
            self.captured = false;
            crate::ui::panel::pie::end_pie(self.shared_state.clone());
        }
    }

    /// Copy the game's offscreen texture into the surface's back buffer.
    fn blit_game(&mut self, surface: &WgpuSurfaceHandle, view: &wgpu::TextureView) {
        let format = surface.format();
        if self.pie_blit.as_ref().map(|b| b.format) != Some(format) {
            self.pie_blit = Some(PieBlit::new(surface.device(), format));
        }
        if let (Some(blit), Some(game)) = (self.pie_blit.as_mut(), self.game.as_ref()) {
            // The texture is only used for this blit, while the game is
            // borrowed.
            if let Some(tex) = game.out_texture() {
                blit.blit(surface.device(), surface.queue(), tex, view);
            }
        }
    }
}
