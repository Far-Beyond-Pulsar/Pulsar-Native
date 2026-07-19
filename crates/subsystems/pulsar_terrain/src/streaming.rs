use crate::{
    FixedSphereGenerator, PageKey, PlanetDefinition, PlanetId, PlanetPosition, TerrainRequestClass,
    LOD0_CELL_SIZE_METERS,
};
use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};
use thiserror::Error;

/// Conservative classification of one hierarchical terrain page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainRegion {
    UniformAir,
    UniformSolid,
    Surface,
}

/// Supplies conservative occupancy without making the planner own terrain state.
///
/// Returning `Surface` is always safe. Returning a uniform state promises that
/// no surface sample exists inside the page and allows the traversal to stop.
pub trait TerrainRegionClassifier {
    fn classify_region(&self, key: PageKey) -> Result<TerrainRegion, TerrainStreamingError>;
}

impl TerrainRegionClassifier for FixedSphereGenerator {
    fn classify_region(&self, key: PageKey) -> Result<TerrainRegion, TerrainStreamingError> {
        let min = key
            .lod0_cell_min()
            .ok_or(TerrainStreamingError::CoordinateOverflow)?;
        let span = key
            .lod0_cell_span()
            .ok_or(TerrainStreamingError::CoordinateOverflow)?;
        let max = [
            min[0]
                .checked_add(span - 1)
                .ok_or(TerrainStreamingError::CoordinateOverflow)?,
            min[1]
                .checked_add(span - 1)
                .ok_or(TerrainStreamingError::CoordinateOverflow)?,
            min[2]
                .checked_add(span - 1)
                .ok_or(TerrainStreamingError::CoordinateOverflow)?,
        ];

        let mut minimum_distance_squared = 0_u128;
        let mut maximum_distance_squared = 0_u128;
        for axis in 0..3 {
            let center = i128::from(self.center_cell[axis]);
            let low = i128::from(min[axis]);
            let high = i128::from(max[axis]);
            let nearest = if center < low {
                low - center
            } else if center > high {
                center - high
            } else {
                0
            } as u128;
            let farthest = (low - center)
                .unsigned_abs()
                .max((high - center).unsigned_abs());
            minimum_distance_squared =
                minimum_distance_squared.saturating_add(nearest.saturating_mul(nearest));
            maximum_distance_squared =
                maximum_distance_squared.saturating_add(farthest.saturating_mul(farthest));
        }

        let radius_squared = u128::from(self.radius_cells).pow(2);
        if minimum_distance_squared > radius_squared {
            Ok(TerrainRegion::UniformAir)
        } else if maximum_distance_squared <= radius_squared {
            Ok(TerrainRegion::UniformSolid)
        } else {
            Ok(TerrainRegion::Surface)
        }
    }
}

/// One canonical camera and its predictive streaming inputs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlanetView {
    camera: PlanetPosition,
    forward: [f64; 3],
    up: [f64; 3],
    vertical_fov_radians: f64,
    viewport_px: [u32; 2],
    near_m: f64,
    far_m: f64,
    velocity_mps: [f64; 3],
}

impl PlanetView {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        camera: PlanetPosition,
        forward: [f64; 3],
        up: [f64; 3],
        vertical_fov_radians: f64,
        viewport_px: [u32; 2],
        near_m: f64,
        far_m: f64,
        velocity_mps: [f64; 3],
    ) -> Result<Self, TerrainStreamingError> {
        if forward
            .iter()
            .chain(up.iter())
            .chain(velocity_mps.iter())
            .any(|value| !value.is_finite())
            || !vertical_fov_radians.is_finite()
            || !near_m.is_finite()
            || !far_m.is_finite()
        {
            return Err(TerrainStreamingError::InvalidView(
                "view values must be finite",
            ));
        }
        if viewport_px[0] == 0 || viewport_px[1] == 0 {
            return Err(TerrainStreamingError::InvalidView(
                "viewport dimensions must be non-zero",
            ));
        }
        if !(0.0..std::f64::consts::PI).contains(&vertical_fov_radians) {
            return Err(TerrainStreamingError::InvalidView(
                "vertical FOV must be in (0, pi)",
            ));
        }
        if near_m < 0.0 || far_m <= near_m {
            return Err(TerrainStreamingError::InvalidView(
                "far distance must be greater than a non-negative near distance",
            ));
        }
        let forward = normalize(forward).ok_or(TerrainStreamingError::InvalidView(
            "forward vector must be non-zero",
        ))?;
        let right = normalize(cross(forward, up)).ok_or(TerrainStreamingError::InvalidView(
            "forward and up vectors must not be collinear",
        ))?;
        let up = cross(right, forward);
        Ok(Self {
            camera,
            forward,
            up,
            vertical_fov_radians,
            viewport_px,
            near_m,
            far_m,
            velocity_mps,
        })
    }

    pub const fn camera(self) -> PlanetPosition {
        self.camera
    }

    pub const fn velocity_mps(self) -> [f64; 3] {
        self.velocity_mps
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainStreamingConfig {
    pub interaction_radius_m: f64,
    pub target_projected_error_px: f64,
    pub prediction_seconds: f64,
    pub max_pages: usize,
    pub max_traversal_nodes: usize,
}

impl Default for TerrainStreamingConfig {
    fn default() -> Self {
        Self {
            interaction_radius_m: 64.0,
            target_projected_error_px: 2.0,
            prediction_seconds: 0.75,
            max_pages: 8_192,
            max_traversal_nodes: 262_144,
        }
    }
}

impl TerrainStreamingConfig {
    fn validate(self) -> Result<(), TerrainStreamingError> {
        if !self.interaction_radius_m.is_finite() || self.interaction_radius_m < 0.0 {
            return Err(TerrainStreamingError::InvalidConfig(
                "interaction radius must be finite and non-negative",
            ));
        }
        if !self.target_projected_error_px.is_finite() || self.target_projected_error_px <= 0.0 {
            return Err(TerrainStreamingError::InvalidConfig(
                "projected error target must be finite and positive",
            ));
        }
        if !self.prediction_seconds.is_finite() || self.prediction_seconds < 0.0 {
            return Err(TerrainStreamingError::InvalidConfig(
                "prediction horizon must be finite and non-negative",
            ));
        }
        if self.max_pages == 0 || self.max_traversal_nodes < 8 {
            return Err(TerrainStreamingError::InvalidConfig(
                "budgets must allow at least the eight centered root children",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageDemand {
    page_key: PageKey,
    request_class: TerrainRequestClass,
    projected_error_px: f64,
    distance_m: f64,
}

impl PageDemand {
    pub const fn page_key(self) -> PageKey {
        self.page_key
    }

    pub const fn request_class(self) -> TerrainRequestClass {
        self.request_class
    }

    pub const fn projected_error_px(self) -> f64 {
        self.projected_error_px
    }

    pub const fn distance_m(self) -> f64 {
        self.distance_m
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TerrainStreamingLimit {
    PageBudget,
    TraversalBudget,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainStreamingCounters {
    pub traversed_nodes: usize,
    pub refinements: usize,
    pub balance_refinements: usize,
    pub page_high_water: usize,
    pub deferred_refinements: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TerrainStreamingPlan {
    planet_id: PlanetId,
    demands: Vec<PageDemand>,
    limits: Vec<TerrainStreamingLimit>,
    counters: TerrainStreamingCounters,
}

impl TerrainStreamingPlan {
    pub const fn planet_id(&self) -> PlanetId {
        self.planet_id
    }

    pub fn demands(&self) -> &[PageDemand] {
        &self.demands
    }

    pub fn limits(&self) -> &[TerrainStreamingLimit] {
        &self.limits
    }

    pub const fn counters(&self) -> TerrainStreamingCounters {
        self.counters
    }

    pub fn visible_count(&self) -> usize {
        self.demands
            .iter()
            .filter(|demand| demand.request_class == TerrainRequestClass::Visible)
            .count()
    }

    pub fn prefetch_count(&self) -> usize {
        self.demands.len().saturating_sub(self.visible_count())
    }

    pub fn is_face_balanced(&self) -> bool {
        let leaves = self
            .demands
            .iter()
            .map(|demand| demand.page_key())
            .collect::<BTreeSet<_>>();
        leaves.iter().all(|leaf| {
            faces().into_iter().all(|(axis, direction)| {
                covering_face_neighbor(*leaf, axis, direction, &leaves)
                    .is_none_or(|neighbor| leaf.lod.abs_diff(neighbor.lod) <= 1)
            })
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TerrainStreamingError {
    #[error("invalid terrain streaming configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("invalid planetary view: {0}")]
    InvalidView(&'static str),
    #[error("planet root LOD{0} cannot be represented by the page-address contract")]
    UnsupportedRootLod(u8),
    #[error("planet definition extends outside its centered sparse hierarchy root")]
    PlanetOutsideRoot,
    #[error("planetary page coordinate arithmetic overflowed")]
    CoordinateOverflow,
    #[error("page budget {available} cannot represent {required} visible root regions")]
    RootPageBudget { required: usize, available: usize },
}

#[derive(Clone, Copy, Debug)]
pub struct TerrainStreamingPlanner {
    config: TerrainStreamingConfig,
}

impl TerrainStreamingPlanner {
    pub fn new(config: TerrainStreamingConfig) -> Result<Self, TerrainStreamingError> {
        config.validate()?;
        Ok(Self { config })
    }

    pub const fn config(self) -> TerrainStreamingConfig {
        self.config
    }

    pub fn plan_fixed_sphere(
        &self,
        definition: &PlanetDefinition,
        view: PlanetView,
    ) -> Result<TerrainStreamingPlan, TerrainStreamingError> {
        let classifier = FixedSphereGenerator {
            center_cell: definition.center_cell,
            radius_cells: definition.radius_cells,
            material: definition.material,
        };
        self.plan_with_classifier(definition, view, &classifier)
    }

    pub fn plan_with_classifier<C: TerrainRegionClassifier>(
        &self,
        definition: &PlanetDefinition,
        view: PlanetView,
        classifier: &C,
    ) -> Result<TerrainStreamingPlan, TerrainStreamingError> {
        validate_planet_root(definition)?;
        let page_budget = self.config.max_pages.min(definition.max_resident_pages);
        let geometry = ViewGeometry::new(view, self.config)?;
        let mut state = PlannerState {
            classifier,
            geometry,
            config: self.config,
            root_lod: definition.root_lod,
            evaluations: HashMap::new(),
            counters: TerrainStreamingCounters::default(),
            limits: BTreeSet::new(),
        };

        let mut leaves = HashMap::new();
        let mut leaf_keys = HashSet::new();
        let root_child_lod = definition.root_lod - 1;
        for z in -1..=0 {
            for y in -1..=0 {
                for x in -1..=0 {
                    let key = PageKey::new(root_child_lod, [x, y, z]);
                    if let Evaluation::Leaf(info) = state.evaluate(key)? {
                        leaves.insert(key, info);
                        leaf_keys.insert(key);
                    }
                }
            }
        }
        if leaves.len() > page_budget {
            return Err(TerrainStreamingError::RootPageBudget {
                required: leaves.len(),
                available: page_budget,
            });
        }
        state.counters.page_high_water = leaves.len();

        let mut candidates = BinaryHeap::new();
        for (key, info) in &leaves {
            if info.should_refine && key.lod > 0 {
                candidates.push(Candidate::new(*key, *info));
            }
        }

        while let Some(candidate) = candidates.pop() {
            let Some(info) = leaves.get(&candidate.key).copied() else {
                continue;
            };
            if !info.should_refine || candidate.key.lod == 0 {
                continue;
            }
            if leaves.len() == page_budget {
                state.limits.insert(TerrainStreamingLimit::PageBudget);
                state.counters.deferred_refinements = state
                    .counters
                    .deferred_refinements
                    .saturating_add(candidates.len() + 1);
                break;
            }

            let mut refinements = BTreeSet::new();
            collect_balancing_refinements(
                candidate.key,
                state.root_lod,
                &leaf_keys,
                &mut refinements,
            );

            let mut replacements = Vec::new();
            let mut traversal_exhausted = false;
            for leaf in &refinements {
                for child in children(*leaf)? {
                    match state.evaluate(child)? {
                        Evaluation::Leaf(child_info) => replacements.push((child, child_info)),
                        Evaluation::Culled => {}
                        Evaluation::TraversalBudget => {
                            traversal_exhausted = true;
                            break;
                        }
                    }
                }
                if traversal_exhausted {
                    break;
                }
            }
            if traversal_exhausted {
                state.limits.insert(TerrainStreamingLimit::TraversalBudget);
                state.counters.deferred_refinements = state
                    .counters
                    .deferred_refinements
                    .saturating_add(candidates.len() + 1);
                break;
            }

            let next_len = leaves
                .len()
                .saturating_sub(refinements.len())
                .saturating_add(replacements.len());
            if next_len > page_budget {
                state.limits.insert(TerrainStreamingLimit::PageBudget);
                state.counters.deferred_refinements =
                    state.counters.deferred_refinements.saturating_add(1);
                continue;
            }

            for leaf in &refinements {
                leaves.remove(leaf);
                leaf_keys.remove(leaf);
            }
            for (child, child_info) in replacements {
                leaves.insert(child, child_info);
                leaf_keys.insert(child);
                if child_info.should_refine && child.lod > 0 {
                    candidates.push(Candidate::new(child, child_info));
                }
            }
            state.counters.refinements =
                state.counters.refinements.saturating_add(refinements.len());
            state.counters.balance_refinements = state
                .counters
                .balance_refinements
                .saturating_add(refinements.len().saturating_sub(1));
            state.counters.page_high_water = state.counters.page_high_water.max(leaves.len());
        }

        let mut demands = leaves
            .into_iter()
            .map(|(page_key, info)| PageDemand {
                page_key,
                request_class: info.request_class,
                projected_error_px: info.projected_error_px,
                distance_m: info.distance_m,
            })
            .collect::<Vec<_>>();
        demands.sort_by(|left, right| {
            request_priority(right.request_class)
                .cmp(&request_priority(left.request_class))
                .then_with(|| left.page_key.cmp(&right.page_key))
        });

        let plan = TerrainStreamingPlan {
            planet_id: definition.planet_id,
            demands,
            limits: state.limits.into_iter().collect(),
            counters: state.counters,
        };
        debug_assert!(plan.demands.len() <= page_budget);
        debug_assert!(plan.counters.traversed_nodes <= self.config.max_traversal_nodes);
        debug_assert!(plan.is_face_balanced());
        Ok(plan)
    }
}

#[derive(Clone, Copy, Debug)]
struct LeafInfo {
    request_class: TerrainRequestClass,
    projected_error_px: f64,
    distance_m: f64,
    should_refine: bool,
    interaction_forced: bool,
}

#[derive(Clone, Copy, Debug)]
enum Evaluation {
    Leaf(LeafInfo),
    Culled,
    TraversalBudget,
}

struct PlannerState<'a, C> {
    classifier: &'a C,
    geometry: ViewGeometry,
    config: TerrainStreamingConfig,
    root_lod: u8,
    evaluations: HashMap<PageKey, Option<LeafInfo>>,
    counters: TerrainStreamingCounters,
    limits: BTreeSet<TerrainStreamingLimit>,
}

impl<C: TerrainRegionClassifier> PlannerState<'_, C> {
    fn evaluate(&mut self, key: PageKey) -> Result<Evaluation, TerrainStreamingError> {
        if let Some(cached) = self.evaluations.get(&key) {
            return Ok(cached.map_or(Evaluation::Culled, Evaluation::Leaf));
        }
        if self.counters.traversed_nodes >= self.config.max_traversal_nodes {
            return Ok(Evaluation::TraversalBudget);
        }
        self.counters.traversed_nodes = self.counters.traversed_nodes.saturating_add(1);
        let info = if self.classifier.classify_region(key)? == TerrainRegion::Surface {
            self.geometry.relevance(key)?
        } else {
            None
        };
        self.evaluations.insert(key, info);
        Ok(info.map_or(Evaluation::Culled, Evaluation::Leaf))
    }
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    key: PageKey,
    request_priority: u8,
    interaction_forced: bool,
    error_bits: u64,
}

impl Candidate {
    fn new(key: PageKey, info: LeafInfo) -> Self {
        Self {
            key,
            request_priority: request_priority(info.request_class),
            interaction_forced: info.interaction_forced,
            error_bits: info.projected_error_px.to_bits(),
        }
    }
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.request_priority == other.request_priority
            && self.interaction_forced == other.interaction_forced
            && self.error_bits == other.error_bits
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            self.request_priority,
            self.interaction_forced,
            self.error_bits,
            self.key.lod,
            self.key,
        )
            .cmp(&(
                other.request_priority,
                other.interaction_forced,
                other.error_bits,
                other.key.lod,
                other.key,
            ))
    }
}

#[derive(Clone, Copy, Debug)]
struct ViewGeometry {
    camera: PlanetPosition,
    current: Frustum,
    predicted: Frustum,
    motion_m: [f64; 3],
    interaction_radius_m: f64,
    target_projected_error_px: f64,
    focal_length_px: f64,
}

impl ViewGeometry {
    fn new(
        view: PlanetView,
        config: TerrainStreamingConfig,
    ) -> Result<Self, TerrainStreamingError> {
        let right = normalize(cross(view.forward, view.up)).ok_or(
            TerrainStreamingError::InvalidView("view basis became degenerate"),
        )?;
        let up = cross(right, view.forward);
        let aspect = f64::from(view.viewport_px[0]) / f64::from(view.viewport_px[1]);
        let tan_vertical = (view.vertical_fov_radians * 0.5).tan();
        let tan_horizontal = tan_vertical * aspect;
        let focal_length_px = f64::from(view.viewport_px[1]) / (2.0 * tan_vertical);
        let motion_m = view
            .velocity_mps
            .map(|axis| axis * config.prediction_seconds);
        let current = Frustum {
            right,
            up,
            forward: view.forward,
            tan_horizontal,
            tan_vertical,
            near_m: view.near_m,
            far_m: view.far_m,
            camera_offset_m: [0.0; 3],
        };
        let predicted = Frustum {
            camera_offset_m: motion_m,
            ..current
        };
        Ok(Self {
            camera: view.camera,
            current,
            predicted,
            motion_m,
            interaction_radius_m: config.interaction_radius_m,
            target_projected_error_px: config.target_projected_error_px,
            focal_length_px,
        })
    }

    fn relevance(&self, key: PageKey) -> Result<Option<LeafInfo>, TerrainStreamingError> {
        let bounds = RelativeAabb::from_page(key, self.camera)?;
        let current_distance = bounds.distance_to_point([0.0; 3]);
        let predicted_distance = bounds.distance_to_point(self.motion_m);
        let swept_distance = bounds.distance_to_segment([0.0; 3], self.motion_m);
        let current_relevant =
            self.current.intersects(bounds) || current_distance <= self.interaction_radius_m;
        let predictive_relevant = self.motion_m != [0.0; 3]
            && (self.predicted.intersects(bounds) || swept_distance <= self.interaction_radius_m);
        if !current_relevant && !predictive_relevant {
            return Ok(None);
        }

        let request_class = if current_relevant {
            TerrainRequestClass::Visible
        } else {
            TerrainRequestClass::Prefetch
        };
        let distance_m = if current_relevant {
            current_distance
        } else {
            predicted_distance
        };
        let cell_size_m = LOD0_CELL_SIZE_METERS * 2_f64.powi(i32::from(key.lod));
        let error_distance = current_distance
            .min(predicted_distance)
            .max(cell_size_m * 0.5);
        let projected_error_px = self.focal_length_px * cell_size_m / error_distance;
        let interaction_forced = current_distance <= self.interaction_radius_m
            || swept_distance <= self.interaction_radius_m;
        Ok(Some(LeafInfo {
            request_class,
            projected_error_px,
            distance_m,
            should_refine: key.lod > 0
                && (interaction_forced || projected_error_px > self.target_projected_error_px),
            interaction_forced,
        }))
    }
}

#[derive(Clone, Copy, Debug)]
struct Frustum {
    right: [f64; 3],
    up: [f64; 3],
    forward: [f64; 3],
    tan_horizontal: f64,
    tan_vertical: f64,
    near_m: f64,
    far_m: f64,
    camera_offset_m: [f64; 3],
}

impl Frustum {
    fn intersects(self, bounds: RelativeAabb) -> bool {
        let center = sub(bounds.center(), self.camera_offset_m);
        let half = bounds.half_extent();
        let x = dot(center, self.right);
        let y = dot(center, self.up);
        let z = dot(center, self.forward);
        let extent_x = projected_extent(half, self.right);
        let extent_y = projected_extent(half, self.up);
        let extent_z = projected_extent(half, self.forward);
        if z + extent_z < self.near_m || z - extent_z > self.far_m {
            return false;
        }
        let horizontal_limit = z * self.tan_horizontal + extent_x + extent_z * self.tan_horizontal;
        let vertical_limit = z * self.tan_vertical + extent_y + extent_z * self.tan_vertical;
        x.abs() <= horizontal_limit.max(0.0) && y.abs() <= vertical_limit.max(0.0)
    }
}

#[derive(Clone, Copy, Debug)]
struct RelativeAabb {
    min: [f64; 3],
    max: [f64; 3],
}

impl RelativeAabb {
    fn from_page(key: PageKey, camera: PlanetPosition) -> Result<Self, TerrainStreamingError> {
        let min_cell = key
            .lod0_cell_min()
            .ok_or(TerrainStreamingError::CoordinateOverflow)?;
        let span = key
            .lod0_cell_span()
            .ok_or(TerrainStreamingError::CoordinateOverflow)?;
        let camera_cell = camera.lod0_cell();
        let camera_subcell = camera.subcell_m();
        let mut min = [0.0; 3];
        let mut max = [0.0; 3];
        for axis in 0..3 {
            let delta = min_cell[axis]
                .checked_sub(camera_cell[axis])
                .ok_or(TerrainStreamingError::CoordinateOverflow)?;
            min[axis] = delta as f64 * LOD0_CELL_SIZE_METERS - camera_subcell[axis];
            max[axis] = min[axis] + span as f64 * LOD0_CELL_SIZE_METERS;
        }
        Ok(Self { min, max })
    }

    fn center(self) -> [f64; 3] {
        std::array::from_fn(|axis| (self.min[axis] + self.max[axis]) * 0.5)
    }

    fn half_extent(self) -> [f64; 3] {
        std::array::from_fn(|axis| (self.max[axis] - self.min[axis]) * 0.5)
    }

    fn distance_to_point(self, point: [f64; 3]) -> f64 {
        squared_distance_to_aabb(point, self).sqrt()
    }

    fn distance_to_segment(self, start: [f64; 3], end: [f64; 3]) -> f64 {
        let direction = sub(end, start);
        if direction == [0.0; 3] {
            return self.distance_to_point(start);
        }

        let mut breaks = vec![0.0, 1.0];
        for axis in 0..3 {
            if direction[axis] != 0.0 {
                for boundary in [self.min[axis], self.max[axis]] {
                    let time = (boundary - start[axis]) / direction[axis];
                    if time > 0.0 && time < 1.0 {
                        breaks.push(time);
                    }
                }
            }
        }
        breaks.sort_by(f64::total_cmp);
        breaks.dedup_by(|left, right| left.to_bits() == right.to_bits());

        let mut best = f64::INFINITY;
        for interval in breaks.windows(2) {
            let low = interval[0];
            let high = interval[1];
            let middle = (low + high) * 0.5;
            let mut quadratic = 0.0;
            let mut linear = 0.0;
            for axis in 0..3 {
                let sample = start[axis] + direction[axis] * middle;
                let boundary = if sample < self.min[axis] {
                    Some(self.min[axis])
                } else if sample > self.max[axis] {
                    Some(self.max[axis])
                } else {
                    None
                };
                if let Some(boundary) = boundary {
                    let a = direction[axis];
                    let b = start[axis] - boundary;
                    quadratic += a * a;
                    linear += a * b;
                }
            }
            let candidate = if quadratic > 0.0 {
                (-linear / quadratic).clamp(low, high)
            } else {
                middle
            };
            for time in [low, candidate, high] {
                let point = add(start, direction.map(|axis| axis * time));
                best = best.min(squared_distance_to_aabb(point, self));
            }
        }
        best.sqrt()
    }
}

fn validate_planet_root(definition: &PlanetDefinition) -> Result<(), TerrainStreamingError> {
    if !(1..=58).contains(&definition.root_lod) {
        return Err(TerrainStreamingError::UnsupportedRootLod(
            definition.root_lod,
        ));
    }
    if !definition.fits_centered_root() {
        return Err(TerrainStreamingError::PlanetOutsideRoot);
    }
    Ok(())
}

fn children(parent: PageKey) -> Result<[PageKey; 8], TerrainStreamingError> {
    let lod = parent
        .lod
        .checked_sub(1)
        .ok_or(TerrainStreamingError::CoordinateOverflow)?;
    let base = [
        parent.page_xyz[0]
            .checked_mul(2)
            .ok_or(TerrainStreamingError::CoordinateOverflow)?,
        parent.page_xyz[1]
            .checked_mul(2)
            .ok_or(TerrainStreamingError::CoordinateOverflow)?,
        parent.page_xyz[2]
            .checked_mul(2)
            .ok_or(TerrainStreamingError::CoordinateOverflow)?,
    ];
    let mut output = [PageKey::default(); 8];
    for (index, child) in output.iter_mut().enumerate() {
        *child = PageKey::new(
            lod,
            [
                base[0] + (index & 1) as i64,
                base[1] + ((index >> 1) & 1) as i64,
                base[2] + ((index >> 2) & 1) as i64,
            ],
        );
    }
    Ok(output)
}

fn collect_balancing_refinements(
    leaf: PageKey,
    root_lod: u8,
    leaves: &HashSet<PageKey>,
    output: &mut BTreeSet<PageKey>,
) {
    if !output.insert(leaf) {
        return;
    }
    for (axis, direction) in faces() {
        if let Some(neighbor) = covering_face_neighbor(leaf, axis, direction, leaves) {
            if neighbor.lod > leaf.lod && neighbor.lod < root_lod {
                collect_balancing_refinements(neighbor, root_lod, leaves, output);
            }
        }
    }
}

fn covering_face_neighbor(
    leaf: PageKey,
    axis: usize,
    direction: i64,
    leaves: &impl PageKeySet,
) -> Option<PageKey> {
    let mut coordinate = leaf.page_xyz;
    coordinate[axis] = coordinate[axis].checked_add(direction)?;
    let mut candidate = PageKey::new(leaf.lod, coordinate);
    loop {
        if leaves.has_page(&candidate) {
            return Some(candidate);
        }
        candidate = candidate.parent()?;
        if candidate.lod > 57 {
            return None;
        }
    }
}

trait PageKeySet {
    fn has_page(&self, key: &PageKey) -> bool;
}

impl PageKeySet for BTreeSet<PageKey> {
    fn has_page(&self, key: &PageKey) -> bool {
        self.contains(key)
    }
}

impl PageKeySet for HashSet<PageKey> {
    fn has_page(&self, key: &PageKey) -> bool {
        self.contains(key)
    }
}

const fn faces() -> [(usize, i64); 6] {
    [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)]
}

const fn request_priority(class: TerrainRequestClass) -> u8 {
    match class {
        TerrainRequestClass::Visible => 1,
        TerrainRequestClass::Prefetch => 0,
        TerrainRequestClass::Collision => 2,
        TerrainRequestClass::EditResponse => 3,
    }
}

fn squared_distance_to_aabb(point: [f64; 3], bounds: RelativeAabb) -> f64 {
    (0..3)
        .map(|axis| {
            let delta = if point[axis] < bounds.min[axis] {
                bounds.min[axis] - point[axis]
            } else if point[axis] > bounds.max[axis] {
                point[axis] - bounds.max[axis]
            } else {
                0.0
            };
            delta * delta
        })
        .sum()
}

fn projected_extent(half: [f64; 3], axis: [f64; 3]) -> f64 {
    half[0] * axis[0].abs() + half[1] * axis[1].abs() + half[2] * axis[2].abs()
}

fn normalize(vector: [f64; 3]) -> Option<[f64; 3]> {
    let length_squared = dot(vector, vector);
    if !length_squared.is_finite() || length_squared <= f64::EPSILON {
        return None;
    }
    let inverse = length_squared.sqrt().recip();
    Some(vector.map(|axis| axis * inverse))
}

const fn cross(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    [
        left[1] * right[2] - left[2] * right[1],
        left[2] * right[0] - left[0] * right[2],
        left[0] * right[1] - left[1] * right[0],
    ]
}

fn dot(left: [f64; 3], right: [f64; 3]) -> f64 {
    left.iter()
        .zip(right)
        .map(|(left, right)| left * right)
        .sum()
}

fn add(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| left[axis] + right[axis])
}

fn sub(left: [f64; 3], right: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|axis| left[axis] - right[axis])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(radius_cells: u64, root_lod: u8, max_pages: usize) -> PlanetDefinition {
        PlanetDefinition {
            planet_id: PlanetId([7; 16]),
            center_cell: [0; 3],
            radius_cells,
            material: 3,
            root_lod,
            max_resident_pages: max_pages,
        }
    }

    fn view(camera: [i64; 3], forward: [f64; 3], velocity: [f64; 3]) -> PlanetView {
        PlanetView::new(
            PlanetPosition::from_lod0_cell(camera),
            forward,
            [0.0, 1.0, 0.0],
            60_f64.to_radians(),
            [1280, 720],
            0.1,
            20_000_000.0,
            velocity,
        )
        .unwrap()
    }

    #[test]
    fn fixed_sphere_classifier_prunes_uniform_space_conservatively() {
        let generator = FixedSphereGenerator {
            center_cell: [0; 3],
            radius_cells: 100,
            material: 1,
        };
        assert_eq!(
            generator
                .classify_region(PageKey::new(0, [20, 20, 20]))
                .unwrap(),
            TerrainRegion::UniformAir
        );
        assert_eq!(
            generator
                .classify_region(PageKey::new(0, [3, 0, 0]))
                .unwrap(),
            TerrainRegion::Surface
        );
        assert_eq!(
            generator
                .classify_region(PageKey::new(0, [-4, -1, -1]))
                .unwrap(),
            TerrainRegion::Surface
        );
        let large = FixedSphereGenerator {
            radius_cells: 10_000,
            ..generator
        };
        assert_eq!(
            large.classify_region(PageKey::new(0, [0, 0, 0])).unwrap(),
            TerrainRegion::UniformSolid
        );
    }

    #[test]
    fn ground_plan_is_deterministic_bounded_balanced_and_reaches_lod0() {
        let definition = definition(1_000, 6, 4_096);
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            target_projected_error_px: 2.0,
            prediction_seconds: 0.5,
            max_pages: 4_096,
            max_traversal_nodes: 65_536,
        })
        .unwrap();
        let view = view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]);
        let first = planner.plan_fixed_sphere(&definition, view).unwrap();
        let second = planner.plan_fixed_sphere(&definition, view).unwrap();
        assert_eq!(first, second);
        assert!(first.demands().len() <= 4_096);
        assert!(first.counters().traversed_nodes <= 65_536);
        assert!(first.is_face_balanced());
        assert!(first.limits().is_empty());
        assert_eq!(first.counters().deferred_refinements, 0);
        assert!(first
            .demands()
            .iter()
            .any(|demand| demand.page_key().lod == 0));
    }

    #[test]
    fn antipode_uses_negative_page_addresses_without_precision_collapse() {
        let definition = definition(1_000, 6, 4_096);
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            max_pages: 4_096,
            max_traversal_nodes: 65_536,
            ..TerrainStreamingConfig::default()
        })
        .unwrap();
        let plan = planner
            .plan_fixed_sphere(&definition, view([-1_000, 0, 0], [1.0, 0.0, 0.0], [0.0; 3]))
            .unwrap();
        assert!(plan.is_face_balanced());
        assert!(plan
            .demands()
            .iter()
            .any(|demand| { demand.page_key().lod == 0 && demand.page_key().page_xyz[0] < 0 }));
    }

    #[test]
    fn earth_orbit_plan_stays_strictly_bounded_and_coarse() {
        let radius = 63_710_000_u64;
        let definition = definition(radius, 22, 2_048);
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 64.0,
            target_projected_error_px: 2.0,
            prediction_seconds: 1.0,
            max_pages: 2_048,
            max_traversal_nodes: 131_072,
        })
        .unwrap();
        let camera = i64::try_from(radius).unwrap() + 40_000_000;
        let plan = planner
            .plan_fixed_sphere(
                &definition,
                view([camera, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]),
            )
            .unwrap();
        assert!(plan.demands().len() <= 2_048);
        assert!(plan.counters().traversed_nodes <= 131_072);
        assert!(plan.is_face_balanced());
        assert!(plan
            .demands()
            .iter()
            .all(|demand| demand.page_key().lod > 0));
    }

    #[test]
    fn page_pressure_defers_detail_without_breaking_balance_or_budget() {
        let definition = definition(1_000, 6, 32);
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 64.0,
            target_projected_error_px: 0.25,
            prediction_seconds: 0.0,
            max_pages: 32,
            max_traversal_nodes: 4_096,
        })
        .unwrap();
        let plan = planner
            .plan_fixed_sphere(&definition, view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]))
            .unwrap();
        assert!(plan.demands().len() <= 32);
        assert!(plan.limits().contains(&TerrainStreamingLimit::PageBudget));
        assert!(plan.counters().deferred_refinements > 0);
        assert!(plan.is_face_balanced());
    }

    #[test]
    fn swept_motion_generates_prefetch_demands_along_the_prediction_path() {
        let definition = definition(2_000, 7, 4_096);
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            target_projected_error_px: 4.0,
            prediction_seconds: 1.0,
            max_pages: 4_096,
            max_traversal_nodes: 65_536,
        })
        .unwrap();
        let plan = planner
            .plan_fixed_sphere(
                &definition,
                view([2_000, 0, 0], [-1.0, 0.0, 0.0], [0.0, 100.0, 0.0]),
            )
            .unwrap();
        assert!(plan.prefetch_count() > 0);
        assert!(plan.is_face_balanced());
    }

    #[test]
    fn traversal_pressure_is_explicit_and_preserves_a_valid_coarse_plan() {
        let definition = definition(1_000, 6, 4_096);
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 64.0,
            target_projected_error_px: 0.25,
            prediction_seconds: 0.0,
            max_pages: 4_096,
            max_traversal_nodes: 8,
        })
        .unwrap();
        let plan = planner
            .plan_fixed_sphere(&definition, view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]))
            .unwrap();
        assert_eq!(plan.counters().traversed_nodes, 8);
        assert!(plan
            .limits()
            .contains(&TerrainStreamingLimit::TraversalBudget));
        assert!(plan.demands().len() <= 8);
        assert!(plan.is_face_balanced());
    }

    #[test]
    fn teleport_rebuilds_a_bounded_plan_without_retaining_the_old_location() {
        let definition = definition(1_000, 6, 1_024);
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig {
            interaction_radius_m: 8.0,
            max_pages: 1_024,
            max_traversal_nodes: 32_768,
            ..TerrainStreamingConfig::default()
        })
        .unwrap();
        let ground = planner
            .plan_fixed_sphere(&definition, view([1_000, 0, 0], [-1.0, 0.0, 0.0], [0.0; 3]))
            .unwrap();
        let antipode = planner
            .plan_fixed_sphere(&definition, view([-1_000, 0, 0], [1.0, 0.0, 0.0], [0.0; 3]))
            .unwrap();
        assert!(ground.demands().len() <= 1_024);
        assert!(antipode.demands().len() <= 1_024);
        assert!(ground.is_face_balanced() && antipode.is_face_balanced());
        assert_ne!(ground.demands(), antipode.demands());
    }

    #[test]
    fn invalid_or_unrepresentable_contracts_fail_explicitly() {
        let planner = TerrainStreamingPlanner::new(TerrainStreamingConfig::default()).unwrap();
        let too_deep = definition(1, 59, 8_192);
        assert_eq!(
            planner.plan_fixed_sphere(&too_deep, view([0, 0, 10], [0.0, 0.0, -1.0], [0.0; 3])),
            Err(TerrainStreamingError::UnsupportedRootLod(59))
        );

        let outside = PlanetDefinition {
            center_cell: [10_000, 0, 0],
            ..definition(100, 4, 8_192)
        };
        assert_eq!(
            planner.plan_fixed_sphere(&outside, view([0, 0, 10], [0.0, 0.0, -1.0], [0.0; 3])),
            Err(TerrainStreamingError::PlanetOutsideRoot)
        );
    }
}
