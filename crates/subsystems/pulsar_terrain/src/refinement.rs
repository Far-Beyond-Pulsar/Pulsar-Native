use crate::{
    sampling::terrain_frontier_sampling_support, PageDemand, PageKey, PlanetId,
    TerrainRenderDeltaError, TerrainRenderDeltaPublisher, TerrainRequestClass,
    TerrainRequestOutcome, TerrainRuntimeError, TerrainRuntimeHandle, TerrainStreamingPlan,
    TerrainSurfaceSamplingError,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Limits for incremental, parent-preserving terrain refinement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainRefinementConfig {
    /// Maximum stable visible/prefetch frontier.
    pub max_active_pages: usize,
    /// Maximum union of committed surfaces, their extraction support, and the
    /// next staged handoff. This bounds canonical and renderer residency while
    /// a parent-preserving replacement is prepared.
    pub max_transition_pages: usize,
    /// Maximum pages in the first coarse publication.
    pub initial_coarse_pages: usize,
    /// Maximum page requests submitted by one reconcile call.
    pub max_requests_per_reconcile: usize,
    /// Maximum already-resident replacements committed by one reconcile call.
    pub max_commits_per_reconcile: usize,
    pub visible_deadline_ticks: u64,
    pub prefetch_deadline_ticks: u64,
}

impl Default for TerrainRefinementConfig {
    fn default() -> Self {
        Self {
            max_active_pages: 2_048,
            max_transition_pages: 8_192,
            initial_coarse_pages: 32,
            max_requests_per_reconcile: 8,
            max_commits_per_reconcile: 4,
            visible_deadline_ticks: 1,
            prefetch_deadline_ticks: 8,
        }
    }
}

impl TerrainRefinementConfig {
    fn validate(self) -> Result<Self, TerrainRefinementError> {
        if self.max_active_pages == 0 {
            return Err(TerrainRefinementError::InvalidConfig(
                "max_active_pages must be non-zero",
            ));
        }
        if self.max_transition_pages < self.max_active_pages {
            return Err(TerrainRefinementError::InvalidConfig(
                "max_transition_pages must cover max_active_pages",
            ));
        }
        if self.initial_coarse_pages == 0 || self.initial_coarse_pages > self.max_active_pages {
            return Err(TerrainRefinementError::InvalidConfig(
                "initial_coarse_pages must be in 1..=max_active_pages",
            ));
        }
        if self.max_requests_per_reconcile == 0 {
            return Err(TerrainRefinementError::InvalidConfig(
                "max_requests_per_reconcile must be non-zero",
            ));
        }
        if self.max_commits_per_reconcile == 0 {
            return Err(TerrainRefinementError::InvalidConfig(
                "max_commits_per_reconcile must be non-zero",
            ));
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainRefinementCounters {
    pub targets_seen: u64,
    pub targets_superseded: u64,
    pub stages_cancelled: u64,
    pub replacements_committed: u64,
    pub requests_queued: u64,
    pub requests_coalesced: u64,
    pub pages_current: u64,
    pub requests_deferred: u64,
    pub pages_evicted: u64,
    pub active_page_high_water: usize,
    pub transition_page_high_water: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerrainRefinementReport {
    pub submitted: usize,
    pub queued: usize,
    pub coalesced: usize,
    pub current: usize,
    pub deferred: usize,
    pub evicted: usize,
    pub committed_pages: usize,
    pub committed_sampling_pages: usize,
    pub staged_pages: usize,
    pub staged_sampling_pages: usize,
    pub target_pages: usize,
    pub replacements_committed: usize,
    pub visible_set_changed: bool,
    pub converged: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct TargetFrontier {
    plan: TerrainStreamingPlan,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StagedReplacement {
    additions: BTreeMap<PageKey, TerrainRequestClass>,
    removals: BTreeSet<PageKey>,
    sampling_support: BTreeSet<PageKey>,
}

/// Persistent canonical LOD frontier. Staged pages are never part of
/// `committed`, so a renderer cannot draw a parent and its replacement at the
/// same time.
pub struct TerrainRefinementFrontier {
    planet_id: PlanetId,
    config: TerrainRefinementConfig,
    committed: BTreeMap<PageKey, TerrainRequestClass>,
    sampling_support: BTreeSet<PageKey>,
    target: Option<TargetFrontier>,
    staged: Option<StagedReplacement>,
    counters: TerrainRefinementCounters,
}

impl TerrainRefinementFrontier {
    pub fn new(
        planet_id: PlanetId,
        config: TerrainRefinementConfig,
    ) -> Result<Self, TerrainRefinementError> {
        Ok(Self {
            planet_id,
            config: config.validate()?,
            committed: BTreeMap::new(),
            sampling_support: BTreeSet::new(),
            target: None,
            staged: None,
            counters: TerrainRefinementCounters::default(),
        })
    }

    pub const fn planet_id(&self) -> PlanetId {
        self.planet_id
    }

    pub const fn config(&self) -> TerrainRefinementConfig {
        self.config
    }

    pub const fn counters(&self) -> TerrainRefinementCounters {
        self.counters
    }

    pub fn committed_pages(&self) -> impl ExactSizeIterator<Item = PageKey> + '_ {
        self.committed.keys().copied()
    }

    pub fn committed_demands(
        &self,
    ) -> impl ExactSizeIterator<Item = (PageKey, TerrainRequestClass)> + '_ {
        self.committed.iter().map(|(key, class)| (*key, *class))
    }

    pub fn sampling_support_pages(&self) -> impl ExactSizeIterator<Item = PageKey> + '_ {
        self.sampling_support.iter().copied()
    }

    pub fn protected_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.committed
            .keys()
            .copied()
            .chain(self.sampling_support.iter().copied())
    }

    pub fn staged_surface_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.staged
            .as_ref()
            .into_iter()
            .flat_map(|stage| stage.additions.keys().copied())
    }

    pub fn staged_sampling_support_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.staged
            .as_ref()
            .into_iter()
            .flat_map(|stage| stage.sampling_support.iter().copied())
    }

    pub fn staged_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.staged_surface_pages()
            .chain(self.staged_sampling_support_pages())
    }

    pub fn is_converged(&self) -> bool {
        self.staged.is_none()
            && self.target.as_ref().is_some_and(|target| {
                target.plan.residency_identity().len() == self.committed.len()
                    && target
                        .plan
                        .residency_identity()
                        .iter()
                        .all(|(key, request_class)| self.committed.get(key) == Some(request_class))
            })
    }

    /// Replace the desired canonical frontier. Returns staged pages that are
    /// no longer protected by a parent-preserving replacement and may be
    /// evicted from the resident cache.
    pub fn set_target(
        &mut self,
        plan: &TerrainStreamingPlan,
    ) -> Result<Vec<PageKey>, TerrainRefinementError> {
        if plan.planet_id() != self.planet_id {
            return Err(TerrainRefinementError::PlanetMismatch {
                session: self.planet_id,
                plan: plan.planet_id(),
            });
        }
        if plan.demands().len() > self.config.max_active_pages {
            return Err(TerrainRefinementError::ActivePageBudget {
                pages: plan.demands().len(),
                capacity: self.config.max_active_pages,
            });
        }
        if !plan.is_face_balanced() {
            return Err(TerrainRefinementError::UnbalancedTarget);
        }
        if !plan.is_non_overlapping() {
            let pages = plan
                .residency_identity()
                .iter()
                .map(|(page_key, _)| *page_key);
            validate_non_overlapping(pages)?;
            return Err(TerrainRefinementError::InvalidTargetInvariant(
                "precomputed overlap validation disagrees with the page set",
            ));
        }
        if let Some(current) = self.target.as_mut() {
            if current.plan.has_same_residency(plan) {
                // The topology and request classes are unchanged. Replace the
                // immutable metrics snapshot without rebuilding validation
                // maps or disturbing a reusable staged replacement.
                current.plan = plan.clone();
                return Ok(Vec::new());
            }
        }
        let next = TargetFrontier { plan: plan.clone() };

        self.counters.targets_seen = self.counters.targets_seen.saturating_add(1);
        if self.target.is_some() {
            self.counters.targets_superseded = self.counters.targets_superseded.saturating_add(1);
        }
        let cancelled = self.staged.take().map_or_else(Vec::new, |mut stage| {
            retarget_stage_classes(&mut stage, &next);
            if stage_advances_target(&self.committed, &stage, &next) {
                self.staged = Some(stage);
                Vec::new()
            } else {
                self.counters.stages_cancelled = self.counters.stages_cancelled.saturating_add(1);
                stage
                    .additions
                    .into_keys()
                    .chain(stage.sampling_support)
                    .filter(|key| {
                        !self.committed.contains_key(key) && !self.sampling_support.contains(key)
                    })
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            }
        });
        self.target = Some(next);
        Ok(cancelled)
    }

    fn prepare_stage(&mut self) -> Result<bool, TerrainRefinementError> {
        if self.staged.is_some() || self.is_converged() {
            return Ok(false);
        }
        let target = self
            .target
            .as_ref()
            .ok_or(TerrainRefinementError::MissingTarget)?;
        let mut stage = if self.committed.is_empty() {
            StagedReplacement {
                additions: coarse_seed(target, self.config.initial_coarse_pages)?,
                removals: BTreeSet::new(),
                sampling_support: BTreeSet::new(),
            }
        } else {
            choose_replacement(&self.committed, target)?
                .ok_or(TerrainRefinementError::NoSafeReplacement)?
        };
        stage.sampling_support = sampling_support_after(&self.committed, &stage)?;
        let transition_pages = self
            .committed
            .keys()
            .chain(self.sampling_support.iter())
            .chain(stage.additions.keys())
            .chain(stage.sampling_support.iter())
            .copied()
            .collect::<BTreeSet<_>>()
            .len();
        if transition_pages > self.config.max_transition_pages {
            return Err(TerrainRefinementError::TransitionPageBudget {
                pages: transition_pages,
                capacity: self.config.max_transition_pages,
            });
        }
        self.counters.transition_page_high_water = self
            .counters
            .transition_page_high_water
            .max(transition_pages);
        self.staged = Some(stage);
        Ok(true)
    }

    fn commit_ready_stages(
        &mut self,
        resident: &BTreeSet<PageKey>,
    ) -> Result<(Vec<PageKey>, usize), TerrainRefinementError> {
        let mut retired = BTreeSet::new();
        let mut committed = 0;
        while committed < self.config.max_commits_per_reconcile {
            self.prepare_stage()?;
            let Some(stage) = self.staged.as_ref() else {
                break;
            };
            if !stage
                .additions
                .keys()
                .chain(stage.sampling_support.iter())
                .all(|key| resident.contains(key))
            {
                break;
            }
            let stage = self.staged.take().expect("the ready stage exists");
            let mut prospective = self.committed.clone();
            for key in &stage.removals {
                prospective.remove(key);
            }
            prospective.extend(stage.additions.iter().map(|(key, class)| (*key, *class)));
            validate_non_overlapping(prospective.keys().copied())?;
            if !is_face_balanced(prospective.keys().copied()) {
                return Err(TerrainRefinementError::UnbalancedCommit);
            }
            let retained = prospective
                .keys()
                .copied()
                .chain(stage.sampling_support.iter().copied())
                .collect::<BTreeSet<_>>();
            retired.extend(
                stage
                    .removals
                    .iter()
                    .copied()
                    .chain(self.sampling_support.iter().copied())
                    .filter(|key| !retained.contains(key)),
            );
            self.committed = prospective;
            self.sampling_support = stage.sampling_support;
            committed += 1;
            self.counters.replacements_committed =
                self.counters.replacements_committed.saturating_add(1);
            self.counters.active_page_high_water = self
                .counters
                .active_page_high_water
                .max(self.committed.len());
            if self.is_converged() {
                break;
            }
        }
        Ok((retired.into_iter().collect(), committed))
    }
}

/// Runtime wrapper around [`TerrainRefinementFrontier`]. Expensive target-plan
/// construction is intentionally outside this type; reconcile performs only
/// bounded requests and ready-page commits.
pub struct TerrainIncrementalResidencySession {
    frontier: TerrainRefinementFrontier,
}

impl TerrainIncrementalResidencySession {
    pub fn new(
        planet_id: PlanetId,
        config: TerrainRefinementConfig,
    ) -> Result<Self, TerrainRefinementError> {
        Ok(Self {
            frontier: TerrainRefinementFrontier::new(planet_id, config)?,
        })
    }

    pub const fn planet_id(&self) -> PlanetId {
        self.frontier.planet_id()
    }

    pub const fn config(&self) -> TerrainRefinementConfig {
        self.frontier.config()
    }

    pub const fn counters(&self) -> TerrainRefinementCounters {
        self.frontier.counters()
    }

    pub fn committed_pages(&self) -> impl ExactSizeIterator<Item = PageKey> + '_ {
        self.frontier.committed_pages()
    }

    pub fn committed_demands(
        &self,
    ) -> impl ExactSizeIterator<Item = (PageKey, TerrainRequestClass)> + '_ {
        self.frontier.committed_demands()
    }

    pub fn sampling_support_pages(&self) -> impl ExactSizeIterator<Item = PageKey> + '_ {
        self.frontier.sampling_support_pages()
    }

    pub fn protected_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.frontier.protected_pages()
    }

    pub fn staged_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.frontier.staged_pages()
    }

    pub fn staged_surface_pages(&self) -> impl Iterator<Item = PageKey> + '_ {
        self.frontier.staged_surface_pages()
    }

    pub fn is_converged(&self) -> bool {
        self.frontier.is_converged()
    }

    pub fn reconcile(
        &mut self,
        runtime: &TerrainRuntimeHandle,
        publisher: &mut TerrainRenderDeltaPublisher,
        plan: &TerrainStreamingPlan,
        tick: u64,
    ) -> Result<TerrainRefinementReport, TerrainRefinementError> {
        let cancelled = self.frontier.set_target(plan)?;
        let mut report = TerrainRefinementReport {
            target_pages: plan.demands().len(),
            ..TerrainRefinementReport::default()
        };
        if !cancelled.is_empty() {
            report.evicted += runtime.evict_pages(self.planet_id(), &cancelled)?;
        }

        let resident = runtime.resident_page_generations(self.planet_id())?;
        let published = publisher.published_resident_pages(self.planet_id(), &resident);
        let (retired, replacements_committed) = self.frontier.commit_ready_stages(&published)?;
        report.replacements_committed = replacements_committed;
        report.visible_set_changed = replacements_committed != 0;
        if !retired.is_empty() {
            report.evicted += runtime.evict_pages(self.planet_id(), &retired)?;
        }

        self.frontier.prepare_stage()?;
        let resident = runtime.resident_page_generations(self.planet_id())?;
        let published = publisher.published_resident_pages(self.planet_id(), &resident);
        let staged_work = self
            .frontier
            .staged
            .as_ref()
            .into_iter()
            .flat_map(|stage| {
                stage
                    .additions
                    .iter()
                    .map(|(key, class)| (*key, *class))
                    .chain(
                        stage
                            .sampling_support
                            .iter()
                            .map(|key| (*key, TerrainRequestClass::Visible)),
                    )
            })
            .collect::<Vec<_>>();
        let mut requests = Vec::new();
        let mut bounded_work = 0;
        for (key, request_class) in staged_work {
            if published.contains(&key) {
                continue;
            }
            if bounded_work == self.frontier.config.max_requests_per_reconcile {
                break;
            }
            if let Some(generation) = resident.get(&key) {
                publisher.ensure_resident_upload(self.planet_id(), key, *generation)?;
                bounded_work += 1;
                continue;
            }
            let deadline = match request_class {
                TerrainRequestClass::Prefetch => self.frontier.config.prefetch_deadline_ticks,
                TerrainRequestClass::Visible
                | TerrainRequestClass::Collision
                | TerrainRequestClass::EditResponse => self.frontier.config.visible_deadline_ticks,
            };
            requests.push((key, request_class, tick.saturating_add(deadline)));
            bounded_work += 1;
        }
        let (outcomes, request_error) = runtime.request_pages_bounded(self.planet_id(), &requests);
        report.submitted = outcomes.len() + usize::from(request_error.is_some());
        for outcome in outcomes {
            match outcome {
                TerrainRequestOutcome::Queued { .. } => report.queued += 1,
                TerrainRequestOutcome::Coalesced { .. } => report.coalesced += 1,
                TerrainRequestOutcome::Current { .. } => report.current += 1,
            }
        }
        if let Some(error) = request_error {
            if is_backpressure(&error) {
                report.deferred += 1;
            } else {
                return Err(error.into());
            }
        }

        self.frontier.counters.requests_queued = self
            .frontier
            .counters
            .requests_queued
            .saturating_add(report.queued as u64);
        self.frontier.counters.requests_coalesced = self
            .frontier
            .counters
            .requests_coalesced
            .saturating_add(report.coalesced as u64);
        self.frontier.counters.pages_current = self
            .frontier
            .counters
            .pages_current
            .saturating_add(report.current as u64);
        self.frontier.counters.requests_deferred = self
            .frontier
            .counters
            .requests_deferred
            .saturating_add(report.deferred as u64);
        self.frontier.counters.pages_evicted = self
            .frontier
            .counters
            .pages_evicted
            .saturating_add(report.evicted as u64);
        report.committed_pages = self.frontier.committed.len();
        report.committed_sampling_pages = self.frontier.sampling_support.len();
        report.staged_pages = self
            .frontier
            .staged
            .as_ref()
            .map_or(0, |stage| stage.additions.len());
        report.staged_sampling_pages = self
            .frontier
            .staged
            .as_ref()
            .map_or(0, |stage| stage.sampling_support.len());
        report.converged = self.frontier.is_converged();
        Ok(report)
    }
}

fn choose_replacement(
    committed: &BTreeMap<PageKey, TerrainRequestClass>,
    target: &TargetFrontier,
) -> Result<Option<StagedReplacement>, TerrainRefinementError> {
    for (key, request_class) in target.plan.residency_identity() {
        if committed
            .get(key)
            .is_some_and(|class| class != request_class)
        {
            return Ok(Some(StagedReplacement {
                additions: BTreeMap::from([(*key, *request_class)]),
                removals: BTreeSet::from([*key]),
                sampling_support: BTreeSet::new(),
            }));
        }
    }

    let mut coarsen = BTreeSet::new();
    for key in committed.keys().copied() {
        let Some(parent) = key.parent() else {
            continue;
        };
        if target_covering(parent, target).is_some() {
            coarsen.insert(parent);
        }
    }
    for parent in coarsen {
        let removals = committed
            .keys()
            .copied()
            .filter(|key| is_ancestor(parent, *key))
            .collect::<BTreeSet<_>>();
        let Some(target_class) = target_covering(parent, target) else {
            continue;
        };
        let stage = StagedReplacement {
            additions: BTreeMap::from([(parent, target_class)]),
            removals,
            sampling_support: BTreeSet::new(),
        };
        if replacement_is_safe(committed, &stage) {
            return Ok(Some(stage));
        }
    }

    let mut refine = committed
        .keys()
        .copied()
        .filter_map(|parent| {
            let descendants = target_descendants(parent, target);
            (!descendants.is_empty()).then_some((parent, descendants))
        })
        .collect::<Vec<_>>();
    refine.sort_unstable_by(|(left, left_demands), (right, right_demands)| {
        let left_visible = left_demands
            .iter()
            .any(|demand| demand.request_class() == TerrainRequestClass::Visible);
        let right_visible = right_demands
            .iter()
            .any(|demand| demand.request_class() == TerrainRequestClass::Visible);
        right_visible
            .cmp(&left_visible)
            .then_with(|| right.lod.cmp(&left.lod))
            .then_with(|| {
                let left_error = left_demands
                    .iter()
                    .map(|demand| demand.projected_error_px())
                    .fold(0.0_f64, f64::max);
                let right_error = right_demands
                    .iter()
                    .map(|demand| demand.projected_error_px())
                    .fold(0.0_f64, f64::max);
                right_error.total_cmp(&left_error)
            })
            .then_with(|| left.cmp(right))
    });
    for (parent, descendants) in refine {
        let child_lod = parent
            .lod
            .checked_sub(1)
            .ok_or(TerrainRefinementError::CoordinateOverflow)?;
        let mut additions = BTreeMap::new();
        for demand in descendants {
            let child = ancestor_at_lod(demand.page_key(), child_lod)
                .ok_or(TerrainRefinementError::CoordinateOverflow)?;
            additions
                .entry(child)
                .and_modify(|class| *class = stronger_request_class(*class, demand.request_class()))
                .or_insert(demand.request_class());
        }
        let stage = StagedReplacement {
            additions,
            removals: BTreeSet::from([parent]),
            sampling_support: BTreeSet::new(),
        };
        if replacement_is_safe(committed, &stage) {
            return Ok(Some(stage));
        }
    }
    Ok(None)
}

fn retarget_stage_classes(stage: &mut StagedReplacement, target: &TargetFrontier) {
    for (page_key, request_class) in &mut stage.additions {
        let next_class = target.plan.class_for_related(*page_key);
        if let Some(next_class) = next_class {
            *request_class = next_class;
        }
    }
}

fn stage_advances_target(
    committed: &BTreeMap<PageKey, TerrainRequestClass>,
    stage: &StagedReplacement,
    target: &TargetFrontier,
) -> bool {
    if committed.is_empty() {
        return stage
            .additions
            .keys()
            .all(|addition| target_has_descendant_or_same(*addition, target));
    }
    let refining = stage.removals.iter().any(|removal| {
        stage
            .additions
            .keys()
            .any(|addition| removal != addition && is_ancestor(*removal, *addition))
    });
    if refining {
        return stage
            .additions
            .keys()
            .all(|addition| target_has_descendant_or_same(*addition, target));
    }
    let coarsening = stage.additions.keys().any(|addition| {
        stage
            .removals
            .iter()
            .any(|removal| addition != removal && is_ancestor(*addition, *removal))
    });
    if coarsening {
        return stage
            .additions
            .keys()
            .all(|addition| target_has_ancestor_or_same(*addition, target));
    }
    stage.additions.iter().all(|(addition, class)| {
        target
            .plan
            .residency_identity()
            .binary_search_by_key(addition, |(page_key, _)| *page_key)
            .ok()
            .is_some_and(|index| target.plan.residency_identity()[index].1 == *class)
    })
}

fn target_has_descendant_or_same(page_key: PageKey, target: &TargetFrontier) -> bool {
    target.plan.class_for_descendant_or_same(page_key).is_some()
}

fn target_has_ancestor_or_same(page_key: PageKey, target: &TargetFrontier) -> bool {
    target.plan.class_for_ancestor_or_same(page_key).is_some()
}

#[cfg(test)]
fn frontier_distance(
    frontier: &BTreeMap<PageKey, TerrainRequestClass>,
    target: &TargetFrontier,
) -> Option<u128> {
    let target_residency = target.plan.residency_identity();
    if frontier.is_empty() || target_residency.is_empty() {
        return None;
    }
    if frontier.keys().any(|frontier_key| {
        !target_residency
            .iter()
            .any(|(target_key, _)| pages_are_related(*frontier_key, *target_key))
    }) {
        return None;
    }

    let mut distance = 0_u128;
    for (target_key, target_class) in target_residency {
        let local = frontier
            .iter()
            .filter_map(|(frontier_key, frontier_class)| {
                pages_are_related(*frontier_key, *target_key).then_some(
                    u128::from(frontier_key.lod.abs_diff(target_key.lod))
                        + u128::from(
                            (*frontier_key == *target_key && *frontier_class != *target_class)
                                as u8,
                        ),
                )
            })
            .min()?;
        distance = distance.checked_add(local)?;
    }
    Some(distance)
}

#[cfg(test)]
fn pages_are_related(left: PageKey, right: PageKey) -> bool {
    is_ancestor(left, right) || is_ancestor(right, left)
}

fn coarse_seed(
    target: &TargetFrontier,
    maximum_pages: usize,
) -> Result<BTreeMap<PageKey, TerrainRequestClass>, TerrainRefinementError> {
    if target.plan.residency_identity().is_empty() {
        return Err(TerrainRefinementError::EmptyTarget);
    }
    target
        .plan
        .coarse_frontier(maximum_pages)
        .map(|pages| pages.iter().copied().collect())
        .ok_or(TerrainRefinementError::CoordinateOverflow)
}

fn target_descendants(parent: PageKey, target: &TargetFrontier) -> Vec<&PageDemand> {
    target
        .plan
        .demands()
        .iter()
        .filter(|demand| {
            let key = demand.page_key();
            key.lod < parent.lod && is_ancestor(parent, key)
        })
        .collect()
}

fn target_covering(key: PageKey, target: &TargetFrontier) -> Option<TerrainRequestClass> {
    target.plan.class_for_ancestor_or_same(key)
}

fn replacement_is_safe(
    committed: &BTreeMap<PageKey, TerrainRequestClass>,
    stage: &StagedReplacement,
) -> bool {
    let mut prospective = committed.clone();
    for key in &stage.removals {
        prospective.remove(key);
    }
    prospective.extend(stage.additions.iter().map(|(key, class)| (*key, *class)));
    validate_non_overlapping(prospective.keys().copied()).is_ok()
        && is_face_balanced(prospective.keys().copied())
}

fn sampling_support_after(
    committed: &BTreeMap<PageKey, TerrainRequestClass>,
    stage: &StagedReplacement,
) -> Result<BTreeSet<PageKey>, TerrainSurfaceSamplingError> {
    let mut prospective = committed.clone();
    for key in &stage.removals {
        prospective.remove(key);
    }
    prospective.extend(stage.additions.iter().map(|(key, class)| (*key, *class)));
    let masks = transition_masks(prospective.keys().copied());
    terrain_frontier_sampling_support(prospective.into_keys(), &masks)
}

pub(crate) fn is_ancestor(ancestor: PageKey, descendant: PageKey) -> bool {
    if ancestor.lod < descendant.lod {
        return false;
    }
    ancestor_at_lod(descendant, ancestor.lod) == Some(ancestor)
}

fn ancestor_at_lod(mut key: PageKey, lod: u8) -> Option<PageKey> {
    if lod < key.lod {
        return None;
    }
    while key.lod < lod {
        key = key.parent()?;
    }
    Some(key)
}

fn validate_non_overlapping(
    pages: impl IntoIterator<Item = PageKey>,
) -> Result<(), TerrainRefinementError> {
    let pages = pages.into_iter().collect::<BTreeSet<_>>();
    for key in &pages {
        let mut ancestor = key.parent();
        while let Some(parent) = ancestor {
            if pages.contains(&parent) {
                return Err(TerrainRefinementError::OverlappingPages {
                    ancestor: parent,
                    descendant: *key,
                });
            }
            ancestor = parent.parent();
        }
    }
    Ok(())
}

pub(crate) fn is_face_balanced(pages: impl IntoIterator<Item = PageKey>) -> bool {
    let pages = pages.into_iter().collect::<BTreeSet<_>>();
    pages.iter().all(|leaf| {
        faces().into_iter().all(|(axis, direction)| {
            covering_face_neighbor(*leaf, axis, direction, &pages)
                .is_none_or(|neighbor| leaf.lod.abs_diff(neighbor.lod) <= 1)
        })
    })
}

pub(crate) fn transition_masks(pages: impl IntoIterator<Item = PageKey>) -> BTreeMap<PageKey, u8> {
    let pages = pages.into_iter().collect::<BTreeSet<_>>();
    pages
        .iter()
        .copied()
        .map(|leaf| {
            let mut mask = 0_u8;
            for (face_index, (axis, direction)) in faces().into_iter().enumerate() {
                let finer = finer_face_neighbors(leaf, axis, direction).is_some_and(|neighbors| {
                    neighbors.iter().any(|neighbor| pages.contains(neighbor))
                });
                if finer {
                    mask |= 1 << face_index;
                }
            }
            (leaf, mask)
        })
        .collect()
}

fn covering_face_neighbor(
    leaf: PageKey,
    axis: usize,
    direction: i64,
    pages: &BTreeSet<PageKey>,
) -> Option<PageKey> {
    let mut coordinate = leaf.page_xyz;
    coordinate[axis] = coordinate[axis].checked_add(direction)?;
    let mut candidate = PageKey::new(leaf.lod, coordinate);
    loop {
        if pages.contains(&candidate) {
            return Some(candidate);
        }
        candidate = candidate.parent()?;
        if candidate.lod > 62 {
            return None;
        }
    }
}

fn finer_face_neighbors(coarse: PageKey, axis: usize, direction: i64) -> Option<[PageKey; 4]> {
    let lod = coarse.lod.checked_sub(1)?;
    let base = [
        coarse.page_xyz[0].checked_mul(2)?,
        coarse.page_xyz[1].checked_mul(2)?,
        coarse.page_xyz[2].checked_mul(2)?,
    ];
    let face_coordinate = if direction < 0 {
        base[axis].checked_sub(1)?
    } else {
        base[axis].checked_add(2)?
    };
    let tangential = match axis {
        0 => [1, 2],
        1 => [0, 2],
        2 => [0, 1],
        _ => return None,
    };
    let mut neighbors = [PageKey::default(); 4];
    for (quadrant, neighbor) in neighbors.iter_mut().enumerate() {
        let mut page_xyz = base;
        page_xyz[axis] = face_coordinate;
        page_xyz[tangential[0]] = page_xyz[tangential[0]].checked_add((quadrant & 1) as i64)?;
        page_xyz[tangential[1]] =
            page_xyz[tangential[1]].checked_add(((quadrant >> 1) & 1) as i64)?;
        *neighbor = PageKey::new(lod, page_xyz);
    }
    Some(neighbors)
}

const fn faces() -> [(usize, i64); 6] {
    [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)]
}

const fn stronger_request_class(
    left: TerrainRequestClass,
    right: TerrainRequestClass,
) -> TerrainRequestClass {
    if request_priority(right) > request_priority(left) {
        right
    } else {
        left
    }
}

const fn request_priority(class: TerrainRequestClass) -> u8 {
    match class {
        TerrainRequestClass::Prefetch => 0,
        TerrainRequestClass::Visible => 1,
        TerrainRequestClass::Collision => 2,
        TerrainRequestClass::EditResponse => 3,
    }
}

fn is_backpressure(error: &TerrainRuntimeError) -> bool {
    matches!(
        error,
        TerrainRuntimeError::RequestBackpressure { .. }
            | TerrainRuntimeError::CompletionBackpressure { .. }
            | TerrainRuntimeError::EventBackpressure { .. }
            | TerrainRuntimeError::PlanetResidentPageBudget { .. }
            | TerrainRuntimeError::GlobalResidentPageBudget { .. }
            | TerrainRuntimeError::ResidentByteBudget { .. }
    )
}

#[derive(Debug, Error)]
pub enum TerrainRefinementError {
    #[error("invalid terrain refinement configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("refinement session belongs to planet {session:?}, not plan planet {plan:?}")]
    PlanetMismatch { session: PlanetId, plan: PlanetId },
    #[error("target contains {pages} pages but the active-page budget is {capacity}")]
    ActivePageBudget { pages: usize, capacity: usize },
    #[error("refinement handoff requires {pages} pages but the transition budget is {capacity}")]
    TransitionPageBudget { pages: usize, capacity: usize },
    #[error("the target terrain frontier is empty")]
    EmptyTarget,
    #[error("no target terrain frontier has been submitted")]
    MissingTarget,
    #[error(transparent)]
    Sampling(#[from] TerrainSurfaceSamplingError),
    #[error("the target terrain frontier is not 2:1 face balanced")]
    UnbalancedTarget,
    #[error("invalid target terrain frontier: {0}")]
    InvalidTargetInvariant(&'static str),
    #[error("the committed terrain frontier would not be 2:1 face balanced")]
    UnbalancedCommit,
    #[error("terrain pages overlap: ancestor {ancestor:?}, descendant {descendant:?}")]
    OverlappingPages {
        ancestor: PageKey,
        descendant: PageKey,
    },
    #[error("no parent-preserving replacement can advance the current frontier")]
    NoSafeReplacement,
    #[error("terrain refinement coordinate arithmetic overflowed")]
    CoordinateOverflow,
    #[error(transparent)]
    Runtime(#[from] TerrainRuntimeError),
    #[error(transparent)]
    Render(#[from] TerrainRenderDeltaError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PlanetDefinition, PlanetPosition, PlanetView, TerrainStreamingConfig,
        TerrainStreamingPlanner,
    };

    fn planet() -> PlanetId {
        PlanetId([7; 16])
    }

    fn demand(key: PageKey) -> PageDemand {
        PageDemand::for_test(key, TerrainRequestClass::Visible)
    }

    fn plan(keys: impl IntoIterator<Item = PageKey>) -> TerrainStreamingPlan {
        TerrainStreamingPlan::for_test(planet(), keys.into_iter().map(demand).collect())
    }

    fn direct_children(parent: PageKey) -> Vec<PageKey> {
        let lod = parent.lod - 1;
        let base = parent.page_xyz.map(|axis| axis * 2);
        (0..8)
            .map(|index| {
                PageKey::new(
                    lod,
                    [
                        base[0] + (index & 1),
                        base[1] + ((index >> 1) & 1),
                        base[2] + ((index >> 2) & 1),
                    ],
                )
            })
            .collect()
    }

    fn converge_frontier(
        frontier: &mut TerrainRefinementFrontier,
        target: &TerrainStreamingPlan,
        resident: &mut BTreeSet<PageKey>,
    ) {
        frontier.set_target(target).unwrap();
        let mut commits = 0;
        while !frontier.is_converged() {
            frontier.prepare_stage().unwrap();
            resident.extend(frontier.staged_pages());
            let (retired, committed) = frontier.commit_ready_stages(resident).unwrap();
            assert!((1..=frontier.config().max_commits_per_reconcile).contains(&committed));
            for key in retired {
                resident.remove(&key);
            }
            commits += committed;
            let pages = frontier.committed_pages().collect::<BTreeSet<_>>();
            validate_non_overlapping(pages.iter().copied()).unwrap();
            assert!(is_face_balanced(pages.iter().copied()));
            assert!(
                frontier_distance(&frontier.committed, frontier.target.as_ref().unwrap()).is_some()
            );
            assert!(commits < 512, "refinement did not converge");
        }
    }

    #[test]
    fn initial_publication_is_a_coarse_parent_not_the_full_target() {
        let parent = PageKey::new(3, [-1, 0, 0]);
        let children = direct_children(parent);
        let target = plan(children.clone());
        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                initial_coarse_pages: 1,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        frontier.set_target(&target).unwrap();
        frontier.prepare_stage().unwrap();
        assert_eq!(
            frontier.staged_surface_pages().collect::<Vec<_>>(),
            vec![parent]
        );
        assert_eq!(frontier.staged_sampling_support_pages().count(), 26);

        let resident = frontier.staged_pages().collect();
        let (retired, committed) = frontier.commit_ready_stages(&resident).unwrap();
        assert!(retired.is_empty());
        assert_eq!(committed, 1);
        assert_eq!(frontier.committed_pages().collect::<Vec<_>>(), vec![parent]);
        assert!(!frontier.is_converged());
    }

    #[test]
    fn parent_remains_committed_until_every_replacement_child_is_ready() {
        let parent = PageKey::new(2, [0, 0, 0]);
        let children = direct_children(parent);
        let target = plan(children.clone());
        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                initial_coarse_pages: 1,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        frontier.set_target(&target).unwrap();
        frontier.prepare_stage().unwrap();
        let initial = frontier.staged_pages().collect();
        frontier.commit_ready_stages(&initial).unwrap();
        frontier.prepare_stage().unwrap();

        let mut partial = frontier.staged_pages().collect::<BTreeSet<_>>();
        partial.remove(&children[7]);
        let (retired, committed) = frontier.commit_ready_stages(&partial).unwrap();
        assert!(retired.is_empty());
        assert_eq!(committed, 0);
        assert_eq!(frontier.committed_pages().collect::<Vec<_>>(), vec![parent]);

        let ready = frontier.staged_pages().collect::<BTreeSet<_>>();
        let (retired, committed) = frontier.commit_ready_stages(&ready).unwrap();
        assert!(retired.contains(&parent));
        assert_eq!(committed, 1);
        assert_eq!(
            frontier.committed_pages().collect::<BTreeSet<_>>(),
            children.into_iter().collect::<BTreeSet<_>>()
        );
        assert!(frontier.is_converged());
    }

    #[test]
    fn coarsening_keeps_children_until_the_parent_is_ready() {
        let parent = PageKey::new(2, [-1, -1, -1]);
        let children = direct_children(parent);
        let fine = plan(children.clone());
        let coarse = plan([parent]);
        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                initial_coarse_pages: 8,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        frontier.set_target(&fine).unwrap();
        frontier.prepare_stage().unwrap();
        let fine_ready = frontier.staged_pages().collect();
        frontier.commit_ready_stages(&fine_ready).unwrap();
        assert!(frontier.is_converged());

        frontier.set_target(&coarse).unwrap();
        frontier.prepare_stage().unwrap();
        let (retired, committed) = frontier.commit_ready_stages(&BTreeSet::new()).unwrap();
        assert!(retired.is_empty());
        assert_eq!(committed, 0);
        assert_eq!(frontier.committed_pages().count(), 8);

        let coarse_ready = frontier.staged_pages().collect();
        let (retired, committed) = frontier.commit_ready_stages(&coarse_ready).unwrap();
        let retired = retired.into_iter().collect::<BTreeSet<_>>();
        assert!(children.into_iter().all(|child| retired.contains(&child)));
        assert_eq!(committed, 1);
        assert_eq!(frontier.committed_pages().collect::<Vec<_>>(), vec![parent]);
        assert!(frontier.is_converged());
    }

    #[test]
    fn a_converged_stationary_target_performs_no_more_work() {
        let target = plan([PageKey::new(4, [0, 0, 0])]);
        let mut frontier =
            TerrainRefinementFrontier::new(planet(), TerrainRefinementConfig::default()).unwrap();
        frontier.set_target(&target).unwrap();
        frontier.prepare_stage().unwrap();
        frontier
            .commit_ready_stages(&BTreeSet::from([PageKey::new(4, [0, 0, 0])]))
            .unwrap();
        let before = frontier.counters();
        assert!(frontier.set_target(&target).unwrap().is_empty());
        assert!(!frontier.prepare_stage().unwrap());
        let (retired, committed) = frontier
            .commit_ready_stages(&BTreeSet::from([PageKey::new(4, [0, 0, 0])]))
            .unwrap();
        assert!(retired.is_empty());
        assert_eq!(committed, 0);
        assert_eq!(frontier.counters(), before);
    }

    #[test]
    fn request_class_changes_do_not_evict_the_resident_page() {
        let key = PageKey::new(4, [-1, 0, 1]);
        let visible = TerrainStreamingPlan::for_test(
            planet(),
            vec![PageDemand::for_test(key, TerrainRequestClass::Visible)],
        );
        let prefetch = TerrainStreamingPlan::for_test(
            planet(),
            vec![PageDemand::for_test(key, TerrainRequestClass::Prefetch)],
        );
        let mut frontier =
            TerrainRefinementFrontier::new(planet(), TerrainRefinementConfig::default()).unwrap();
        frontier.set_target(&visible).unwrap();
        frontier.prepare_stage().unwrap();
        let ready = frontier.staged_pages().collect();
        frontier.commit_ready_stages(&ready).unwrap();

        frontier.set_target(&prefetch).unwrap();
        let resident = frontier.protected_pages().collect();
        let (retired, committed) = frontier.commit_ready_stages(&resident).unwrap();
        assert!(retired.is_empty());
        assert_eq!(committed, 1);
        assert_eq!(
            frontier.committed_demands().collect::<Vec<_>>(),
            vec![(key, TerrainRequestClass::Prefetch)]
        );
        assert!(frontier.is_converged());
    }

    #[test]
    fn superseding_a_target_cancels_only_uncommitted_staging() {
        let parent = PageKey::new(3, [0, 0, 0]);
        let fine = plan(direct_children(parent));
        let replacement = plan([parent]);
        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                initial_coarse_pages: 8,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        frontier.set_target(&fine).unwrap();
        frontier.prepare_stage().unwrap();
        let staged = frontier.staged_pages().collect::<BTreeSet<_>>();
        let cancelled = frontier
            .set_target(&replacement)
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert_eq!(cancelled, staged);
        assert_eq!(frontier.counters().stages_cancelled, 1);
    }

    #[test]
    fn superseding_with_a_finer_plan_preserves_useful_staging() {
        let parent = PageKey::new(3, [0, 0, 0]);
        let children = direct_children(parent);
        let first = plan(children.clone());
        let refined_child = children[0];
        let mut next_keys = children[1..].to_vec();
        next_keys.extend(direct_children(refined_child));
        let next = plan(next_keys);
        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                initial_coarse_pages: 1,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        frontier.set_target(&first).unwrap();
        frontier.prepare_stage().unwrap();
        let parent_ready = frontier.staged_pages().collect();
        frontier.commit_ready_stages(&parent_ready).unwrap();
        frontier.prepare_stage().unwrap();
        assert_eq!(
            frontier.staged_surface_pages().collect::<BTreeSet<_>>(),
            children.iter().copied().collect()
        );

        assert!(frontier.set_target(&next).unwrap().is_empty());
        assert_eq!(frontier.counters().stages_cancelled, 0);
        assert_eq!(
            frontier.staged_surface_pages().collect::<BTreeSet<_>>(),
            children.iter().copied().collect()
        );
        let ready = frontier.staged_pages().collect();
        let (_, committed) = frontier.commit_ready_stages(&ready).unwrap();
        assert_eq!(committed, 1);
        assert_eq!(
            frontier.committed_pages().collect::<BTreeSet<_>>(),
            children.into_iter().collect()
        );
        assert!(!frontier.is_converged());
    }

    #[test]
    fn transition_masks_are_owned_only_by_the_coarse_page() {
        let coarse = PageKey::new(2, [0, 0, 0]);
        let fine = [
            PageKey::new(1, [2, 0, 0]),
            PageKey::new(1, [2, 1, 0]),
            PageKey::new(1, [2, 0, 1]),
            PageKey::new(1, [2, 1, 1]),
        ];
        let masks = transition_masks(std::iter::once(coarse).chain(fine));
        assert_eq!(masks[&coarse], 1 << 1);
        assert!(fine.into_iter().all(|key| masks[&key] == 0));
    }

    #[test]
    fn an_authoritative_mixed_lod_plan_converges_through_valid_frontiers() {
        let definition = PlanetDefinition {
            planet_id: planet(),
            center_cell: [0; 3],
            radius_cells: 1_000,
            material: 1,
            root_lod: 6,
            max_resident_pages: 256,
        };
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            target_projected_error_px: 2.0,
            finest_surface_lod: 0,
            prediction_seconds: 0.0,
            max_pages: 256,
            max_traversal_nodes: 4_096,
        })
        .unwrap();
        let view = PlanetView::new(
            PlanetPosition::from_lod0_cell([-1_000, 0, 0]),
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            60_f64.to_radians(),
            [1280, 720],
            0.1,
            20_000_000.0,
            [0.0; 3],
        )
        .unwrap();
        let target = planner.plan_fixed_sphere(&definition, view).unwrap();
        assert!(target
            .demands()
            .iter()
            .any(|demand| demand.page_key().lod == 0));
        assert!(target
            .demands()
            .iter()
            .any(|demand| demand.page_key().lod > 0));

        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                max_active_pages: 256,
                max_transition_pages: 2_048,
                initial_coarse_pages: 8,
                max_commits_per_reconcile: 1,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        let mut resident = BTreeSet::new();
        converge_frontier(&mut frontier, &target, &mut resident);
        assert!(frontier.counters().transition_page_high_water <= 2_048);
        assert_eq!(
            frontier.committed_pages().collect::<BTreeSet<_>>(),
            target
                .demands()
                .iter()
                .map(|demand| demand.page_key())
                .collect()
        );
    }

    #[test]
    fn ground_to_orbit_coarsening_converges_without_invalidating_coverage() {
        let definition = PlanetDefinition {
            planet_id: planet(),
            center_cell: [0; 3],
            radius_cells: 1_000,
            material: 1,
            root_lod: 6,
            max_resident_pages: 64,
        };
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            target_projected_error_px: 2.0,
            finest_surface_lod: 0,
            prediction_seconds: 0.0,
            max_pages: 64,
            max_traversal_nodes: 4_096,
        })
        .unwrap();
        let make_view = |cell, look| {
            PlanetView::new(
                PlanetPosition::from_lod0_cell(cell),
                look,
                [0.0, 1.0, 0.0],
                60_f64.to_radians(),
                [1280, 720],
                0.1,
                20_000_000.0,
                [0.0; 3],
            )
            .unwrap()
        };
        let ground = planner
            .plan_fixed_sphere(&definition, make_view([1_000, 0, 0], [-1.0, 0.0, 0.0]))
            .unwrap();
        let orbit = planner
            .plan_fixed_sphere(&definition, make_view([100_000, 0, 0], [-1.0, 0.0, 0.0]))
            .unwrap();
        assert!(ground.demands().len() > orbit.demands().len());

        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                max_active_pages: 64,
                max_transition_pages: 1_024,
                initial_coarse_pages: 8,
                max_commits_per_reconcile: 1,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        let mut resident = BTreeSet::new();
        converge_frontier(&mut frontier, &ground, &mut resident);
        converge_frontier(&mut frontier, &orbit, &mut resident);
        assert!(frontier.counters().transition_page_high_water <= 1_024);
        assert_eq!(
            frontier.committed_pages().collect::<BTreeSet<_>>(),
            orbit
                .demands()
                .iter()
                .map(|demand| demand.page_key())
                .collect()
        );
    }

    #[test]
    fn earth_scale_live_frontier_fits_the_bounded_sampling_residency() {
        let radius_cells = 63_710_000_u64;
        let definition = PlanetDefinition {
            planet_id: planet(),
            center_cell: [0; 3],
            radius_cells,
            material: 1,
            root_lod: 22,
            max_resident_pages: 2_048,
        };
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            max_pages: 96,
            ..TerrainStreamingConfig::default()
        })
        .unwrap();
        let camera_cell = i64::try_from(radius_cells).unwrap() + 10;
        let make_view = |camera_cell, forward| {
            PlanetView::new(
                PlanetPosition::from_lod0_cell(camera_cell),
                forward,
                [0.0, 1.0, 0.0],
                60_f64.to_radians(),
                [1920, 1080],
                0.1,
                100_000_000.0,
                [0.0; 3],
            )
            .unwrap()
        };
        let ground = planner
            .plan_fixed_sphere(
                &definition,
                make_view([camera_cell, 0, 0], [-1.0, 0.0, 0.0]),
            )
            .unwrap();
        let orbit = planner
            .plan_fixed_sphere(
                &definition,
                make_view([camera_cell + 40_000_000, 0, 0], [-1.0, 0.0, 0.0]),
            )
            .unwrap();
        let antipode = planner
            .plan_fixed_sphere(
                &definition,
                make_view([-camera_cell, 0, 0], [1.0, 0.0, 0.0]),
            )
            .unwrap();
        let mut frontier = TerrainRefinementFrontier::new(
            planet(),
            TerrainRefinementConfig {
                max_active_pages: 96,
                max_transition_pages: 2_048,
                initial_coarse_pages: 32,
                max_commits_per_reconcile: 8,
                ..TerrainRefinementConfig::default()
            },
        )
        .unwrap();
        let mut resident = BTreeSet::new();
        converge_frontier(&mut frontier, &ground, &mut resident);
        converge_frontier(&mut frontier, &orbit, &mut resident);
        converge_frontier(&mut frontier, &antipode, &mut resident);
        assert!(frontier.counters().transition_page_high_water > 512);
        assert!(frontier.counters().transition_page_high_water <= 2_048);
    }
}
