//! Sculpt strokes: brush samples along rays from a camera's eye, stamped
//! where the ray hits the terrain as drawn.
//!
//! [`sculpt`] (the editor's sculpt tool, a script's `sculpt_stroke`) takes
//! one sample of a stroke. Near the stroke's last stamp it stamps at once,
//! walking the exact terrain's bricks around the eye; elsewhere it asks the
//! renderer for the surface drawn under the ray (a pick the terrain's
//! runtime publishes to the voxel pass) and stamps when the answer comes,
//! in order, on a later scene step ([`resolve_strokes`]); a sample with no
//! answer in time walks the exact terrain within a bounded reach. Stamps of
//! one stroke fill the gaps between samples, so a fast drag carves a
//! continuous trench. Every stamp is an edit through
//! [`append_edits`](super::voxel_world::append_edits).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use glam::DVec3;
use helio_pass_voxel_planet::engine::PickHit;
use helio_pass_voxel_planet::queries::Resident;
use helio_pass_voxel_planet::terrain::material;
use helio_pass_voxel_planet::{Matter, Planet, RayHit};
use helio_voxel_data::{VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape};
use pulsar_scenedb::{Entity, World};

use super::VoxelTerrainComponent;
use super::voxel_runtime::{TerrainWorld, VoxelTerrainRuntime};
use super::voxel_world::{append_edits, planet_brush};

/// One sculpt stroke sample's brush.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelBrushRequest {
    /// Remove digs into the hit block; Add builds on the face in front of
    /// it; Paint recolours solid blocks.
    pub op: VoxelBrushOp,
    pub shape: VoxelBrushShape,
    /// Brush radius (half size for a cube) in metres.
    pub radius: f32,
    /// Terrain material for Add and Paint.
    pub material: u32,
    /// Edit exactly one block, whatever the radius.
    pub single_block: bool,
    /// What a stamp does with the brush (see [`VoxelBrushTool`]).
    pub tool: VoxelBrushTool,
}

/// What a sculpt stamp does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VoxelBrushTool {
    /// The brush itself, with its op (dig, build, paint).
    #[default]
    Stamp,
    /// Level the ground under the brush to the height where the stroke
    /// started: carve above it, fill below it.
    Flatten,
    /// Ease the ground under the brush to its local average height.
    Smooth,
}

/// Where a brush ray hits the terrain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VoxelRayHint {
    /// The renderer's hit for this ray: what the user aimed at.
    Drawn(PickHit),
    /// No renderer hit (nothing drawn there, no answer in time): the exact
    /// terrain, walked this far.
    Reach(f64),
}

/// A stamp a brush ray makes: its edits and where.
#[derive(Clone, Debug)]
pub struct VoxelBrushCommit {
    pub distance: f64,
    pub edit: VoxelBrushEdit,
    /// Edits of the same stamp after `edit` (flatten and smooth: the fill
    /// below the level after the carve above it).
    pub then: Vec<VoxelBrushEdit>,
    /// The ground height (radial, m) the stamp leveled to or hit: a
    /// flatten stroke keeps its first.
    pub level: f64,
}

/// Without a renderer hit, an exact brush ray walks at most this far.
pub const UNPICKED_REACH_M: f64 = 500.0;
/// How long a brush sample waits for the renderer's hit before it walks the
/// exact terrain without one.
const PICK_TIMEOUT: Duration = Duration::from_millis(150);
/// After a stamp within this distance, samples stamp at once: the exact walk
/// from the eye to twice that distance costs under a millisecond, where a
/// renderer pick takes frames.
const INSTANT_STAMP_M: f64 = 40.0;
/// The query focus that follows a sculpting camera, and its radius: stamps
/// within it walk the world's bricks at once, farther ones take the pick.
const SCULPT_FOCUS: u64 = 1;
const SCULPT_FOCUS_M: f64 = 16.0;

/// A sample waiting for the renderer's hit under it.
struct Sample {
    pick: u64,
    requested: Instant,
    stroke: u64,
    origin: DVec3,
    direction: DVec3,
    brush: VoxelBrushRequest,
}

/// A terrain's strokes: the stroke in progress, its last stamp and level,
/// and samples waiting for picks. Kept by the terrain's runtime.
#[derive(Default)]
pub struct Strokes {
    stroke: u64,
    last: Option<(u64, VoxelBrushCommit)>,
    level: Option<(u64, f64)>,
    pending: VecDeque<Sample>,
}

impl Strokes {
    /// Whether samples wait for picks.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// One sample of a sculpt stroke on terrain `entity`: the brush stamps where
/// the ray from `origin` (a camera's eye) along `direction` hits the
/// terrain. `start` begins a stroke (pointer down).
pub fn sculpt(
    world: &mut World,
    entity: Entity,
    origin: DVec3,
    direction: DVec3,
    brush: VoxelBrushRequest,
    start: bool,
) -> Result<(), String> {
    let terrain = world
        .get::<VoxelTerrainComponent>(entity)
        .ok_or("the entity has no voxel terrain")?;
    if !terrain.editable {
        return Err("the voxel terrain is not editable".into());
    }
    let direction = direction
        .try_normalize()
        .filter(|_| origin.is_finite())
        .ok_or("a sculpt ray needs a finite origin and a direction")?;
    let runtime =
        VoxelTerrainRuntime::of(world, entity).ok_or("the voxel world is not built yet")?;
    let (stroke, near) = {
        let mut strokes = runtime.strokes();
        if start {
            strokes.stroke += 1;
        }
        let near = strokes
            .last
            .as_ref()
            .map(|(_, last)| last.distance)
            .filter(|&d| d <= INSTANT_STAMP_M && strokes.pending.is_empty());
        (strokes.stroke, near)
    };
    // Near the last stamp (any stroke's), with no older sample waiting (it
    // must land first): stamp now, walking the exact terrain from the eye
    // to twice that distance. A surface beyond it takes the renderer's pick.
    if let Some(distance) = near {
        let hint = VoxelRayHint::Reach(distance * 2.0);
        if stamp(
            world, entity, &runtime, stroke, origin, direction, brush, hint,
        )? {
            return Ok(());
        }
    }
    let pick = runtime.request_pick(origin, direction);
    runtime.strokes().pending.push_back(Sample {
        pick,
        requested: Instant::now(),
        stroke,
        origin,
        direction,
        brush,
    });
    Ok(())
}

/// Stamp the samples whose picks were answered (or timed out), in order:
/// where the renderer drew the surface under them. A sample over the sky
/// stamps nothing. The terrains' scene step runs it.
pub(crate) fn resolve_strokes(world: &mut World, entity: Entity, runtime: &TerrainWorld) {
    loop {
        let (hint, sample) = {
            let mut strokes = runtime.strokes();
            let Some(front) = strokes.pending.front() else {
                return;
            };
            let hint = match runtime.pick_answer(front.pick) {
                Some(Some(hit)) => Some(VoxelRayHint::Drawn(hit)),
                Some(None) => None,
                None if front.requested.elapsed() >= PICK_TIMEOUT => {
                    runtime.forget_pick(front.pick);
                    Some(VoxelRayHint::Reach(UNPICKED_REACH_M))
                }
                None => return,
            };
            (hint, strokes.pending.pop_front().expect("front checked"))
        };
        if let Some(hint) = hint {
            let applied = stamp(
                world,
                entity,
                runtime,
                sample.stroke,
                sample.origin,
                sample.direction,
                sample.brush,
                hint,
            );
            if let Err(error) = applied {
                tracing::warn!("Voxel sculpt: {error}");
            }
        }
    }
}

/// Stamp `brush` where the ray hits; returns whether it did.
#[allow(clippy::too_many_arguments)]
fn stamp(
    world: &mut World,
    entity: Entity,
    runtime: &TerrainWorld,
    stroke: u64,
    origin: DVec3,
    direction: DVec3,
    brush: VoxelBrushRequest,
    hint: VoxelRayHint,
) -> Result<bool, String> {
    let Some(planet) = runtime.world() else {
        return Ok(false);
    };
    // Flatten levels every stamp of a stroke to its first stamp's ground.
    let level = match brush.tool {
        VoxelBrushTool::Flatten => runtime
            .strokes()
            .level
            .filter(|(s, _)| *s == stroke)
            .map(|(_, level)| level),
        _ => None,
    };
    let Some(commit) = brush_commit(&planet, runtime, origin, direction, hint, brush, level)?
    else {
        return Ok(false);
    };
    let voxel = planet.grid().voxel_size();
    let fill = {
        let mut strokes = runtime.strokes();
        if brush.tool == VoxelBrushTool::Flatten && level.is_none() {
            strokes.level = Some((stroke, commit.level));
        }
        // Stamps fill the gap from the stroke's previous one; flatten and
        // smooth stamps overlap by their footprint.
        let fill = strokes
            .last
            .as_ref()
            .filter(|(s, _)| *s == stroke && brush.tool == VoxelBrushTool::Stamp)
            .map(|(_, last)| stroke_fill(&last.edit, &commit.edit, voxel))
            .unwrap_or_default();
        strokes.last = Some((stroke, commit.clone()));
        fill
    };
    let edits = fill
        .into_iter()
        .chain(std::iter::once(commit.edit))
        .chain(commit.then)
        .collect();
    append_edits(world, entity, edits)?;
    Ok(true)
}

/// The stamp a brush ray makes on `planet` (the world shown), if its ray
/// hits ([`VoxelRayHint`]). `level` is a flatten stroke's ground height.
fn brush_commit(
    planet: &Planet,
    runtime: &TerrainWorld,
    origin: DVec3,
    direction: DVec3,
    hint: VoxelRayHint,
    request: VoxelBrushRequest,
    level: Option<f64>,
) -> Result<Option<VoxelBrushCommit>, String> {
    let d = direction.normalize();
    let hit = match hint {
        VoxelRayHint::Drawn(pick) => Some(planet.drawn_hit(
            origin,
            d,
            pick.distance,
            pick.cell,
            pick.level,
            pick.entered,
        )),
        // The world's bricks around the sculpting camera: a ray past them
        // stamps nothing now (the renderer's pick takes it).
        VoxelRayHint::Reach(reach) => {
            let Some(queries) = runtime.queries() else {
                return Ok(None);
            };
            queries.focus(SCULPT_FOCUS, origin, SCULPT_FOCUS_M);
            let wait = runtime.waits_for_bricks();
            // Stamps show to reads a few milliseconds after they were
            // appended; a waiting runtime (tests) waits for them, as for
            // the bricks.
            while wait && queries.editing() > 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            loop {
                match queries.raycast(origin, d, reach, Matter::All) {
                    Resident::Ready(hit) => break hit,
                    Resident::Pending if wait => std::thread::sleep(Duration::from_millis(1)),
                    Resident::Pending => return Ok(None),
                }
            }
        }
    };
    let Some(hit) = hit else {
        return Ok(None);
    };
    let material = material::ID & request.material;
    if request.op != VoxelBrushOp::Remove && !(1..material::COUNT).contains(&material) {
        return Err(format!(
            "material {} is not a solid terrain material",
            request.material
        ));
    }
    // Building fills the empty block in front of the hit face.
    let cell = if request.op == VoxelBrushOp::Add {
        hit.previous
    } else {
        hit.cell
    };
    let grid = planet.grid();
    if request.tool != VoxelBrushTool::Stamp {
        return Ok(Some(level_stamp(planet, &hit, request, level, material)?));
    }
    let (radius, shape) = if request.single_block {
        (grid.voxel_size() * 0.5, VoxelBrushShape::Cube)
    } else {
        (
            f64::from(request.radius).max(grid.voxel_size() * 0.5),
            request.shape,
        )
    };
    let edit = VoxelBrushEdit {
        center: grid.cell_center(cell).to_array(),
        radius,
        shape,
        op: request.op,
        material: if request.op == VoxelBrushOp::Remove {
            0
        } else {
            material
        },
        height: Default::default(),
    };
    planet_brush(&edit).resolve(grid)?;
    Ok(Some(VoxelBrushCommit {
        distance: hit.distance,
        edit,
        then: Vec::new(),
        level: grid.layer_radius(f64::from(hit.cell.k + 1)),
    }))
}

/// A flatten or smooth stamp at `hit`: the ground under the brush's square
/// footprint levels to a layer boundary, carved above it and filled below,
/// a box each as tall as the brush. Flatten levels to `level` (the stroke's
/// first ground, this hit's ground for the first stamp); smooth to the
/// average ground height around the hit, filling with the ground's own
/// material.
fn level_stamp(
    planet: &Planet,
    hit: &RayHit,
    request: VoxelBrushRequest,
    level: Option<f64>,
    material: u32,
) -> Result<VoxelBrushCommit, String> {
    let grid = planet.grid();
    let voxel = grid.voxel_size();
    let centre = grid.cell_center(hit.cell);
    // The ground's height as a layer boundary (k: layers above the datum).
    let layer_of = |radial: f64| ((radial - grid.layer_radius(0.0)) / voxel).round();
    let hit_top = f64::from(hit.cell.k + 1);
    let k = match request.tool {
        VoxelBrushTool::Smooth => {
            let up = grid.up(centre);
            let side = up.any_orthonormal_vector();
            let ahead = up.cross(side);
            let r = f64::from(request.radius) * 0.7;
            let mut sum = 0.0;
            let mut n = 0.0;
            for (a, b) in [
                (0.0, 0.0),
                (1.0, 0.0),
                (-1.0, 0.0),
                (0.0, 1.0),
                (0.0, -1.0),
                (0.7, 0.7),
                (-0.7, 0.7),
                (0.7, -0.7),
                (-0.7, -0.7),
            ] {
                let ground = planet.surface_point(centre + (side * a + ahead * b) * r, 0.0);
                sum += layer_of(grid.radial(ground));
                n += 1.0;
            }
            (sum / n).round()
        }
        _ => level.map_or(hit_top, layer_of),
    };
    let level = grid.layer_radius(k);
    // Whole cells tall, so the boxes meet exactly on the level.
    let cells = (f64::from(request.radius) / voxel).round().max(1.0);
    let half = cells * voxel / 2.0;
    let fill_material = match request.tool {
        VoxelBrushTool::Smooth => match planet.material(hit.cell) {
            0 => material,
            m => m,
        },
        _ => material,
    };
    let at = |radial: f64| grid.at_radial(centre, radial).to_array();
    let carve = VoxelBrushEdit {
        center: at(level + half),
        radius: f64::from(request.radius).max(voxel * 0.5),
        shape: VoxelBrushShape::Cube,
        op: VoxelBrushOp::Remove,
        material: 0,
        height: half,
    };
    let fill = VoxelBrushEdit {
        center: at(level - half),
        op: VoxelBrushOp::Add,
        material: fill_material,
        ..carve
    };
    planet_brush(&carve).resolve(grid)?;
    planet_brush(&fill).resolve(grid)?;
    Ok(VoxelBrushCommit {
        distance: hit.distance,
        edit: carve,
        then: vec![fill],
        level,
    })
}

/// Stamps filling the gap between two consecutive samples of one stroke,
/// so a fast drag carves a continuous trench (or, with one-block cubes, a
/// continuous line of blocks) however few samples the frame rate allows.
/// Stamps are `spacing` apart (a fraction of the brush, never below a
/// voxel); the endpoints are not repeated. Samples of different brushes, or
/// farther apart than a plausible drag step (the pointer jumped to other
/// ground), start a new stroke segment instead.
pub fn stroke_fill(
    previous: &VoxelBrushEdit,
    next: &VoxelBrushEdit,
    voxel_size: f64,
) -> Vec<VoxelBrushEdit> {
    let same_brush = previous.op == next.op
        && previous.shape == next.shape
        && previous.material == next.material
        && (previous.radius - next.radius).abs() < 1e-9;
    let (a, b) = (
        DVec3::from_array(previous.center),
        DVec3::from_array(next.center),
    );
    let distance = a.distance(b);
    let spacing = (next.radius * 0.6).max(voxel_size);
    if !same_brush || distance <= spacing || distance > (next.radius * 16.0).max(voxel_size * 32.0)
    {
        return Vec::new();
    }
    let steps = (distance / spacing).ceil() as usize;
    (1..steps)
        .map(|k| VoxelBrushEdit {
            center: a.lerp(b, k as f64 / steps as f64).to_array(),
            ..*next
        })
        .collect()
}

/// The brush a script names: tool "dig", "build", "paint", "flatten" or
/// "smooth"; shape "sphere" or "cube".
fn script_brush(
    tool: &str,
    shape: &str,
    radius: f64,
    material: u32,
) -> Result<VoxelBrushRequest, String> {
    let (op, tool) = match tool.to_ascii_lowercase().as_str() {
        "dig" => (VoxelBrushOp::Remove, VoxelBrushTool::Stamp),
        "build" => (VoxelBrushOp::Add, VoxelBrushTool::Stamp),
        "paint" => (VoxelBrushOp::Paint, VoxelBrushTool::Stamp),
        "flatten" => (VoxelBrushOp::Add, VoxelBrushTool::Flatten),
        "smooth" => (VoxelBrushOp::Add, VoxelBrushTool::Smooth),
        other => {
            return Err(format!(
                "unknown sculpt tool {other:?} (dig, build, paint, flatten, smooth)"
            ));
        }
    };
    let shape = match shape.to_ascii_lowercase().as_str() {
        "sphere" => VoxelBrushShape::Sphere,
        "cube" => VoxelBrushShape::Cube,
        other => return Err(format!("unknown brush shape {other:?} (sphere, cube)")),
    };
    if !radius.is_finite() || radius <= 0.0 {
        return Err("a brush needs a positive radius".into());
    }
    Ok(VoxelBrushRequest {
        op,
        shape,
        radius: radius as f32,
        material,
        single_block: false,
        tool,
    })
}

// Sculpting for scripts: the same strokes the editor's sculpt tool makes.
#[pulsar_scenedb::component_methods]
impl VoxelTerrainComponent {
    /// One sample of a sculpt stroke along the ray from (x, y, z) towards
    /// (dx, dy, dz) (a camera's eye and view ray): `tool` is "dig",
    /// "build", "paint", "flatten" or "smooth", `shape` "sphere" or "cube";
    /// `start` begins a new stroke. The stamp lands where the ray hits the
    /// terrain as drawn, a few frames later.
    #[world_method(category = "Voxel|Sculpt")]
    #[allow(clippy::too_many_arguments)]
    fn sculpt_stroke(
        world: &mut World,
        entity: Entity,
        x: f64,
        y: f64,
        z: f64,
        dx: f64,
        dy: f64,
        dz: f64,
        tool: String,
        shape: String,
        radius: f64,
        material: u32,
        start: bool,
    ) -> Result<(), String> {
        let brush = script_brush(&tool, &shape, radius, material)?;
        sculpt(
            world,
            entity,
            DVec3::new(x, y, z),
            DVec3::new(dx, dy, dz),
            brush,
            start,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strokes_fill_gaps_between_samples_and_break_on_jumps() {
        let edit = |x: f64| VoxelBrushEdit {
            center: [x, 0.0, 0.0],
            radius: 1.0,
            shape: VoxelBrushShape::Sphere,
            op: VoxelBrushOp::Remove,
            material: 0,
            height: Default::default(),
        };
        // 3 m apart with 0.6 m spacing: stamps at 0.6 m steps between them.
        let fill = stroke_fill(&edit(0.0), &edit(3.0), 0.1);
        assert_eq!(fill.len(), 4);
        assert!(
            fill.windows(2)
                .all(|w| (w[1].center[0] - w[0].center[0] - 0.6).abs() < 1e-9)
        );
        assert!(
            stroke_fill(&edit(0.0), &edit(0.5), 0.1).is_empty(),
            "close samples need no fill"
        );
        assert!(
            stroke_fill(&edit(0.0), &edit(40.0), 0.1).is_empty(),
            "a jump starts a new segment"
        );
        let paint = VoxelBrushEdit {
            op: VoxelBrushOp::Paint,
            material: 5,
            ..edit(3.0)
        };
        assert!(
            stroke_fill(&edit(0.0), &paint, 0.1).is_empty(),
            "another brush starts a new segment"
        );
        // One-block cubes: a stamp per voxel.
        let block = |x: f64| VoxelBrushEdit {
            shape: VoxelBrushShape::Cube,
            radius: 0.05,
            op: VoxelBrushOp::Add,
            material: 13,
            ..edit(x)
        };
        assert_eq!(stroke_fill(&block(0.0), &block(1.0), 0.1).len(), 9);
    }
}
