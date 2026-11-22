//! Per-frame driving of the embedded game: honouring stop and start requests,
//! loading the game library, ticking it and blitting its output.
use std::time::Instant;

use engine_backend::services::{PieBlit, PieHost};
use gpui::WgpuSurfaceHandle;

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
        if let Some(mut host) = self.pie_host.take() {
            host.stop();
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
    /// and reload every class in it; no new library.
    fn reload_classes_in_running_game(
        &self,
        pending: Option<PieStartRequest>,
    ) -> Option<PieStartRequest> {
        match pending {
            Some(req) if req.scripts_only && self.pie_host.is_some() => {
                let events = crate::ui::panel::pie::class_reload_events(&req.project_root);
                tracing::info!(
                    classes = events.len(),
                    "PiE: script-only change; reloading classes in the running game"
                );
                self.shared_state
                    .write()
                    .play
                    .pie
                    .pending_asset_updates
                    .extend(events);
                None
            }
            other => other,
        }
    }

    fn load_game(&mut self, surface: &WgpuSurfaceHandle, req: PieStartRequest, w: u32, h: u32) {
        // Native hot reload: a pending start while a game is
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
                req.view.as_ref().map(|view| {
                    engine_backend::subsystems::render::EditorCameraState {
                        position: view.position,
                        yaw: view.yaw,
                        pitch: view.pitch,
                    }
                }),
            )
        };
        match loaded {
            Ok(host) => {
                let mut st = self.shared_state.write();
                st.play.pie.active = true;
                st.play.pie.last_error = None;
                st.play.pie.supports_control = host.has_simulation_control();
                st.play.pie.paused = false;
                st.play.pie.loaded_artifact = crate::ui::panel::pie::artifact_mtime(&req.dylib_path)
                    .map(|mtime| (req.dylib_path.clone(), mtime));
                drop(st);
                self.pie_host = Some(host);
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

    /// Feed the running game its queued asset updates and pause/step
    /// requests, then advance it one frame.
    fn tick_game(&mut self, w: u32, h: u32) {
        let Some(host) = self.pie_host.as_mut() else {
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
    }

    /// Copy the game's offscreen texture into the surface's back buffer.
    fn blit_game(&mut self, surface: &WgpuSurfaceHandle, view: &wgpu::TextureView) {
        let format = surface.format();
        if self.pie_blit.as_ref().map(|b| b.format) != Some(format) {
            self.pie_blit = Some(PieBlit::new(surface.device(), format));
        }
        if let (Some(blit), Some(host)) = (self.pie_blit.as_mut(), self.pie_host.as_ref()) {
            // SAFETY: `out_texture` borrows memory owned by the loaded game
            // library. `host` is a live `&PieHost` (the library stays mapped
            // until `stop`/drop, which need `&mut` or ownership), and `tex` is
            // only used for this blit on the render thread, so it cannot outlive
            // the host.
            if let Some(tex) = unsafe { host.out_texture() } {
                blit.blit(surface.device(), surface.queue(), tex, view);
            }
        }
    }
}
