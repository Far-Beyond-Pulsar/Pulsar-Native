//! The world of each voxel terrain, owned by its runtime.
//!
//! Every enabled terrain instance has a [`VoxelTerrainRuntime`], a
//! component SceneDB keeps beside the terrain (never saved): one world per
//! terrain, which every reader shares (block methods, the camera's ground,
//! the renderer). The world is built on the runtime's own worker thread,
//! never on the game or render thread: a journal that only grew extends a
//! copy of the world shown (its new edits counted for block events), one
//! whose latest edits were undone undoes them in a copy, anything else (a
//! new recipe, a loaded level) rebuilds it, restoring the chunks a save kept
//! of its edits. Until a build lands, readers see the last world built; a
//! source that does not build keeps it, and says why.
//!
//! [`VoxelWorlds`] is the scene subsystem that drives them: it keeps
//! journals on their ground and skies on their terrain
//! ([`crate::sync`]), asks each terrain's runtime for the world the
//! terrain describes, turns the edits a build applied into the terrain's
//! block events, and stamps sculpt samples whose picks were answered
//! ([`crate::sculpt`]).
//!
//! What the renderer draws is what the runtimes publish: every frame the
//! scene's voxel worlds go to the voxel pass as a payload
//! ([`helio_pass_voxel_planet::engine::VOXEL_FRAME`], registered below),
//! each with its look and what its runtime asks of the drawn world (picks
//! along rays, a save's kept chunks). The pass answers into the runtime's
//! inbox.

use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::{Duration, Instant};

use glam::{DVec3, Vec3};
use helio_pass_voxel_planet::chunks::KeptChunks;
use helio_pass_voxel_planet::edit_yield::EditYield;
use helio_pass_voxel_planet::engine::{
    PickHit, PickRequest, PlanetFrame, VOXEL_FRAME, VolumeAnswer, VolumeAnswers, VolumeFrame,
    VolumeRequests,
};
use helio_pass_voxel_planet::queries::WorldQueries;
use helio_pass_voxel_planet::terrain::material;
use helio_pass_voxel_planet::{Planet, VolumeTransform, VoxelVolume};
use helio_voxel_data::VoxelEditJournal;
use pulsar_scenedb::{Entity, World};

use crate::sync::VoxelWorldSync;
use crate::world::{WorldSource, entity_source, is_generated, journal_planet, planet_brush};
use crate::{BlockData, BlockMaterialChange, VoxelTerrainComponent};

/// Worker threads answering a world's brick queries.
const QUERY_THREADS: usize = 2;
/// Block events a terrain keeps for listeners that have not run yet (an
/// editor scene never ticks its components): past it the oldest go.
const MAX_PENDING_EVENTS: usize = 10_000;

/// The world a terrain describes: what it is made from and its journal.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldTarget {
    pub source: WorldSource,
    pub edits: VoxelEditJournal,
}

/// A built world: the target it was built for, the world, and the chunks a
/// save kept of its edits (restored by copy when it is drawn).
#[derive(Clone)]
pub struct BuiltWorld {
    pub target: WorldTarget,
    pub planet: Arc<Planet>,
    pub kept: Option<Arc<KeptChunks>>,
}

struct Request {
    target: WorldTarget,
    base: Option<Arc<BuiltWorld>>,
}

struct Landed {
    target: WorldTarget,
    built: Result<(BuiltWorld, Vec<EditYield>), String>,
}

#[derive(Default)]
struct State {
    shown: Option<Arc<BuiltWorld>>,
    /// The target being built.
    building: Option<WorldTarget>,
    /// Why the terrain has no world for what it describes: the target that
    /// failed to build (not built again), or `None` when the terrain's
    /// description itself is invalid.
    rejected: Option<(Option<WorldTarget>, String)>,
}

/// What the runtime asks of the drawn world, and the answers in hand.
#[derive(Default)]
struct Asked {
    /// Picks not answered yet, published with every frame.
    picks: Vec<PickRequest>,
    /// Answered picks not yet taken, by id.
    answered: Vec<(u64, Option<PickHit>)>,
    next_pick: u64,
    /// A save's request for the kept chunks, and the answer: the world
    /// they are valid for and the chunks.
    keep: Option<u64>,
    next_keep: u64,
    kept: Option<(u64, Arc<Planet>, KeptChunks)>,
}

/// One terrain's world and the worker that builds it. Readers share it
/// through the terrain's [`VoxelTerrainRuntime`].
pub struct TerrainWorld {
    state: Mutex<State>,
    requests: mpsc::Sender<Request>,
    results: Mutex<mpsc::Receiver<Landed>>,
    /// Brick queries over the world shown, made with the first world built.
    queries: OnceLock<WorldQueries>,
    /// What the drawn world is asked, and its inbox.
    asked: Mutex<Asked>,
    answers: Arc<VolumeAnswers>,
    strokes: Mutex<crate::sculpt::Strokes>,
    /// Brick walks wait for their bricks instead of answering later (tests).
    wait_for_bricks: std::sync::atomic::AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl TerrainWorld {
    fn new() -> Self {
        let (requests, inbox) = mpsc::channel::<Request>();
        let (outbox, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("voxel-world".into())
            .spawn(move || {
                // Ends with the runtime (its sender dropped).
                while let Ok(request) = inbox.recv() {
                    let built = build(&request.target, request.base.as_deref());
                    let landed = Landed {
                        target: request.target,
                        built,
                    };
                    if outbox.send(landed).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn the voxel world builder");
        Self {
            state: Mutex::default(),
            requests,
            results: Mutex::new(results),
            queries: OnceLock::new(),
            asked: Mutex::default(),
            answers: Arc::default(),
            strokes: Mutex::default(),
            wait_for_bricks: Default::default(),
        }
    }

    /// The terrain's sculpt strokes.
    pub(crate) fn strokes(&self) -> MutexGuard<'_, crate::sculpt::Strokes> {
        lock(&self.strokes)
    }

    /// Whether brick walks wait for their bricks ([`Self::set_wait_for_bricks`]).
    pub fn waits_for_bricks(&self) -> bool {
        self.wait_for_bricks
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Make brick walks (sculpt samples near the last stamp) wait for the
    /// bricks they cross instead of leaving the sample to the renderer's
    /// pick: for tests, which want an answer now.
    pub fn set_wait_for_bricks(&self, wait: bool) {
        self.wait_for_bricks
            .store(wait, std::sync::atomic::Ordering::Relaxed);
    }

    /// Ask the drawn world for the surface along a ray from a camera's eye
    /// (world frame): the renderer whose camera is at `origin` answers a
    /// frame or two later ([`Self::pick_answer`]).
    pub fn request_pick(&self, origin: DVec3, direction: DVec3) -> u64 {
        let mut asked = lock(&self.asked);
        asked.next_pick += 1;
        let id = asked.next_pick;
        asked.picks.push(PickRequest {
            id,
            origin,
            direction,
        });
        id
    }

    /// Pick `id`'s answer once it came (`Some(None)`: nothing drawn along
    /// the ray), taken.
    pub fn pick_answer(&self, id: u64) -> Option<Option<PickHit>> {
        self.take_answers();
        let mut asked = lock(&self.asked);
        let at = asked
            .answered
            .iter()
            .position(|(answered, _)| *answered == id)?;
        Some(asked.answered.swap_remove(at).1)
    }

    /// Stop asking pick `id` (no answer in time).
    pub fn forget_pick(&self, id: u64) {
        let mut asked = lock(&self.asked);
        asked.picks.retain(|p| p.id != id);
        asked.answered.retain(|(answered, _)| *answered != id);
    }

    /// Move the pass's answers into what is asked.
    fn take_answers(&self) {
        let answers = self.answers.take();
        if answers.is_empty() {
            return;
        }
        let mut asked = lock(&self.asked);
        for answer in answers {
            match answer {
                VolumeAnswer::Pick(result) => {
                    if asked.picks.iter().any(|p| p.id == result.id) {
                        asked.picks.retain(|p| p.id != result.id);
                        asked.answered.push((result.id, result.hit));
                    }
                }
                VolumeAnswer::Kept { id, planet, chunks } => {
                    if asked.keep == Some(id) {
                        asked.kept = Some((id, planet, chunks));
                    }
                }
            }
        }
    }

    /// What the drawn world is asked this frame, and where it answers.
    fn volume_requests(&self) -> VolumeRequests {
        self.take_answers();
        let asked = lock(&self.asked);
        VolumeRequests {
            picks: asked.picks.clone(),
            keep: asked.keep,
            answers: Some(self.answers.clone()),
        }
    }

    /// The drawn world's resident edited chunks for a save, with the
    /// journal they are valid for, waiting up to `timeout` for the renderer
    /// to pack them (`None`: nothing draws the terrain, or no answer in
    /// time). Call it off the render thread, holding no scene lock: frames
    /// must keep coming to answer.
    pub fn kept_chunks(&self, timeout: Duration) -> Option<(VoxelEditJournal, KeptChunks)> {
        let deadline = Instant::now() + timeout;
        let ask = |asked: &mut Asked| {
            asked.next_keep += 1;
            asked.keep = Some(asked.next_keep);
            asked.kept = None;
        };
        ask(&mut lock(&self.asked));
        loop {
            self.take_answers();
            let answer = {
                let mut asked = lock(&self.asked);
                match asked.kept.take() {
                    Some((_, planet, chunks)) => {
                        // For the world shown; another world since: ask again.
                        let built = self.shown().filter(|b| Arc::ptr_eq(&b.planet, &planet));
                        match built {
                            Some(built) => {
                                asked.keep = None;
                                Some((built.target.edits.clone(), chunks))
                            }
                            None => {
                                ask(&mut asked);
                                None
                            }
                        }
                    }
                    None => None,
                }
            };
            if answer.is_some() {
                return answer;
            }
            if Instant::now() >= deadline {
                lock(&self.asked).keep = None;
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The world shown: the last one built.
    pub fn world(&self) -> Option<Arc<Planet>> {
        lock(&self.state).shown.as_ref().map(|b| b.planet.clone())
    }

    /// The last world built, with what it was built for.
    pub fn shown(&self) -> Option<Arc<BuiltWorld>> {
        lock(&self.state).shown.clone()
    }

    /// Why the terrain's latest description has no world (the world shown
    /// is an older one, if any).
    pub fn error(&self) -> Option<String> {
        lock(&self.state)
            .rejected
            .as_ref()
            .map(|(_, error)| error.clone())
    }

    /// Whether a world is being built.
    pub fn building(&self) -> bool {
        lock(&self.state).building.is_some()
    }

    /// Whether anything is still on its way: a world building, sculpt
    /// samples or requests waiting on the renderer.
    pub fn settling(&self) -> bool {
        let asked = {
            let asked = lock(&self.asked);
            asked.keep.is_some() || !asked.picks.is_empty()
        };
        asked || self.building() || self.strokes().waiting()
    }

    /// Brick queries over the world shown (exact cells and ray walks near
    /// a focus, answered by worker threads); `None` before the first world.
    pub fn queries(&self) -> Option<&WorldQueries> {
        self.queries.get()
    }

    /// Ask for the world `target` describes, unless it is shown, being built
    /// (the next step asks again if it changed meanwhile) or failed.
    fn want(&self, target: WorldTarget) {
        let mut state = lock(&self.state);
        if state.shown.as_ref().is_some_and(|b| b.target == target) {
            state.rejected = None;
            return;
        }
        if state.building.is_some()
            || state
                .rejected
                .as_ref()
                .is_some_and(|(t, _)| t.as_ref() == Some(&target))
        {
            return;
        }
        state.building = Some(target.clone());
        let base = state.shown.clone();
        if self.requests.send(Request { target, base }).is_err() {
            state.building = None;
            state.rejected = Some((None, "the voxel world builder stopped".into()));
        }
    }

    /// The terrain's description is invalid: the world shown stays.
    fn reject(&self, error: String) {
        lock(&self.state).rejected = Some((None, error));
    }

    /// Take the builds that landed: the edits each applied to its base, in
    /// order (`None`: nothing landed).
    fn receive(&self) -> Option<Vec<EditYield>> {
        let landed: Vec<Landed> = lock(&self.results).try_iter().collect();
        if landed.is_empty() {
            return None;
        }
        let mut yields = Vec::new();
        let mut state = lock(&self.state);
        for result in landed {
            state.building = None;
            match result.built {
                Ok((world, applied)) => {
                    state.rejected = None;
                    match self.queries.get() {
                        Some(queries) => queries.publish(world.planet.clone()),
                        None => {
                            let _ = self
                                .queries
                                .set(WorldQueries::new(world.planet.clone(), QUERY_THREADS));
                        }
                    }
                    state.shown = Some(Arc::new(world));
                    yields.extend(applied);
                }
                Err(error) => state.rejected = Some((Some(result.target), error)),
            }
        }
        Some(yields)
    }
}

/// The world `target` describes: `base` extended by the edits appended to
/// its journal (each one's yield counted against the world the edits before
/// it left), or with its latest edits undone, or built anew.
fn build(
    target: &WorldTarget,
    base: Option<&BuiltWorld>,
) -> Result<(BuiltWorld, Vec<EditYield>), String> {
    let started = Instant::now();
    let base = base.filter(|b| b.target.source == target.source);
    let edits = &target.edits;
    let (planet, how, kept, yields) = match base {
        Some(base) if edits.starts_with(&base.target.edits) => {
            let mut planet = (*base.planet).clone();
            let mut yields = Vec::new();
            for edit in edits.iter_from(base.target.edits.len()) {
                let (_, counted) = planet.apply_counted(planet_brush(edit))?;
                yields.push(counted);
            }
            (planet, "extended", base.kept.clone(), yields)
        }
        // An undo removes the latest edits: undone in the world while it
        // still can (`Edits::undoable`), not rebuilt from every edit.
        Some(base)
            if base.target.edits.starts_with(edits)
                && base.target.edits.len() - edits.len() <= base.planet.edits().undoable() =>
        {
            let mut planet = (*base.planet).clone();
            for _ in edits.len()..base.target.edits.len() {
                planet.undo().expect("within the undoable edits");
            }
            (planet, "undone", base.kept.clone(), Vec::new())
        }
        // A new world: the chunks a save kept of these edits restore by
        // copy (the drawn world follows extended and undone ones).
        _ => {
            let kept = edits.kept_chunks().and_then(|kept| {
                KeptChunks::from_bytes(&kept.bytes)
                    .map_err(|e| tracing::warn!("Voxel kept chunks not loaded: {e}"))
                    .ok()
            });
            let planet = journal_planet(target.source.grown(), edits)?;
            (planet, "rebuilt", kept.map(Arc::new), Vec::new())
        }
    };
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    if ms >= 20.0 {
        tracing::warn!("VOXEL_WORLD {how} in {ms:.1} ms ({} edits)", edits.len());
    }
    Ok((
        BuiltWorld {
            target: target.clone(),
            planet: Arc::new(planet),
            kept,
        },
        yields,
    ))
}

/// A terrain instance's world and its builder: a component SceneDB keeps on
/// the terrain's entity while the terrain is enabled. Never saved; a copy
/// of the scene (play in editor) builds its own.
pub struct VoxelTerrainRuntime(Arc<TerrainWorld>);

impl VoxelTerrainRuntime {
    /// The runtime of terrain instance `entity`, once the scene has stepped
    /// with it.
    pub fn of(world: &World, entity: Entity) -> Option<Arc<TerrainWorld>> {
        world
            .get::<VoxelTerrainRuntime>(entity)
            .map(|runtime| runtime.0.clone())
    }
}

/// The world a terrain instance describes, or why it has none.
pub(crate) fn entity_target(
    world: &World,
    entity: Entity,
    component: &VoxelTerrainComponent,
) -> Result<WorldTarget, String> {
    let source = entity_source(world, entity, component)?;
    let fingerprint = source.fingerprint();
    // Edits made on other ground are not this terrain's (the scene step
    // drops them from its journal: `voxel_sync::sync_edit_journals`).
    let edits = if component.edits.terrain() != 0 && component.edits.terrain() != fingerprint {
        let mut edits = VoxelEditJournal::default();
        edits.made_on(fingerprint);
        edits
    } else {
        component.edits.clone()
    };
    Ok(WorldTarget { source, edits })
}

/// Queue the block events of the edits a build applied to `entity`'s world.
fn queue_events(world: &mut World, entity: Entity, yields: Vec<EditYield>) {
    let mut broken = Vec::new();
    let mut placed = Vec::new();
    let mut changed = Vec::new();
    for counted in yields {
        for c in counted.changes {
            let block = |material: u32| BlockData {
                x: c.centre.x,
                y: c.centre.y,
                z: c.centre.z,
                material,
                count: c.count,
            };
            if c.before != material::AIR {
                broken.push(block(c.before));
            }
            if c.after != material::AIR {
                placed.push(block(c.after));
            }
            if c.before != material::AIR && c.after != material::AIR {
                changed.push(BlockMaterialChange {
                    x: c.centre.x,
                    y: c.centre.y,
                    z: c.centre.z,
                    previous_material: c.before,
                    material: c.after,
                    count: c.count,
                });
            }
        }
    }
    if broken.is_empty() && placed.is_empty() && changed.is_empty() {
        return;
    }
    let Some(mut component) = world.get_mut::<VoxelTerrainComponent>(entity) else {
        return;
    };
    fn keep<T>(pending: &mut Vec<T>, new: Vec<T>) {
        pending.extend(new);
        let excess = pending.len().saturating_sub(MAX_PENDING_EVENTS);
        pending.drain(..excess);
    }
    keep(&mut component.pending_block_broken, broken);
    keep(&mut component.pending_block_placed, placed);
    keep(&mut component.pending_block_material_changed, changed);
}

/// The scene subsystem that steps every voxel terrain's runtime
/// ([`Self::step`]); hosts install it into each scene they step
/// (`pulsar_world_registry::install_scene_subsystems`).
#[derive(Default)]
pub struct VoxelWorlds {
    sync: Option<VoxelWorldSync>,
    /// The scene revision the terrains were last read at.
    revision: Option<u64>,
}

impl VoxelWorlds {
    pub const NAME: &'static str = "pulsar.voxel.worlds";

    /// Keep journals and skies with their terrains, take the worlds that
    /// landed (queueing the block events of their new edits), and ask each
    /// terrain's runtime for the world it now describes. Cheap on a frame
    /// where neither the scene nor a build changed. Returns whether a world
    /// is still being built.
    pub fn step(&mut self, world: &mut World) -> bool {
        self.sync
            .get_or_insert_with(|| VoxelWorldSync::new(world))
            .poll(world);
        let runtimes: Vec<(Entity, Arc<TerrainWorld>)> = world
            .query::<&VoxelTerrainRuntime>()
            .map(|(entity, runtime)| (entity, runtime.0.clone()))
            .collect();
        let mut landed = false;
        for (entity, runtime) in &runtimes {
            if let Some(yields) = runtime.receive() {
                landed = true;
                queue_events(world, *entity, yields);
            }
        }
        if landed || self.revision != Some(world.revision()) {
            self.want(world);
            self.revision = Some(world.revision());
        }
        // Sculpt samples whose picks came back stamp, in order.
        for (entity, runtime) in &runtimes {
            if runtime.strokes().waiting() {
                crate::sculpt::resolve_strokes(world, *entity, runtime);
            }
        }
        world
            .query::<&VoxelTerrainRuntime>()
            .any(|(_, runtime)| runtime.0.building())
    }

    /// Give every enabled terrain a runtime and ask it for its world; drop
    /// the runtimes of terrains that are gone or disabled.
    fn want(&self, world: &mut World) {
        let terrains: Vec<(Entity, Result<WorldTarget, String>)> = world
            .query::<&VoxelTerrainComponent>()
            .filter(|(entity, c)| {
                c.enabled
                    && is_generated(c)
                    && pulsar_scene_model::attachments::is_enabled(world, *entity)
            })
            .map(|(entity, c)| (entity, entity_target(world, entity, c)))
            .collect();
        let orphans: Vec<Entity> = world
            .query::<&VoxelTerrainRuntime>()
            .map(|(entity, _)| entity)
            .filter(|entity| !terrains.iter().any(|(terrain, _)| terrain == entity))
            .collect();
        for entity in orphans {
            world.remove::<VoxelTerrainRuntime>(entity);
        }
        for (entity, target) in terrains {
            let runtime = match VoxelTerrainRuntime::of(world, entity) {
                Some(runtime) => runtime,
                None => {
                    let runtime = Arc::new(TerrainWorld::new());
                    world.insert(entity, VoxelTerrainRuntime(runtime.clone()));
                    runtime
                }
            };
            match target {
                Ok(target) => runtime.want(target),
                Err(error) => runtime.reject(error),
            }
        }
    }
}

impl pulsar_scenedb::Subsystem for VoxelWorlds {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn simulate_a(&mut self, world: &mut World, _witness: &pulsar_scenedb::gpu::SimulateA) {
        self.step(world);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::SceneSubsystemRegistration {
        origin: pulsar_plugins::origin!(),
        name: VoxelWorlds::NAME,
        install: |scene| scene.register_subsystem(VoxelWorlds::default()),
    }
}

/// The scene's voxel worlds as the voxel pass draws them
/// ([`VOXEL_FRAME`]): every enabled, visible terrain with a built world,
/// its look and its requests, and the direction towards the scene's sun.
/// `None` when it has none.
fn publish_frame(world: &World, problems: &mut Vec<String>) -> Option<Arc<PlanetFrame>> {
    let mut volumes = Vec::new();
    for (entity, terrain) in world.query::<&VoxelTerrainComponent>() {
        if !terrain.enabled || !pulsar_scene_model::attachments::is_enabled(world, entity) {
            continue;
        }
        let owner = pulsar_scene_model::attachments::owner_of(world, entity).unwrap_or(entity);
        if world
            .get::<pulsar_scene_model::Visibility>(owner)
            .is_some_and(|v| !v.visible)
        {
            continue;
        }
        let Some(runtime) = VoxelTerrainRuntime::of(world, entity) else {
            continue;
        };
        if let Some(error) = runtime.error() {
            problems.push(format!("voxel terrain {}: {error}", entity.bits()));
        }
        let Some(built) = runtime.shown() else {
            continue;
        };
        // No appearance JSON: the terrain generator's own materials.
        let appearance = match terrain.appearance_parameters.trim() {
            "" => None,
            json => serde_json::from_str(json)
                .map_err(|e| {
                    problems.push(format!(
                        "voxel terrain {}: invalid terrain appearance JSON: {e}",
                        entity.bits()
                    ))
                })
                .ok(),
        };
        volumes.push(VolumeFrame {
            // Centred on the origin (`crate::world::entity_source`), by the
            // terrain's entity.
            volume: VoxelVolume::new(
                entity.bits(),
                built.planet.clone(),
                VolumeTransform::IDENTITY,
            ),
            kept: built.kept.clone(),
            appearance,
            smoothing: terrain.smoothness,
            requests: runtime.volume_requests(),
        });
    }
    if volumes.is_empty() {
        return None;
    }
    Some(Arc::new(PlanetFrame {
        volumes,
        sun: scene_sun(world).unwrap_or(Vec3::new(0.35, 0.75, 0.45).normalize()),
    }))
}

/// Direction towards the scene's directional light, which the deferred
/// lighting multiplies by the terrain's traced visibility: the opposite of
/// the direction it travels, its owner's rotation of -Y. The light must be
/// lit the way the scene join lights it: enabled, and its owner visible.
pub fn scene_sun(world: &World) -> Option<Vec3> {
    // The level's suns by name (`crate::others`): voxel links no light type.
    let owner = crate::others::suns(world).into_iter().find_map(|sun| {
        let owner = pulsar_scene_model::attachments::owner_of(world, sun)?;
        world
            .get::<pulsar_scene_model::Visibility>(owner)
            .is_none_or(|visibility| visibility.visible)
            .then_some(owner)
    })?;
    let transform = pulsar_scene_model::hierarchy_transform::world_transform_of(world, owner)?;
    let rotation = glam::Quat::from_euler(
        glam::EulerRot::YXZ,
        transform.rotation[1].to_radians(),
        transform.rotation[0].to_radians(),
        transform.rotation[2].to_radians(),
    );
    (rotation * Vec3::Y).try_normalize()
}

pulsar_world_registry::inventory::submit! {
    pulsar_world_registry::ScenePayloadRegistration {
        origin: pulsar_plugins::origin!(),
        key: VOXEL_FRAME.key(),
        publish: |world, problems| {
            publish_frame(world, problems).map(|frame| frame as Arc<dyn std::any::Any + Send + Sync>)
        },
        settling: |world| {
            world
                .query::<&VoxelTerrainRuntime>()
                .any(|(_, runtime)| runtime.0.settling())
        },
        // Its sun samples rotate over blocks of pixels for TSR to integrate.
        temporal: true,
    }
}

/// Step `world`'s voxel terrains until every world they describe is built
/// (or failed), or `timeout` passes: for tools and tests that read a world
/// right after describing it. Returns whether they all landed.
pub fn wait_for_voxel_worlds(world: &mut World, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let mut worlds = VoxelWorlds::default();
    loop {
        if !worlds.step(world) {
            // One more step: a landed world may have queued events that
            // changed the scene, and so its target.
            if !worlds.step(world) {
                return true;
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VoxelTerrainComponent, VoxelTerrainLayersComponent, VoxelTerrainMaterial};
    use helio_pass_voxel_planet::engine::PickResult;
    use helio_pass_voxel_planet::{PlanetRecipe, TerrainSource};
    use helio_voxel_data::{VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape};

    /// The world `PlanetRecipe::default()` describes, as a terrain.
    fn planet_terrain() -> VoxelTerrainComponent {
        let mut terrain = VoxelTerrainComponent::planet(6_371_000.0);
        terrain.generator_parameters = String::new();
        terrain.voxel_size = 0.1;
        terrain.seed = TerrainSource::default().seed;
        terrain
    }

    fn scene_with(terrain: VoxelTerrainComponent) -> (World, Entity) {
        let mut world = World::new();
        let owner = world.spawn();
        let entity = pulsar_world_registry::attach_value(&mut world, owner, terrain).unwrap();
        (world, entity)
    }

    fn settle(world: &mut World) {
        assert!(wait_for_voxel_worlds(world, Duration::from_secs(300)));
    }

    fn shown(world: &World, entity: Entity) -> Arc<Planet> {
        VoxelTerrainRuntime::of(world, entity)
            .and_then(|r| r.world())
            .expect("a built world")
    }

    /// A ground method by name, as cameras and scripts call it.
    fn ground(world: &mut World, entity: Entity, method: &str, p: DVec3) -> Option<f64> {
        let mut args: Vec<Box<dyn std::any::Any>> =
            vec![Box::new(p.x), Box::new(p.y), Box::new(p.z)];
        let answer = world
            .call_component_method(
                entity,
                pulsar_scenedb::component_id::<VoxelTerrainComponent>(),
                method,
                &mut args,
            )
            .ok()??;
        answer.downcast::<f64>().ok().map(|v| *v)
    }

    fn dig(center: DVec3, radius: f64) -> VoxelBrushEdit {
        VoxelBrushEdit {
            center: center.to_array(),
            radius,
            shape: VoxelBrushShape::Sphere,
            op: VoxelBrushOp::Remove,
            material: 0,
            height: Default::default(),
        }
    }

    /// One world per terrain, built off the thread that asked: an edit is in
    /// the journal at once and in the world once its build lands (the old
    /// world shown meanwhile), with the blocks it broke as events.
    #[test]
    fn edits_extend_the_terrains_one_world_off_thread() {
        let (mut world, entity) = scene_with(planet_terrain());
        assert!(crate::world::terrain_world(&world, entity).is_err());
        settle(&mut world);
        let first = shown(&world, entity);
        let eye = first.surface_point(DVec3::new(0.2, 1.0, 0.3), 3.0);
        let target = first.raycast(eye, -eye.normalize(), 100.0).unwrap().cell;
        let at = first.grid().cell_center(target);
        crate::world::append_edits(&mut world, entity, vec![dig(at, 0.5)]).unwrap();
        assert!(Arc::ptr_eq(&first, &shown(&world, entity)), "not built yet");
        settle(&mut world);
        let dug = shown(&world, entity);
        assert_eq!(dug.material(target), 0);
        assert_ne!(first.material(target), 0, "old worlds stay as they were");
        let terrain = world.get::<VoxelTerrainComponent>(entity).unwrap();
        assert!(!terrain.pending_block_broken.is_empty());
        assert_eq!(terrain.edits.len(), 1);
    }

    /// A change that does not build keeps the last good world shown, says
    /// why, and is not built again until it changes.
    #[test]
    fn a_world_that_does_not_build_keeps_the_last_good_one() {
        use helio_pass_voxel_planet::feature_recipe::FeatureRecipe;
        let mut terrain = planet_terrain();
        terrain.features = crate::feature_spawns(&FeatureRecipe::earth());
        let (mut world, entity) = scene_with(terrain);
        settle(&mut world);
        let good = shown(&world, entity);
        assert_eq!(
            good.recipe().features,
            FeatureRecipe::earth().grow(0.1, TerrainSource::default().seed)
        );
        world
            .get_mut::<VoxelTerrainComponent>(entity)
            .unwrap()
            .features
            .reverse();
        settle(&mut world);
        let runtime = VoxelTerrainRuntime::of(&world, entity).unwrap();
        let error = runtime.error().expect("rejected");
        assert!(error.contains("widest spacing"), "{error}");
        assert!(Arc::ptr_eq(&good, &shown(&world, entity)));
        VoxelWorlds::default().step(&mut world);
        assert!(!runtime.building(), "not built again");
    }

    #[test]
    fn a_layers_component_configures_the_generator() {
        let mut terrain = planet_terrain();
        terrain.seed = 99;
        let (mut world, entity) = scene_with(terrain);
        let owner = pulsar_scene_model::attachments::owner_of(&world, entity).unwrap();
        let mut layers = VoxelTerrainLayersComponent::default();
        layers.stack.climate.snow_c = -7.5;
        layers.stack.layers[2].scale_km = 55.0;
        pulsar_world_registry::attach_value(&mut world, owner, layers).unwrap();
        settle(&mut world);
        let recipe = shown(&world, entity).recipe().clone();
        assert_eq!(
            recipe.terrain.generator,
            helio_pass_voxel_planet::layers::ID
        );
        assert_eq!(recipe.terrain.seed, 99);
        let settings: helio_pass_voxel_planet::layers::TerrainLayers =
            serde_json::from_str(&recipe.terrain.settings).unwrap();
        assert_eq!(settings.climate.snow_c, -7.5);
        assert_eq!(settings.layers[2].scale_km, 55.0);
    }

    #[test]
    fn plane_worlds_follow_the_component_and_its_settings() {
        let (mut world, entity) = scene_with(VoxelTerrainComponent::plane(1_024.0));
        let owner = pulsar_scene_model::attachments::owner_of(&world, entity).unwrap();
        let mut flat = VoxelTerrainLayersComponent::flat(12.0);
        flat.stack.surface = VoxelTerrainMaterial::Sand;
        pulsar_world_registry::attach_value(&mut world, owner, flat).unwrap();
        settle(&mut world);
        let planet = shown(&world, entity);
        assert!(planet.grid().is_plane());
        let hit = planet
            .raycast(DVec3::new(3.0, 40.0, -5.0), -DVec3::Y, 100.0)
            .unwrap();
        assert!((hit.distance - 28.0).abs() < 0.11, "{}", hit.distance);
        assert_eq!(planet.material(hit.cell), material::SAND);
        let eye = DVec3::new(10.0, 400.0, -20.0);
        let up = ["ground_up_x", "ground_up_y", "ground_up_z"]
            .map(|m| ground(&mut world, entity, m, eye).unwrap());
        assert_eq!(up, [0.0, 1.0, 0.0]);
    }

    /// The ground methods cameras call: altitude over the ground below,
    /// clip planes that follow the clear air, and a lift out of solid
    /// ground that leaves dug caves alone.
    #[test]
    fn ground_methods_answer_from_the_world_shown() {
        let (mut world, entity) = scene_with(planet_terrain());
        settle(&mut world);
        let planet = Planet::new(PlanetRecipe::default()).unwrap();
        let near = planet.surface_point(DVec3::Y, 2.0);
        let altitude = ground(&mut world, entity, "ground_altitude", near).unwrap();
        let orbit = ground(&mut world, entity, "ground_altitude", near * 1.05).unwrap();
        assert!(
            (altitude - 2.0).abs() < 0.2 && orbit > 250_000.0,
            "{altitude} {orbit}"
        );
        let high = planet.surface_point(DVec3::Y, 3000.0);
        let altitude = ground(&mut world, entity, "ground_altitude", high).unwrap();
        assert!((altitude - 3000.0).abs() < 1.0, "{altitude}");
        let far = near.normalize() * (planet.grid().radius() + 300_000.0);
        assert!(ground(&mut world, entity, "clip_near", far).unwrap() > 1_000.0);
        assert!(ground(&mut world, entity, "clip_far", far).unwrap() > far.length());

        let surface = planet.surface_point(DVec3::Y, 0.0);
        let buried = surface - DVec3::Y * 3.0;
        let lift = ground(&mut world, entity, "ground_lift", buried).unwrap();
        assert!((lift - 3.5).abs() < 0.2, "{lift}");
        assert_eq!(
            ground(&mut world, entity, "ground_lift", surface + DVec3::Y * 2.0),
            Some(0.0)
        );
        crate::world::append_edits(&mut world, entity, vec![dig(buried, 1.5)]).unwrap();
        settle(&mut world);
        assert_eq!(ground(&mut world, entity, "ground_lift", buried), Some(0.0));
    }

    /// Sculpt samples stamp where the renderer drew the surface under them
    /// (a pick answered into the runtime's inbox), or with no answer in time
    /// on the exact terrain; the stamps go through the journal.
    #[test]
    fn sculpt_samples_stamp_where_the_ray_hits() {
        use crate::sculpt::{VoxelBrushRequest, VoxelBrushTool, sculpt};
        let (mut world, entity) = scene_with(planet_terrain());
        settle(&mut world);
        let runtime = VoxelTerrainRuntime::of(&world, entity).unwrap();
        runtime.set_wait_for_bricks(true);
        let planet = shown(&world, entity);
        let eye = planet.surface_point(DVec3::new(0.2, 1.0, 0.3), 3.0);
        let down = -eye.normalize();
        let exact = planet.raycast(eye, down, 100.0).unwrap();
        let brush = VoxelBrushRequest {
            op: VoxelBrushOp::Remove,
            shape: VoxelBrushShape::Sphere,
            radius: 0.05,
            material: 0,
            single_block: false,
            tool: VoxelBrushTool::Stamp,
        };
        // Drawn far from the eye: the renderer's hit is the exact cell.
        let remote = eye - down * 300_000.0;
        sculpt(&mut world, entity, remote, down, brush, true).unwrap();
        let pick = lock(&runtime.asked).picks[0];
        runtime.answers.push(VolumeAnswer::Pick(PickResult {
            id: pick.id,
            hit: Some(PickHit {
                volume: entity.bits(),
                distance: 300_000.0 + exact.distance,
                cell: exact.cell,
                level: 0,
                entered: Some(4),
                material: 0,
            }),
        }));
        VoxelWorlds::default().step(&mut world);
        let edits = world
            .get::<VoxelTerrainComponent>(entity)
            .unwrap()
            .edits
            .clone();
        assert_eq!(edits.len(), 1);
        settle(&mut world);
        assert_eq!(shown(&world, entity).material(exact.cell), 0);
        // No answer in time: the exact terrain within reach; the next sample
        // near it stamps at once, into the hole the first made.
        sculpt(&mut world, entity, eye, down, brush, true).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        VoxelWorlds::default().step(&mut world);
        let first = world
            .get::<VoxelTerrainComponent>(entity)
            .unwrap()
            .edits
            .len();
        assert_eq!(first, 2);
        sculpt(&mut world, entity, eye, down, brush, false).unwrap();
        let next = world
            .get::<VoxelTerrainComponent>(entity)
            .unwrap()
            .edits
            .clone();
        assert!(next.len() > first, "stamped at once");
        let [a, b] = [first - 1, next.len() - 1].map(|n| next.listed().nth(n).unwrap().center);
        assert!(
            DVec3::from_array(a).distance(eye) + 0.05 < DVec3::from_array(b).distance(eye),
            "deeper"
        );
    }
}
