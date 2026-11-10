//! The script phase of the driver: one frame's ordered steps (reloads,
//! reconcile, begin_play, events, tick, world commands). Entry points:
//! [`ScriptDriver::run_frame`] and `run_frame_shared`.

use pulsar_scenedb::World;

use super::world_commands::discard_commands;
use super::{DriverReport, LockTimes, ScriptDriver, MAX_COMMAND_ROUNDS};
use crate::scripting::commands::CommandScope;
use pulsar_events::builtin::BeginPlay;
use pulsar_events::entity_channel;

impl ScriptDriver {
    /// Run one script phase against `world`. See the module doc.
    pub fn run_frame(&mut self, world: &mut World, delta_time: f64) -> DriverReport {
        let mut report = DriverReport::default();
        self.frame_delta = delta_time;
        let scope = self.frame_start(world, &mut report);
        report
            .script_errors
            .extend(self.runtime.tick_all(world, delta_time));
        self.frame_finish(world, scope, &mut report);
        report
    }

    /// Run one script phase against a shared scene, holding its lock as
    /// briefly and as shared as the phase allows. Same result as
    /// [`run_frame`](Self::run_frame) except for the order the runtime
    /// ticks instances in (see `pulsar_script_runtime::phase`), and
    /// `report.locks` says how long each lock was waited for and held:
    ///
    /// 1. **Write lock**: reconcile instances with the world, `begin_play`,
    ///    queued event handlers.
    /// 2. **Read lock**: the read stage. Instances of read-only classes run
    ///    concurrently; the renderer's own readers are not blocked.
    /// 3. **Write lock**: every other instance, then the commands scripts
    ///    queued (spawns, destroys).
    pub fn run_frame_shared(
        &mut self,
        scene: &engine_backend::scene::SharedScene,
        delta_time: f64,
    ) -> DriverReport {
        use std::time::Instant;
        let mut report = DriverReport::default();
        let mut locks = LockTimes::default();
        self.frame_delta = delta_time;

        let asked = Instant::now();
        let mut store = scene.write();
        let held = Instant::now();
        locks.write_wait += held - asked;
        let scope = self.frame_start(&mut store.world, &mut report);
        self.runtime.begin_tick(delta_time);
        locks.write_hold += held.elapsed();
        drop(store);

        let asked = Instant::now();
        let store = scene.read();
        let held = Instant::now();
        locks.read_wait = held - asked;
        report
            .script_errors
            .extend(self.runtime.run_read_stage(&store.world, delta_time));
        locks.read_hold = held.elapsed();
        drop(store);

        let asked = Instant::now();
        let mut store = scene.write();
        let held = Instant::now();
        locks.write_wait += held - asked;
        report
            .script_errors
            .extend(self.runtime.run_write_stage(&mut store.world, delta_time));
        self.frame_finish(&mut store.world, scope, &mut report);
        locks.write_hold += held.elapsed();
        drop(store);

        locks.phase = self.runtime.phase_stats();
        report.locks = Some(locks);
        report
    }

    /// The first part of a frame, with exclusive access: reload native
    /// libraries, reconcile instances with the world, run `begin_play`
    /// and queued event handlers. Returns the scope that collects the
    /// commands scripts queue for [`frame_finish`](Self::frame_finish).
    pub(super) fn frame_start(
        &mut self,
        world: &mut World,
        report: &mut DriverReport,
    ) -> CommandScope {
        self.reload_changed_native_libraries();
        let scope = CommandScope::begin();
        if let Some(events) = &self.events {
            events.bridge().set_time(self.runtime.time());
        }
        self.reconcile_into(world, report);
        if !report.started.is_empty() {
            tracing::info!(
                instances = report.started.len(),
                "Starting script instances (begin_play)"
            );
        }
        report
            .script_errors
            .extend(self.runtime.dispatch_pending_begin_play(world));
        for id in &report.started {
            if let Some(entity) = self.by_instance.get(id) {
                self.publish(
                    entity_channel(entity.bits()),
                    BeginPlay {
                        entity: entity.bits(),
                    },
                );
            }
        }
        self.run_role_hooks(world, report);
        if let Some(events) = self.events.as_mut() {
            events.announce_level(&self.level);
            for failure in events.reconcile_component_subscriptions(&self.runtime, world) {
                tracing::warn!("{failure}");
                report.failures.push(failure);
            }
            report
                .script_errors
                .extend(events.run_calls(&mut self.runtime, world));
        }
        scope
    }

    /// The last part of a frame, with exclusive access: fire hub timers and
    /// apply the commands scripts queued.
    pub(super) fn frame_finish(
        &mut self,
        world: &mut World,
        mut scope: CommandScope,
        report: &mut DriverReport,
    ) {
        if let Some(events) = &self.events {
            events.bridge().fire_timers(self.runtime.time());
        }
        for _ in 0..MAX_COMMAND_ROUNDS {
            let commands = scope.take();
            if commands.is_empty() {
                break;
            }
            self.apply_commands(world, commands, report);
        }
        let left = scope.finish();
        if !left.is_empty() {
            tracing::warn!(
                dropped = left.len(),
                "Script world commands kept queueing more; dropping the rest"
            );
            discard_commands(world, left);
        }
    }
}
