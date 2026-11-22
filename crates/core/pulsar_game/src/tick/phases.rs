//! The phases of one [`TickLoop`] tick, in the order `run_tick` runs them.
//!
//! Each takes the store's lock for its own short stretch and drops it before
//! returning, per the locking protocol on [`TickLoop`].

use super::{TickLoop, MAX_KEPT_PROBLEMS};
use pulsar_core::GameTime;
use std::sync::Arc;

impl TickLoop {
    /// Starts watching for moves of fixed objects, once per loop.
    pub(super) fn ensure_static_move_watch(&mut self) {
        if self.static_moves.is_none() {
            let store = self.scene_store.read();
            self.static_moves = Some(helio_component::StaticMoveWatch::new(&store.world));
        }
    }

    /// Flush 1 (after input): input published since the last tick
    /// (`publish_input`, PIE input forwarding) and anything else queued
    /// between ticks.
    pub(super) fn flush_after_input(&self) {
        profiling::profile_scope!("TickLoop::flush_after_input");
        self.events.flush(pulsar_events::FlushPoint::AfterInput);
    }

    /// Animation. Every live skeletal mesh component advances its
    /// animation instance and publishes its pose and GPU palettes *before* the
    /// schedule, so physics (which runs inside it) reads this frame's pose.
    /// Parameters and Montage commands written by scripts later in the frame
    /// apply at the next frame's animation phase.
    pub(super) fn run_before_systems(&self, time: GameTime) {
        profiling::profile_scope!("TickLoop::before_systems");
        let mut store = self.scene_store.write();
        for phase in engine_backend::scene::game_hooks::tick_phases() {
            if let Some(before_systems) = phase.before_systems {
                before_systems(&mut store.world, time.delta.as_secs_f32());
            }
        }
    }

    /// ECS systems. Short write scope -- the renderer takes this
    /// same lock every frame to step the scene and flush its GPU mirror, so
    /// nothing here may hold it across phases.
    pub(super) fn run_schedule(&mut self, time: pulsar_scenedb::GameTime) {
        profiling::profile_scope!("TickLoop::schedule");
        let mut store = self.scene_store.write();
        self.schedule.run(&mut store.world, time);
    }

    /// Macro-registered native component callbacks. The
    /// registry resolves each component through a typed SceneDB query and
    /// calls it while borrowing the authoritative row in place. This is
    /// deliberately separate from renderer synchronization and owns no
    /// component copies. The callback contract forbids re-entering World
    /// while its component borrow is held.
    pub(super) fn run_component_phase(&mut self, time: GameTime) {
        profiling::profile_scope!("TickLoop::component_phase");
        let scene_identity = Arc::as_ptr(&self.scene_store) as usize;
        self.ensure_actor_bank();
        if let Some(inbox) = &self.class_inbox {
            if self
                .component_runtime
                .scope()
                .resource::<pulsar_class::ClassInbox>()
                .is_none()
            {
                self.component_runtime
                    .scope_mut()
                    .insert_resource(inbox.clone());
            }
        }
        let mut store = self.scene_store.write();
        pulsar_world_registry::tick_live_components(
            &mut store.world,
            &self.events,
            time.delta.as_secs_f32(),
            scene_identity,
            &mut self.component_runtime,
        );
    }

    /// Native script actors tick through the component
    /// lifecycle (`crate::lifecycle`), as every component does. This phase is
    /// what remains of the old actor registry's: flush what the actors and
    /// components queued. Separate lock acquisition from the ECS systems on purpose.
    pub(super) fn flush_component_events(&mut self) {
        profiling::profile_scope!("TickLoop::flush_component_events");
        let mut store = self.scene_store.write();
        // World-mutating component methods append to per-owner outboxes.
        // Move them onto Gamma's deferred queue only after actors have
        // released their component/World borrows; the following flush
        // invokes subscribers outside the World lock.
        pulsar_world_registry::flush_component_events(&mut store.world, &self.events);
    }

    /// Ends the lifecycle of the components removed or disabled since the
    /// last drain, before SceneDB's end-of-window cleanup drains the
    /// independent removal journal.
    pub(super) fn process_component_removals(&mut self) {
        let mut store = self.scene_store.write();
        pulsar_world_registry::process_component_removals_mut(
            &mut store.world,
            &self.events,
            &mut self.component_runtime,
        );
    }

    /// Flush 2 (after the systems): the component systems (a physics
    /// plugin among them) run in the ECS schedule; their
    /// hits and overlaps (and whatever systems and actors published)
    /// are delivered before the script phase, which runs the script
    /// handlers they queued.
    pub(super) fn flush_after_systems(&self) {
        profiling::profile_scope!("TickLoop::flush_after_systems");
        self.events.flush(pulsar_events::FlushPoint::AfterSystems);
    }

    /// The script phase, AFTER ECS + actor updates: the driver
    /// starts/stops script instances to match the world's class
    /// instances, runs `begin_play` for the ones it started, `tick` for
    /// all, then applies the spawns/destroys scripts queued. Deferring
    /// `begin_play` to here (rather than firing it at registration during
    /// level setup) means it observes a fully initialised window/world/
    /// scene: `tick_once` only runs after `spawn_ecs_thread`, once the
    /// window is ready. Errors are per instance and logged.
    pub(super) fn run_script_phase(&mut self, time: GameTime) {
        if let Some(driver) = &self.scripts {
            profiling::profile_scope!("TickLoop::script_phase");
            let mut driver = driver
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // Locks the scene itself, in short exclusive stretches around a
            // shared one (see `ScriptDriver::run_frame_shared`).
            let report = driver.run_frame_shared(&self.scene_store, time.delta.as_secs_f64());
            self.script_stats.absorb(&report);
            if self.collect_problems && report.has_problems() {
                for mut problem in driver.problems(&report) {
                    problem.frame = Some(time.tick);
                    self.script_problems.push(problem);
                }
                let excess = self.script_problems.len().saturating_sub(MAX_KEPT_PROBLEMS);
                self.script_problems.drain(..excess);
            }
        }
    }

    /// Animation notifies the animation phase collected are published now that
    /// scripts have released the world. A notify aimed at a class variable
    /// resolves it from the owner's current script instance.
    pub(super) fn publish_animation_notifies(&self) {
        profiling::profile_scope!("TickLoop::after_scripts");
        let mut store = self.scene_store.write();
        let driver = self
            .scripts
            .as_ref()
            .map(|d| d.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
        let resolve = |owner, variable: &str| {
            driver
                .as_ref()
                .and_then(|d| d.entity_variable(owner, variable))
        };
        for phase in engine_backend::scene::game_hooks::tick_phases() {
            if let Some(after_scripts) = phase.after_scripts {
                after_scripts(&mut store.world, &self.events, &resolve);
            }
        }
    }

    /// Flush 3 (after scripts): what scripts sent (`event::*`,
    /// BeginPlay, LevelLoaded, spawns...). Neither the world nor the
    /// driver is locked, so host subscribers may use both; script
    /// handlers are queued for the next script phase. Flush 4 (end of
    /// frame) delivers anything published after that, e.g. by host
    /// handlers of flush 3 on another thread.
    pub(super) fn flush_after_scripts(&self) {
        profiling::profile_scope!("TickLoop::flush_after_scripts");
        self.events.flush(pulsar_events::FlushPoint::AfterScripts);
        self.events.flush(pulsar_events::FlushPoint::EndOfFrame);
    }

    /// Assets this tick referenced (a spawned actor's mesh) start loading,
    /// and the finished ones bind, also in a headless run with no render
    /// loop to do it. An already-loaded mesh binds here.
    pub(super) fn pump_assets(&self) {
        profiling::profile_scope!("TickLoop::pump_assets");
        engine_backend::scene::pump_assets(&self.scene_store);
    }

    /// The world's change history only covers the current frame, so the
    /// frame's moves are checked before it closes. Anything this tick
    /// moved that is authored fixed is a bug (it is logged once).
    pub(super) fn close_change_window(&mut self) {
        let store = self.scene_store.read();
        if let Some(watch) = &mut self.static_moves {
            watch.poll(&store.world);
        }
        engine_backend::scene::end_change_window(&store.world);
    }
}
