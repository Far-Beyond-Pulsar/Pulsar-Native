//! Authored curves as a SceneDB World component.
//!
//! `SplineComponent` owns the curve: its control points, how they are
//! interpolated, and every edit operation the editor's spline tools perform.
//! It hydrates into a typed World value on its object's entity, so the curve
//! saves, loads and replicates with the level like every other component,
//! and the renderer draws it from the World in the editor's debug overlay
//! ([`spline_debug_lines`]) instead of the UI painting it over the viewport.

use engine_class_derive::{engine_class, register_world_component};
use glam::Vec3;
use pulsar_reflection::{ReflectError, ReflectResult, Reflectable, pulsar_type};
use serde::{Deserialize, Deserializer, Serialize};

pub const SPLINE_CLASS_NAME: &str = "SplineComponent";

/// Most control points a curve may hold.
pub const MAX_SPLINE_POINTS: usize = 4096;

/// How a curve passes through (or near) its control points.
#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq, Hash, Reflectable)]
pub enum CurveAlgorithm {
    /// Straight segments between points.
    Linear,
    /// Smooth curve through every point; tangents follow the neighbours.
    #[default]
    CatmullRom,
    /// Cubic Bézier; each point's handles shape the segments beside it.
    Bezier,
    /// Cubic Hermite; each point's tangents are used as authored.
    Hermite,
    /// Uniform cubic B-spline; smooth, approximates rather than touches the
    /// inner points.
    BSpline,
}

impl CurveAlgorithm {
    pub const ALL: [Self; 5] = [
        Self::Linear,
        Self::CatmullRom,
        Self::Bezier,
        Self::Hermite,
        Self::BSpline,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::CatmullRom => "CatmullRom",
            Self::Bezier => "Bezier",
            Self::Hermite => "Hermite",
            Self::BSpline => "BSpline",
        }
    }
}

/// Accepts the variant name (serde's form) and the variant index (the
/// reflection JSON form), as `ObjectMovability` does.
impl<'de> Deserialize<'de> for CurveAlgorithm {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Name(String),
            Index(u64),
        }
        let parsed = match Repr::deserialize(deserializer)? {
            Repr::Name(name) => Self::ALL
                .into_iter()
                .find(|a| a.name().eq_ignore_ascii_case(&name)),
            Repr::Index(index) => Self::ALL.get(index as usize).copied(),
        };
        parsed.ok_or_else(|| serde::de::Error::custom("unknown curve algorithm"))
    }
}

/// One control point, in the owning object's local space.
#[engine_class(no_register, default, clone, debug, serialize, deserialize)]
pub struct SplinePoint {
    #[property]
    pub position: [f32; 3],
    /// Incoming tangent (Bézier/Hermite).
    #[property]
    #[serde(default)]
    pub arrive: [f32; 3],
    /// Outgoing tangent (Bézier/Hermite).
    #[property]
    #[serde(default)]
    pub leave: [f32; 3],
}

impl SplinePoint {
    pub fn new(position: [f32; 3]) -> Self {
        Self {
            position,
            arrive: [0.; 3],
            leave: [0.; 3],
        }
    }
}

impl PartialEq for SplinePoint {
    fn eq(&self, other: &Self) -> bool {
        self.position == other.position && self.arrive == other.arrive && self.leave == other.leave
    }
}

fn serialize_spline_point_json(value: &SplinePoint) -> ReflectResult<serde_json::Value> {
    serde_json::to_value(value).map_err(|e| ReflectError::SerializationFailed(e.to_string()))
}

fn deserialize_spline_point_json(value: serde_json::Value) -> ReflectResult<SplinePoint> {
    serde_json::from_value(value).map_err(|e| ReflectError::DeserializationFailed(e.to_string()))
}

#[pulsar_type(
    serialize_json_with = serialize_spline_point_json,
    deserialize_json_with = deserialize_spline_point_json
)]
pub type RegisteredSplinePoint = SplinePoint;

/// A curve through control points in its object's local space.
#[engine_class(category = "Gameplay", clone, debug, serialize, deserialize)]
pub struct SplineComponent {
    #[property]
    #[serde(default)]
    pub points: Vec<SplinePoint>,
    #[property]
    #[serde(default)]
    pub algorithm: CurveAlgorithm,
    /// Joins the last point back to the first.
    #[property]
    #[serde(default)]
    pub closed: bool,
    /// Samples per segment when the curve is drawn or measured.
    #[property(min = 4.0, max = 128.0, step = 1.0)]
    #[serde(default = "default_resolution")]
    pub resolution: u32,
    /// Catmull-Rom tightness: 0 is the standard curve, 1 straight lines.
    #[property(min = 0.0, max = 1.0, step = 0.01)]
    #[serde(default)]
    pub tension: f32,
}

fn default_resolution() -> u32 {
    24
}

impl Default for SplineComponent {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            algorithm: CurveAlgorithm::CatmullRom,
            closed: false,
            resolution: default_resolution(),
            tension: 0.,
        }
    }
}

impl PartialEq for SplineComponent {
    fn eq(&self, other: &Self) -> bool {
        self.points == other.points
            && self.algorithm == other.algorithm
            && self.closed == other.closed
            && self.resolution == other.resolution
            && self.tension == other.tension
    }
}

impl SplineComponent {
    /// Whether every value is finite and the point count is within bounds,
    /// so the curve is safe to evaluate and store.
    pub fn is_valid(&self) -> bool {
        self.points.len() <= MAX_SPLINE_POINTS
            && self.tension.is_finite()
            && self.points.iter().all(|p| {
                p.position
                    .iter()
                    .chain(&p.arrive)
                    .chain(&p.leave)
                    .all(|v| v.is_finite())
            })
    }

    pub fn segment_count(&self) -> usize {
        if self.points.len() < 2 {
            0
        } else if self.closed {
            self.points.len()
        } else {
            self.points.len() - 1
        }
    }

    fn position(&self, index: isize) -> Vec3 {
        let n = self.points.len() as isize;
        let i = if self.closed {
            index.rem_euclid(n)
        } else {
            index.clamp(0, n - 1)
        };
        Vec3::from_array(self.points[i as usize].position)
    }

    /// The point at `t` in `[0, 1]` along the whole curve, in local space.
    pub fn evaluate(&self, t: f32) -> [f32; 3] {
        if self.points.is_empty() {
            return [0.; 3];
        }
        if self.points.len() == 1 {
            return self.points[0].position;
        }
        let t = t.clamp(0., 1.);
        if self.algorithm == CurveAlgorithm::BSpline && !self.closed {
            return self.de_boor(t).to_array();
        }
        let count = self.segment_count();
        let scaled = t * count as f32;
        let i = (scaled as usize).min(count - 1);
        let u = scaled - i as f32;
        let a = self.position(i as isize);
        let b = self.position(i as isize + 1);
        let prev = self.position(i as isize - 1);
        let next = self.position(i as isize + 2);
        let u2 = u * u;
        let u3 = u2 * u;
        let value = match self.algorithm {
            CurveAlgorithm::Linear => a.lerp(b, u),
            CurveAlgorithm::Bezier => {
                let p = a + Vec3::from_array(self.points[i].leave);
                let q = b - Vec3::from_array(self.points[(i + 1) % self.points.len()].arrive);
                a * (1. - u).powi(3)
                    + p * 3. * (1. - u).powi(2) * u
                    + q * 3. * (1. - u) * u2
                    + b * u3
            }
            CurveAlgorithm::CatmullRom | CurveAlgorithm::Hermite => {
                let (m0, m1) = if self.algorithm == CurveAlgorithm::CatmullRom {
                    (
                        (b - prev) * (1. - self.tension) * 0.5,
                        (next - a) * (1. - self.tension) * 0.5,
                    )
                } else {
                    (
                        Vec3::from_array(self.points[i].leave),
                        Vec3::from_array(self.points[(i + 1) % self.points.len()].arrive),
                    )
                };
                a * (2. * u3 - 3. * u2 + 1.)
                    + m0 * (u3 - 2. * u2 + u)
                    + b * (-2. * u3 + 3. * u2)
                    + m1 * (u3 - u2)
            }
            CurveAlgorithm::BSpline => {
                (prev * (1. - u).powi(3)
                    + a * (3. * u3 - 6. * u2 + 4.)
                    + b * (-3. * u3 + 3. * u2 + 3. * u + 1.)
                    + next * u3)
                    / 6.
            }
        };
        value.to_array()
    }

    // Open uniform B-spline with clamped end knots, degree capped at cubic.
    // Knots are computed, not stored, and the span is found directly, so a
    // sample costs the same whatever the point count (Pulsar-Native#995:
    // the knot vector and span scan made sampling quadratic in points).
    fn de_boor(&self, t: f32) -> Vec3 {
        let n = self.points.len();
        let degree = 3.min(n - 1);
        let interior = (n - degree) as f32;
        let knot = |i: usize| -> f32 {
            if i <= degree {
                0.
            } else if i >= n {
                1.
            } else {
                (i - degree) as f32 / interior
            }
        };
        // The span k with knot(k) <= t < knot(k + 1), clamped to the last
        // one at t = 1.
        let span = (degree + (t * interior) as usize).min(n - 1);
        let mut d = [Vec3::ZERO; 4];
        for (j, slot) in d.iter_mut().enumerate().take(degree + 1) {
            *slot = self.position((span - degree + j) as isize);
        }
        for r in 1..=degree {
            for j in (r..=degree).rev() {
                let i = span - degree + j;
                let denominator = knot(i + degree - r + 1) - knot(i);
                let alpha = if denominator > 0. {
                    (t - knot(i)) / denominator
                } else {
                    0.
                };
                d[j] = d[j - 1].lerp(d[j], alpha);
            }
        }
        d[degree]
    }

    /// The curve as a polyline: `resolution` samples per segment.
    pub fn samples(&self) -> Vec<[f32; 3]> {
        if self.points.len() < 2 {
            return self.points.iter().map(|p| p.position).collect();
        }
        let count = (self.segment_count() * self.resolution.clamp(4, 128) as usize).min(32768);
        (0..=count)
            .map(|i| self.evaluate(i as f32 / count as f32))
            .collect()
    }

    pub fn total_length_m(&self) -> f32 {
        self.samples()
            .windows(2)
            .map(|p| Vec3::from(p[0]).distance(Vec3::from(p[1])))
            .sum()
    }

    pub fn auto_tangents(&mut self) {
        let factor = if self.algorithm == CurveAlgorithm::Bezier {
            1. / 6.
        } else {
            0.5
        };
        let tangents: Vec<_> = (0..self.points.len())
            .map(|i| {
                ((self.position(i as isize + 1) - self.position(i as isize - 1))
                    * factor
                    * (1. - self.tension))
                    .to_array()
            })
            .collect();
        for (p, t) in self.points.iter_mut().zip(tangents) {
            p.arrive = t;
            p.leave = t;
        }
    }

    pub fn reverse(&mut self) {
        self.points.reverse();
        for p in &mut self.points {
            let arrive = p.arrive;
            p.arrive = p.leave.map(|v| -v);
            p.leave = arrive.map(|v| -v);
        }
    }

    /// Split a segment. Manual cubic curves retain their exact shape.
    pub fn insert_at(&mut self, t: f32) -> Option<usize> {
        let count = self.segment_count();
        if count == 0 || self.points.len() >= MAX_SPLINE_POINTS {
            return None;
        }
        let scaled = t.clamp(0., 1.) * count as f32;
        let i = (scaled as usize).min(count - 1);
        let u = (scaled - i as f32).clamp(0.001, 0.999);
        let j = (i + 1) % self.points.len();
        let mut p = SplinePoint::new(self.evaluate((i as f32 + u) / count as f32));
        if matches!(
            self.algorithm,
            CurveAlgorithm::Bezier | CurveAlgorithm::Hermite
        ) {
            let factor = if self.algorithm == CurveAlgorithm::Hermite {
                3.
            } else {
                1.
            };
            let a = Vec3::from(self.points[i].position);
            let b = Vec3::from(self.points[j].position);
            let h0 = a + Vec3::from(self.points[i].leave) / factor;
            let h1 = b - Vec3::from(self.points[j].arrive) / factor;
            let q0 = a.lerp(h0, u);
            let q1 = h0.lerp(h1, u);
            let q2 = h1.lerp(b, u);
            let r0 = q0.lerp(q1, u);
            let r1 = q1.lerp(q2, u);
            let mid = r0.lerp(r1, u);
            self.points[i].leave = ((q0 - a) * factor).to_array();
            self.points[j].arrive = ((b - q2) * factor).to_array();
            p.position = mid.to_array();
            p.arrive = ((mid - r0) * factor).to_array();
            p.leave = ((r1 - mid) * factor).to_array();
        }
        self.points.insert(i + 1, p);
        Some(i + 1)
    }

    pub fn smooth(&mut self, strength: f32) {
        let positions: Vec<_> = (0..self.points.len())
            .map(|i| {
                let p = self.position(i as isize);
                if !self.closed && (i == 0 || i + 1 == self.points.len()) {
                    p.to_array()
                } else {
                    p.lerp(
                        (self.position(i as isize - 1) + self.position(i as isize + 1)) * 0.5,
                        strength.clamp(0., 1.),
                    )
                    .to_array()
                }
            })
            .collect();
        for (p, v) in self.points.iter_mut().zip(positions) {
            p.position = v;
        }
        self.auto_tangents();
    }

    pub fn resample(&mut self, count: usize) {
        let samples = self.samples();
        if samples.len() < 2 {
            return;
        }
        let mut distance = vec![0.];
        for p in samples.windows(2) {
            distance.push(distance.last().unwrap() + Vec3::from(p[0]).distance(Vec3::from(p[1])));
        }
        let total = *distance.last().unwrap();
        if total <= f32::EPSILON {
            return;
        }
        let count = count.clamp(2, 512);
        let denominator = if self.closed { count } else { count - 1 };
        self.points = (0..count)
            .map(|i| {
                let target = total * i as f32 / denominator as f32;
                let j = distance
                    .partition_point(|d| *d < target)
                    .clamp(1, samples.len() - 1);
                let delta = distance[j] - distance[j - 1];
                let f = if delta > 0. {
                    (target - distance[j - 1]) / delta
                } else {
                    0.
                };
                SplinePoint::new(
                    Vec3::from(samples[j - 1])
                        .lerp(Vec3::from(samples[j]), f)
                        .to_array(),
                )
            })
            .collect();
        self.auto_tangents();
    }
}

// The World value is the curve; nothing is pushed anywhere else. The
// renderer reads it back through `spline_debug_lines`.
#[register_world_component]
impl SplineComponent {}

const CURVE_COLOR: [f32; 4] = [0.55, 0.62, 0.72, 1.0];
const SELECTED_CURVE_COLOR: [f32; 4] = [1.0, 0.72, 0.16, 1.0];
const CONTROL_POLYGON_COLOR: [f32; 4] = [1.0, 0.72, 0.16, 0.35];
const TANGENT_COLOR: [f32; 4] = [0.35, 0.85, 1.0, 0.8];

/// Every visible spline in `world` as world-space debug lines for the
/// editor overlay: the sampled curve, plus control points, control polygon
/// and tangent handles on selected splines.
pub fn spline_debug_lines(world: &pulsar_scenedb::World) -> Vec<helio::WorldVertex> {
    let mut lines = Vec::new();
    for (_, owner, spline) in
        pulsar_scene_model::attachments::enabled_components::<SplineComponent>(world)
    {
        if let Some((model, selected)) = owner_view(world, owner, &spline) {
            append_spline_lines(&mut lines, &spline, model, selected);
        }
    }
    lines
}

/// How `owner` shows its spline: its model matrix and whether it is
/// selected, or `None` when the curve is hidden, ownerless or invalid.
fn owner_view(
    world: &pulsar_scenedb::World,
    owner: pulsar_scenedb::Entity,
    spline: &SplineComponent,
) -> Option<(glam::DMat4, bool)> {
    if world
        .get::<pulsar_scene_model::Visibility>(owner)
        .is_some_and(|v| !v.visible)
        || !spline.is_valid()
    {
        return None;
    }
    // The spline's owner placed in the world, under any parents. Lines are
    // world positions in f64 (the overlay draws them relative to the frame's
    // origin, exact on a planet).
    let model = pulsar_scene_model::hierarchy_transform::world_matrix_of(world, owner)?;
    let selected = world.get::<pulsar_scene_model::Selected>(owner).is_some();
    Some((model, selected))
}

/// One spline instance's cached lines.
struct CachedSpline {
    owner: pulsar_scenedb::Entity,
    /// The curve sampled in local space; `None` once its points changed.
    samples: Option<Vec<[f32; 3]>>,
    /// Its world-space lines; `None` once it or its owner changed.
    lines: Option<Vec<helio::WorldVertex>>,
}

/// The editor's spline lines, rebuilt only when what they show changed.
///
/// Reads SceneDB's change journal (its own cursors, so no other reader is
/// affected) for spline values, attachment state, and the transforms,
/// visibility and selection of spline owners. Edits to anything else do not
/// rebuild the lines. Each curve keeps its samples and lines: moving an
/// owner re-transforms that curve's samples without evaluating it, and
/// other curves are copied as they are (Pulsar-Native#995).
pub struct SplineLines {
    cursors: Vec<pulsar_scenedb::ChangeCursor>,
    /// Each enabled spline instance at the last build.
    cache: std::collections::HashMap<pulsar_scenedb::Entity, CachedSpline>,
    /// The world revision at the last poll; a smaller one means the world
    /// was replaced, and the cursors with it.
    revision: u64,
    built: bool,
    scratch: Vec<pulsar_scenedb::ComponentChange>,
}

impl SplineLines {
    pub fn new(world: &pulsar_scenedb::World) -> Self {
        use pulsar_scene_model::{Selected, Transform, Visibility, attachments::ComponentOwner};
        Self {
            cursors: vec![
                world.open_change_cursor::<SplineComponent>(),
                world.open_change_cursor::<ComponentOwner>(),
                world.open_change_cursor::<Transform>(),
                world.open_change_cursor::<Visibility>(),
                world.open_change_cursor::<Selected>(),
            ],
            cache: Default::default(),
            revision: world.revision(),
            built: false,
            scratch: Vec::new(),
        }
    }

    /// The lines to draw, when they changed since the last poll.
    pub fn poll(&mut self, world: &pulsar_scenedb::World) -> Option<Vec<helio::WorldVertex>> {
        if world.revision() < self.revision {
            *self = Self::new(world);
        }
        self.revision = world.revision();
        // Curves whose points changed, owners that moved (or were shown,
        // hidden, selected), and whether the set of curves changed.
        let mut curves = std::collections::HashSet::new();
        let mut owners = std::collections::HashSet::new();
        let mut structure = !self.built;
        for (index, cursor) in self.cursors.iter_mut().enumerate() {
            self.scratch.clear();
            if world.read_changes(cursor, &mut self.scratch)
                == pulsar_scenedb::ChangeRead::Overflowed
            {
                structure = true;
            }
            for change in &self.scratch {
                match index {
                    0 => {
                        curves.insert(change.entity);
                    }
                    // Attached, detached, enabled or disabled.
                    1 => {
                        if self.cache.contains_key(&change.entity)
                            || world.get::<SplineComponent>(change.entity).is_some()
                        {
                            structure = true;
                        }
                    }
                    _ => {
                        if self.cache.values().any(|c| c.owner == change.entity) {
                            owners.insert(change.entity);
                        } else if world.get::<SplineComponent>(change.entity).is_some() {
                            structure = true;
                        }
                    }
                }
            }
        }
        if !structure && curves.is_empty() && owners.is_empty() {
            return None;
        }
        self.built = true;
        if structure {
            self.cache.clear();
        }
        for (instance, cached) in &mut self.cache {
            if curves.contains(instance) {
                cached.samples = None;
                cached.lines = None;
            } else if owners.contains(&cached.owner) {
                cached.lines = None;
            }
        }
        let mut lines = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for (instance, owner, spline) in
            pulsar_scene_model::attachments::enabled_components::<SplineComponent>(world)
        {
            seen.insert(instance);
            let cached = self.cache.entry(instance).or_insert(CachedSpline {
                owner,
                samples: None,
                lines: None,
            });
            if cached.owner != owner {
                cached.owner = owner;
                cached.lines = None;
            }
            if cached.lines.is_none() {
                let samples = cached.samples.get_or_insert_with(|| spline.samples());
                let mut built = Vec::new();
                if let Some((model, selected)) = owner_view(world, owner, &spline) {
                    append_sampled_lines(&mut built, &spline, samples, model, selected);
                }
                cached.lines = Some(built);
            }
            lines.extend_from_slice(cached.lines.as_deref().unwrap_or_default());
        }
        self.cache.retain(|instance, _| seen.contains(instance));
        Some(lines)
    }
}

fn append_spline_lines(
    lines: &mut Vec<helio::WorldVertex>,
    spline: &SplineComponent,
    model: glam::DMat4,
    selected: bool,
) {
    append_sampled_lines(lines, spline, &spline.samples(), model, selected);
}

/// `spline`'s lines, from its local-space `samples`.
fn append_sampled_lines(
    lines: &mut Vec<helio::WorldVertex>,
    spline: &SplineComponent,
    samples: &[[f32; 3]],
    model: glam::DMat4,
    selected: bool,
) {
    use glam::DVec3;
    let world_point = |p: [f32; 3]| model.transform_point3(Vec3::from_array(p).as_dvec3());
    let mut segment = |a: DVec3, b: DVec3, color: [f32; 4]| {
        for position in [a, b] {
            lines.push(helio::WorldVertex { position, color });
        }
    };
    let curve_color = if selected {
        SELECTED_CURVE_COLOR
    } else {
        CURVE_COLOR
    };
    let samples: Vec<DVec3> = samples.iter().copied().map(world_point).collect();
    for pair in samples.windows(2) {
        segment(pair[0], pair[1], curve_color);
    }
    if !selected {
        return;
    }
    let controls: Vec<DVec3> = spline
        .points
        .iter()
        .map(|p| world_point(p.position))
        .collect();
    for pair in controls.windows(2) {
        segment(pair[0], pair[1], CONTROL_POLYGON_COLOR);
    }
    if spline.closed && controls.len() > 2 {
        segment(
            controls[controls.len() - 1],
            controls[0],
            CONTROL_POLYGON_COLOR,
        );
    }
    // Control points as small crosses, sized to the curve's own scale.
    let extent = controls
        .iter()
        .fold(None::<(DVec3, DVec3)>, |acc, p| {
            Some(acc.map_or((*p, *p), |(lo, hi)| (lo.min(*p), hi.max(*p))))
        })
        .map_or(1.0, |(lo, hi)| (hi - lo).length());
    let r = (extent * 0.01).clamp(0.05, 0.5);
    for p in &controls {
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            segment(*p - axis * r, *p + axis * r, SELECTED_CURVE_COLOR);
        }
    }
    if matches!(
        spline.algorithm,
        CurveAlgorithm::Bezier | CurveAlgorithm::Hermite
    ) {
        for p in &spline.points {
            let at = Vec3::from_array(p.position);
            for handle in [
                at - Vec3::from_array(p.arrive),
                at + Vec3::from_array(p.leave),
            ] {
                segment(
                    world_point(p.position),
                    world_point(handle.to_array()),
                    TANGENT_COLOR,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> SplineComponent {
        let mut d = SplineComponent::default();
        d.points = vec![SplinePoint::new([0.; 3]), SplinePoint::new([3., 0., 4.])];
        d.auto_tangents();
        d
    }

    #[test]
    fn all_open_algorithms_preserve_endpoints() {
        for a in CurveAlgorithm::ALL {
            let mut d = line();
            d.algorithm = a;
            assert_eq!(d.evaluate(0.), [0.; 3]);
            assert_eq!(d.evaluate(1.), [3., 0., 4.]);
            assert!((d.total_length_m() - 5.).abs() < 0.001);
        }
    }

    #[test]
    fn closed_curves_join_at_seam() {
        for a in CurveAlgorithm::ALL {
            let mut d = line();
            d.points.push(SplinePoint::new([6., 0., 0.]));
            d.closed = true;
            d.algorithm = a;
            assert!(Vec3::from(d.evaluate(0.)).distance(Vec3::from(d.evaluate(1.))) < 0.001);
        }
    }

    #[test]
    fn reversing_bezier_preserves_geometry() {
        let mut d = line();
        d.algorithm = CurveAlgorithm::Bezier;
        d.points[0].leave = [0., 3., 0.];
        let original = d.clone();
        d.reverse();
        for i in 0..11 {
            let t = i as f32 / 10.;
            assert!(
                Vec3::from(d.evaluate(t)).distance(Vec3::from(original.evaluate(1. - t))) < 0.001
            );
        }
    }

    #[test]
    fn resampling_is_even_and_preserves_open_endpoints() {
        let mut d = line();
        d.resample(6);
        assert_eq!(d.points.len(), 6);
        for pair in d.points.windows(2) {
            assert!(
                (Vec3::from(pair[0].position).distance(Vec3::from(pair[1].position)) - 1.).abs()
                    < 0.001
            );
        }
    }

    #[test]
    fn repeated_points_remain_finite() {
        let mut d = line();
        d.points[1] = d.points[0].clone();
        for a in CurveAlgorithm::ALL {
            d.algorithm = a;
            d.resample(8);
            assert!(d.samples().iter().flatten().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn legacy_editor_json_still_loads() {
        // The shape the level editor stored under `editor_spline`.
        let json = serde_json::json!({
            "points": [{ "position": [1.0, 2.0, 3.0], "arrive": [0.0, 0.0, 0.0], "leave": [0.0, 0.0, 0.0] }],
            "algorithm": "Bezier",
            "closed": true,
            "resolution": 12,
            "tension": 0.25,
        });
        let spline: SplineComponent = serde_json::from_value(json).unwrap();
        assert_eq!(spline.algorithm, CurveAlgorithm::Bezier);
        assert_eq!(spline.points[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(spline.resolution, 12);
    }

    #[test]
    fn selected_splines_add_editing_lines() {
        let mut spline = line();
        spline.algorithm = CurveAlgorithm::Bezier;
        let mut plain = Vec::new();
        append_spline_lines(&mut plain, &spline, glam::DMat4::IDENTITY, false);
        let mut selected = Vec::new();
        append_spline_lines(&mut selected, &spline, glam::DMat4::IDENTITY, true);
        assert!(!plain.is_empty() && plain.len() % 2 == 0);
        assert!(selected.len() > plain.len());
    }

    /// The B-spline evaluation `de_boor` replaced (a knot vector and a span
    /// scan per sample), kept to check the new one against.
    fn reference_de_boor(spline: &SplineComponent, t: f32) -> Vec3 {
        let n = spline.points.len();
        let degree = 3.min(n - 1);
        let knots: Vec<f32> = (0..n + degree + 1)
            .map(|i| {
                if i <= degree {
                    0.
                } else if i >= n {
                    1.
                } else {
                    (i - degree) as f32 / (n - degree) as f32
                }
            })
            .collect();
        let span = (degree..n).find(|&k| t < knots[k + 1]).unwrap_or(n - 1);
        let mut d: Vec<Vec3> = (0..=degree)
            .map(|j| spline.position((span - degree + j) as isize))
            .collect();
        for r in 1..=degree {
            for j in (r..=degree).rev() {
                let i = span - degree + j;
                let denominator = knots[i + degree - r + 1] - knots[i];
                let alpha = if denominator > 0. {
                    (t - knots[i]) / denominator
                } else {
                    0.
                };
                d[j] = d[j - 1].lerp(d[j], alpha);
            }
        }
        d[degree]
    }

    #[test]
    fn open_bsplines_evaluate_as_before() {
        for points in [2, 3, 4, 5, 9, 64] {
            let spline = long_bspline(points);
            for i in 0..=1000 {
                let t = i as f32 / 1000.0;
                let new = Vec3::from_array(spline.evaluate(t));
                let old = reference_de_boor(&spline, t);
                assert!(
                    new.distance(old) < 1e-4,
                    "{points} points, t = {t}: {new} vs {old}"
                );
            }
        }
    }

    /// A long open B-spline (the costliest kind to sample) per scene, for
    /// measuring what dragging its owner costs (Pulsar-Native#995).
    fn long_bspline(points: usize) -> SplineComponent {
        let mut d = SplineComponent::default();
        d.algorithm = CurveAlgorithm::BSpline;
        d.resolution = 128;
        d.points = (0..points)
            .map(|i| SplinePoint::new([i as f32, (i as f32 * 0.3).sin() * 4.0, 0.0]))
            .collect();
        d
    }

    #[test]
    #[ignore = "perf report: cargo test -p helio_component --lib drag_cost -- --ignored --nocapture"]
    fn drag_cost_report() {
        use pulsar_scene_model::{SpawnObject, Transform, world_ext::SceneWorldExt};
        for &(splines, points) in &[(1usize, 64usize), (1, 256), (8, 256)] {
            let mut world = pulsar_scenedb::World::new();
            let mut owners = Vec::new();
            for i in 0..splines {
                let owner = world
                    .spawn_object(SpawnObject::new(format!("s{i}")))
                    .unwrap();
                pulsar_world_registry::attach_value(&mut world, owner, long_bspline(points))
                    .unwrap();
                owners.push(owner);
            }
            let mut lines = SplineLines::new(&world);
            lines.poll(&world).unwrap();
            let frames = 20;
            let start = std::time::Instant::now();
            for frame in 0..frames {
                world.get_mut::<Transform>(owners[0]).unwrap().position = [f64::from(frame), 0.0, 0.0];
                lines.poll(&world).expect("the owner moved");
            }
            let poll = start.elapsed() / frames;
            let spline = long_bspline(points);
            let start = std::time::Instant::now();
            let length = spline.total_length_m();
            let samples = spline.samples().len();
            let panel = start.elapsed();
            eprintln!(
                "{splines} spline(s) x {points} points: poll per drag frame {poll:?}; \
                 samples() + total_length_m() {panel:?} ({samples} samples, {length:.1} m)"
            );
        }
    }

    #[test]
    fn spline_lines_rebuild_only_when_a_spline_or_its_owner_changes() {
        use pulsar_scene_model::{SpawnObject, Transform, Visibility, world_ext::SceneWorldExt};
        let mut world = pulsar_scenedb::World::new();
        let curve = world.spawn_object(SpawnObject::new("curve")).unwrap();
        let bystander = world.spawn_object(SpawnObject::new("bystander")).unwrap();
        let spline = pulsar_world_registry::attach_value(&mut world, curve, line()).unwrap();
        // A second curve that never changes: its cached lines are reused.
        let still = world.spawn_object(SpawnObject::new("still")).unwrap();
        pulsar_world_registry::attach_value(&mut world, still, long_bspline(9)).unwrap();
        let mut inner = SplineLines::new(&world);
        // Every rebuild matches building the lines from scratch.
        let mut lines = Checked(&mut inner);
        struct Checked<'a>(&'a mut SplineLines);
        impl Checked<'_> {
            fn poll(&mut self, world: &pulsar_scenedb::World) -> Option<Vec<helio::WorldVertex>> {
                let lines = self.0.poll(world)?;
                let fresh = spline_debug_lines(world);
                let key = |l: &[helio::WorldVertex]| {
                    l.iter().map(|v| (v.position, v.color)).collect::<Vec<_>>()
                };
                assert_eq!(
                    key(&lines),
                    key(&fresh),
                    "cached lines match a full rebuild"
                );
                Some(lines)
            }
        }

        let first = lines.poll(&world).expect("the first poll builds");
        assert!(!first.is_empty());
        assert!(lines.poll(&world).is_none(), "nothing changed");

        world.get_mut::<Transform>(bystander).unwrap().position = [5.0, 0.0, 0.0];
        assert!(lines.poll(&world).is_none(), "an unrelated object moved");

        world.get_mut::<Transform>(curve).unwrap().position = [5.0, 0.0, 0.0];
        let moved = lines.poll(&world).expect("the owner moved");
        assert!(moved != first, "the owner moved");

        world.get_mut::<SplineComponent>(spline).unwrap().points[1].position = [0.0, 0.0, 9.0];
        assert!(lines.poll(&world).is_some(), "the curve changed");

        let shown = spline_debug_lines(&world).len();
        world.get_mut::<Visibility>(curve).unwrap().visible = false;
        let hidden = lines.poll(&world).expect("hidden").len();
        assert!(hidden < shown, "the hidden curve's lines are gone");
        world.get_mut::<Visibility>(curve).unwrap().visible = true;
        assert_eq!(
            lines.poll(&world).map(|l| l.len()),
            Some(shown),
            "shown again"
        );

        pulsar_scene_model::attachments::set_enabled(&mut world, spline, false);
        assert_eq!(
            lines.poll(&world).map(|l| l.len()),
            Some(hidden),
            "disabled"
        );
    }
}
