//! Production orchestration from camera demand to immutable renderer output.
//!
//! The controller owns only bounded, disposable streaming state. Canonical
//! terrain remains in [`TerrainRuntimeHandle`], while Helio applies the
//! returned commands and reports generation-exact feedback.

use crate::{
    PlanetFrame, PlanetFramePayload, PlanetId, PlanetView, TerrainIncrementalResidencySession,
    TerrainPlanningConfig, TerrainPlanningError, TerrainPlanningHandle, TerrainPlanningTicket,
    TerrainRefinementConfig, TerrainRefinementError, TerrainRefinementReport, TerrainRenderDelta,
    TerrainRenderDeltaConfig, TerrainRenderDeltaError, TerrainRenderDeltaPublisher,
    TerrainRenderFeedback, TerrainRuntimeError, TerrainRuntimeEvent, TerrainRuntimeHandle,
    TerrainStreamingPlan, TerrainVisiblePageSet,
};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainControllerConfig {
    pub planning: TerrainPlanningConfig,
    pub refinement: TerrainRefinementConfig,
    pub rendering: TerrainRenderDeltaConfig,
    pub max_planets: usize,
    pub max_planning_results_per_frame: usize,
}

impl Default for TerrainControllerConfig {
    fn default() -> Self {
        Self {
            planning: TerrainPlanningConfig::default(),
            refinement: TerrainRefinementConfig::default(),
            rendering: TerrainRenderDeltaConfig::default(),
            max_planets: 8,
            max_planning_results_per_frame: 8,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainPlanningFailure {
    pub ticket: TerrainPlanningTicket,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainControllerFrame {
    pub frame_index: u64,
    pub render_delta: TerrainRenderDelta,
    pub planet_frames: Vec<PlanetFramePayload>,
    pub visible_sets: Vec<TerrainVisiblePageSet>,
    pub refinement: Vec<(PlanetId, TerrainRefinementReport)>,
    pub plans_applied: usize,
    pub planning_failures: Vec<TerrainPlanningFailure>,
}

#[derive(Debug, Error)]
pub enum TerrainControllerError {
    #[error("invalid terrain controller configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("terrain controller planet capacity {maximum} is exhausted")]
    PlanetCapacity { maximum: usize },
    #[error(transparent)]
    Planning(#[from] TerrainPlanningError),
    #[error(transparent)]
    Refinement(#[from] TerrainRefinementError),
    #[error(transparent)]
    Rendering(#[from] TerrainRenderDeltaError),
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
}

struct PlanetControllerState {
    planet_generation: u64,
    latest_plan_submission: u64,
    latest_camera: crate::PlanetPosition,
    target: Option<TerrainStreamingPlan>,
    residency: TerrainIncrementalResidencySession,
}

/// Bounded owner of the live planet streaming protocol.
pub struct TerrainStreamingController {
    runtime: TerrainRuntimeHandle,
    planning: TerrainPlanningHandle,
    config: TerrainControllerConfig,
    planets: BTreeMap<PlanetId, PlanetControllerState>,
    publisher: TerrainRenderDeltaPublisher,
}

impl TerrainStreamingController {
    pub fn new(
        runtime: TerrainRuntimeHandle,
        planning: TerrainPlanningHandle,
        config: TerrainControllerConfig,
    ) -> Result<Self, TerrainControllerError> {
        if config.max_planets == 0 || config.max_planning_results_per_frame == 0 {
            return Err(TerrainControllerError::InvalidConfig(
                "planet and planning-result limits must be non-zero",
            ));
        }
        config.planning.validate()?;
        let _ = TerrainIncrementalResidencySession::new(PlanetId([0; 16]), config.refinement)?;
        let publisher = TerrainRenderDeltaPublisher::new(config.rendering)?;
        if config.planning.streaming.max_pages > config.refinement.max_active_pages {
            return Err(TerrainControllerError::InvalidConfig(
                "planning page budget must fit the active refinement frontier",
            ));
        }
        if config.refinement.max_active_pages > config.rendering.max_visible_pages {
            return Err(TerrainControllerError::InvalidConfig(
                "active refinement frontier must fit the published surface frontier",
            ));
        }
        if config.refinement.max_transition_pages > config.rendering.max_tracked_pages {
            return Err(TerrainControllerError::InvalidConfig(
                "transition frontier must fit the renderer-tracked page budget",
            ));
        }
        Ok(Self {
            runtime,
            planning,
            config,
            planets: BTreeMap::new(),
            publisher,
        })
    }

    pub const fn config(&self) -> TerrainControllerConfig {
        self.config
    }

    pub fn planet_count(&self) -> usize {
        self.planets.len()
    }

    pub fn submit_view(
        &mut self,
        planet_id: PlanetId,
        view: PlanetView,
    ) -> Result<TerrainPlanningTicket, TerrainControllerError> {
        if !self.planets.contains_key(&planet_id) && self.planets.len() == self.config.max_planets {
            return Err(TerrainControllerError::PlanetCapacity {
                maximum: self.config.max_planets,
            });
        }
        let ticket = self
            .planning
            .submit(planet_id, view, self.config.planning)?;
        let replace = self
            .planets
            .get(&planet_id)
            .is_none_or(|state| state.planet_generation != ticket.planet_generation);
        if replace {
            self.planets.insert(
                planet_id,
                PlanetControllerState {
                    planet_generation: ticket.planet_generation,
                    latest_plan_submission: 0,
                    latest_camera: view.camera(),
                    target: None,
                    residency: TerrainIncrementalResidencySession::new(
                        planet_id,
                        self.config.refinement,
                    )?,
                },
            );
        } else if let Some(state) = self.planets.get_mut(&planet_id) {
            state.latest_camera = view.camera();
        }
        Ok(ticket)
    }

    pub fn remove_planet(&mut self, planet_id: PlanetId) -> bool {
        self.planets.remove(&planet_id).is_some()
    }

    pub fn is_converged(&self, planet_id: PlanetId) -> Option<bool> {
        self.planets
            .get(&planet_id)
            .map(|state| state.target.is_some() && state.residency.is_converged())
    }

    pub fn acknowledge_render_feedback(
        &mut self,
        feedback: &TerrainRenderFeedback,
    ) -> Result<(), TerrainControllerError> {
        self.publisher.acknowledge_render_feedback(feedback)?;
        // A renderer-local cache eviction never changes canonical residency.
        // Requeue any evicted page that still owns part of the committed
        // frontier so the disposable cache repairs itself on the next frame.
        for eviction in &feedback.cache_evictions {
            let Some(state) = self.planets.get(&eviction.planet_id) else {
                continue;
            };
            if !state
                .residency
                .protected_pages()
                .any(|page| page == eviction.page_key)
            {
                continue;
            }
            let Some(generation) = self
                .runtime
                .resident_page_generation(eviction.planet_id, eviction.page_key)
            else {
                continue;
            };
            self.publisher.ensure_resident_upload(
                eviction.planet_id,
                eviction.page_key,
                generation,
            )?;
        }
        Ok(())
    }

    /// Pages the controller believes are currently present in Helio's
    /// disposable residency cache. A non-zero value paired with an empty live
    /// cache means Helio rebuilt its graph and the canonical frontier must be
    /// republished.
    pub fn published_page_count(&self) -> usize {
        self.publisher.tracked_page_count()
    }

    /// Forget every renderer-local publication after Helio recreates its
    /// graph (for example on resize or device recovery). Canonical pages and
    /// the committed refinement frontier remain untouched; the next frame
    /// republishes the bounded committed set into the new disposable cache.
    pub fn invalidate_renderer_cache(&mut self) -> Result<(), TerrainControllerError> {
        self.publisher = TerrainRenderDeltaPublisher::new(self.config.rendering)?;
        for state in self.planets.values() {
            let resident = self
                .runtime
                .resident_page_generations(state.residency.planet_id())?;
            for page_key in state.residency.protected_pages() {
                if let Some(generation) = resident.get(&page_key) {
                    self.publisher.ensure_resident_upload(
                        state.residency.planet_id(),
                        page_key,
                        *generation,
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Advance bounded orchestration for one frame. Runtime event draining
    /// stays explicit so persistence, collision, replication, and tooling can
    /// observe the same event stream.
    pub fn process_frame(
        &mut self,
        events: &[TerrainRuntimeEvent],
        tick: u64,
        frame_index: u64,
    ) -> Result<TerrainControllerFrame, TerrainControllerError> {
        self.remove_retired_sessions(events);
        let mut frame = TerrainControllerFrame {
            frame_index,
            ..TerrainControllerFrame::default()
        };
        for result in self
            .planning
            .drain_completed(self.config.max_planning_results_per_frame)
        {
            let ticket = result.ticket();
            let Some(state) = self.planets.get_mut(&ticket.planet_id) else {
                continue;
            };
            if state.planet_generation != ticket.planet_generation
                || ticket.submission < state.latest_plan_submission
            {
                continue;
            }
            match result.into_plan() {
                Ok(plan) => {
                    state.latest_plan_submission = ticket.submission;
                    state.target = Some(plan);
                    frame.plans_applied += 1;
                }
                Err(error) => frame.planning_failures.push(TerrainPlanningFailure {
                    ticket,
                    message: error.to_string(),
                }),
            }
        }

        for (planet_id, state) in &mut self.planets {
            let Some(target) = state.target.as_ref() else {
                continue;
            };
            let report =
                state
                    .residency
                    .reconcile(&self.runtime, &mut self.publisher, target, tick)?;
            frame.refinement.push((*planet_id, report));
        }

        frame.render_delta = self.publisher.translate_events(&self.runtime, events)?;
        for state in self.planets.values() {
            if state.residency.committed_pages().len() == 0 {
                continue;
            }
            frame.planet_frames.push(
                PlanetFrame::new(
                    state.residency.planet_id(),
                    state.latest_camera,
                    frame_index,
                )
                .renderer_payload(),
            );
            let resident = self
                .runtime
                .resident_page_generations(state.residency.planet_id())?;
            let published = self
                .publisher
                .published_resident_pages(state.residency.planet_id(), &resident);
            if state
                .residency
                .protected_pages()
                .all(|page| published.contains(&page))
            {
                frame.visible_sets.push(self.publisher.visible_set(
                    &self.runtime,
                    &state.residency,
                    frame_index,
                )?);
            }
        }
        Ok(frame)
    }

    fn remove_retired_sessions(&mut self, events: &[TerrainRuntimeEvent]) {
        for event in events {
            let TerrainRuntimeEvent::EvictPlanet {
                planet_id,
                retired_generation,
            } = event
            else {
                continue;
            };
            let still_current = self
                .runtime
                .planet_generation(*planet_id)
                .is_some_and(|generation| generation > *retired_generation);
            if !still_current {
                self.planets.remove(planet_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PlanetDefinition, PlanetPosition, TerrainRenderCommandDisposition,
        TerrainRenderCommandFeedback, TerrainRuntimeConfig, TerrainStreamingConfig,
        TerrainSubsystem,
    };
    use engine_subsystems::{Subsystem, SubsystemContext};
    use std::time::{Duration, Instant};

    #[test]
    fn asynchronous_plan_converges_through_renderer_feedback_without_losing_coverage() {
        let mut subsystem = TerrainSubsystem::new(TerrainRuntimeConfig {
            worker_count: 2,
            max_planets: 2,
            max_component_sources: 2,
            request_capacity: 128,
            critical_request_reserve: 16,
            completion_capacity: 128,
            event_capacity: 256,
            max_resident_pages: 256,
            max_resident_dense_bytes: 256 * crate::CELL_COUNT * 4,
            max_completions_per_frame: 64,
        })
        .unwrap();
        subsystem.init(&SubsystemContext::new()).unwrap();
        let runtime = subsystem.runtime_handle();
        let planet = PlanetDefinition {
            planet_id: PlanetId([31; 16]),
            center_cell: [0; 3],
            radius_cells: 200,
            material: 3,
            root_lod: 6,
            max_resident_pages: 256,
        };
        runtime.upsert_planet(planet.clone()).unwrap();
        let controller_config = TerrainControllerConfig {
            planning: TerrainPlanningConfig {
                streaming: TerrainStreamingConfig {
                    max_pages: 64,
                    max_traversal_nodes: 4_096,
                    ..TerrainStreamingConfig::default()
                },
                ..TerrainPlanningConfig::default()
            },
            refinement: TerrainRefinementConfig {
                max_active_pages: 64,
                max_transition_pages: 256,
                initial_coarse_pages: 8,
                max_requests_per_reconcile: 32,
                max_commits_per_reconcile: 8,
                ..TerrainRefinementConfig::default()
            },
            rendering: TerrainRenderDeltaConfig {
                max_events_per_delta: 256,
                max_commands_per_delta: 128,
                max_upload_bytes_per_delta: 128 * crate::CELL_COUNT * 4,
                max_tracked_pages: 256,
                max_visible_pages: 64,
            },
            max_planets: 2,
            max_planning_results_per_frame: 2,
        };
        for invalid in [
            TerrainControllerConfig {
                planning: TerrainPlanningConfig {
                    streaming: TerrainStreamingConfig {
                        max_pages: 65,
                        ..controller_config.planning.streaming
                    },
                    ..controller_config.planning
                },
                ..controller_config
            },
            TerrainControllerConfig {
                rendering: TerrainRenderDeltaConfig {
                    max_visible_pages: 63,
                    ..controller_config.rendering
                },
                ..controller_config
            },
            TerrainControllerConfig {
                rendering: TerrainRenderDeltaConfig {
                    max_tracked_pages: 79,
                    ..controller_config.rendering
                },
                ..controller_config
            },
        ] {
            assert!(matches!(
                TerrainStreamingController::new(
                    runtime.clone(),
                    subsystem.planning_handle(),
                    invalid,
                ),
                Err(TerrainControllerError::InvalidConfig(_))
            ));
        }
        let mut controller = TerrainStreamingController::new(
            runtime.clone(),
            subsystem.planning_handle(),
            controller_config,
        )
        .unwrap();
        let view = PlanetView::new(
            PlanetPosition::from_lod0_cell([2_500, 0, 0]),
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60_f64.to_radians(),
            [1920, 1080],
            0.1,
            10_000.0,
            [0.0; 3],
        )
        .unwrap();
        controller.submit_view(planet.planet_id, view).unwrap();

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut had_coverage = false;
        let mut frame_index = 0;
        loop {
            runtime.pump(64);
            let events = runtime.drain_events(256);
            let frame = controller
                .process_frame(&events, frame_index, frame_index)
                .unwrap();
            assert!(frame.planning_failures.is_empty());
            if let Some(visible) = frame.visible_sets.first() {
                assert!(!visible.pages.is_empty());
                had_coverage = true;
            } else {
                assert!(!had_coverage, "committed coverage regressed to empty");
            }
            let feedback = TerrainRenderFeedback {
                commands: frame
                    .render_delta
                    .commands
                    .iter()
                    .map(|command| TerrainRenderCommandFeedback {
                        command: command.id(),
                        disposition: TerrainRenderCommandDisposition::Applied,
                    })
                    .collect(),
                cache_evictions: Vec::new(),
            };
            controller.acknowledge_render_feedback(&feedback).unwrap();
            if had_coverage && controller.is_converged(planet.planet_id) == Some(true) {
                break;
            }
            assert!(Instant::now() < deadline, "controller did not converge");
            frame_index += 1;
            std::thread::yield_now();
        }
        assert!(had_coverage);
        assert!(controller.planet_count() <= controller.config().max_planets);
        assert!(controller.published_page_count() > 0);

        controller.invalidate_renderer_cache().unwrap();
        assert_eq!(controller.published_page_count(), 0);
        frame_index += 1;
        let rebuilding = controller
            .process_frame(&[], frame_index, frame_index)
            .unwrap();
        assert_eq!(rebuilding.planet_frames.len(), 1);
        assert!(rebuilding.visible_sets.is_empty());
        assert!(!rebuilding.render_delta.commands.is_empty());
        controller
            .acknowledge_render_feedback(&TerrainRenderFeedback {
                commands: rebuilding
                    .render_delta
                    .commands
                    .iter()
                    .map(|command| TerrainRenderCommandFeedback {
                        command: command.id(),
                        disposition: TerrainRenderCommandDisposition::Applied,
                    })
                    .collect(),
                cache_evictions: Vec::new(),
            })
            .unwrap();
        assert!(controller.published_page_count() > 0);
        let restored = loop {
            frame_index += 1;
            let restored = controller
                .process_frame(&[], frame_index, frame_index)
                .unwrap();
            controller
                .acknowledge_render_feedback(&TerrainRenderFeedback {
                    commands: restored
                        .render_delta
                        .commands
                        .iter()
                        .map(|command| TerrainRenderCommandFeedback {
                            command: command.id(),
                            disposition: TerrainRenderCommandDisposition::Applied,
                        })
                        .collect(),
                    cache_evictions: Vec::new(),
                })
                .unwrap();
            if !restored.visible_sets.is_empty() {
                break restored;
            }
            assert!(
                Instant::now() < deadline,
                "renderer cache did not republish"
            );
        };
        assert!(!restored.visible_sets[0].pages.is_empty());
        subsystem.shutdown().unwrap();
    }
}
