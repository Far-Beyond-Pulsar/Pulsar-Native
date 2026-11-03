//! Canonical editable voxel world (a planet or a plane): recipe, exact cell
//! queries and ray casts.
use crate::edit_store::{ColumnKey, EditStore, StoreUndo};
use crate::edits::{apply, center_half, Brush, EditLog, FaceBrush, ALL_LAYERS};
use crate::features::{self, CompiledFeatures, FeatureEval, FeatureSet, PlacementCache};
use crate::grid::{face_axes, Cell, Grid, Shape};
use crate::terrain::{self, material, Column, TerrainField, TerrainSource, HEIGHT_ONE};
use crate::volumes::{q2_center, Primitive, VolumeColumn, VolumeLayer, CLIFF_LEVELS, SITES};

use glam::DVec3;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

pub const RECIPE_VERSION: u32 = 2;
pub const EARTH_RADIUS: f64 = 6_371_000.0;

/// Serialized authoring recipe: the world's form and its terrain generator.
/// Edits are stored separately (see `journal`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlanetRecipe {
    pub version: u32,
    /// Sphere (planet), finite plane or infinite plane.
    pub shape: Shape,
    /// Planet radius (spheres).
    pub radius_m: f64,
    /// Edge length of a finite plane, centred on the origin.
    pub plane_size_m: f64,
    pub voxel_size_m: f64,
    pub terrain: TerrainSource,
    /// Procedural content standing on the terrain (`crate::features`).
    pub features: FeatureSet,
}

impl Default for PlanetRecipe {
    fn default() -> Self {
        Self {
            version: RECIPE_VERSION,
            shape: Shape::Sphere,
            radius_m: EARTH_RADIUS,
            plane_size_m: 4_096.0,
            voxel_size_m: 0.1,
            terrain: TerrainSource::default(),
            features: FeatureSet::default(),
        }
    }
}

impl PlanetRecipe {
    /// Fingerprint of the ground this recipe makes (form, voxel size,
    /// generator, seed and settings, the settings compared as JSON values):
    /// edits belong to it (`VoxelEditJournal::made_on`).
    pub fn fingerprint(&self) -> u64 {
        let settings: serde_json::Value =
            serde_json::from_str(&self.terrain.settings).unwrap_or(serde_json::Value::Null);
        // Only the size the shape uses.
        let (radius, plane) = match self.shape {
            Shape::Sphere => (self.radius_m, 0.0),
            _ => (0.0, self.plane_size_m),
        };
        let key = serde_json::json!([
            self.version,
            self.shape,
            radius,
            plane,
            self.voxel_size_m,
            self.terrain.generator,
            self.terrain.version,
            self.terrain.seed,
            settings,
            self.features
        ]);
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for byte in key.to_string().bytes() {
            h = (h ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
        // 0 means "no terrain yet".
        h.max(1)
    }
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("recipe serializes")
    }
    /// The grid of the world this recipe makes, without generating it: what
    /// an edit is resolved against before its world is built.
    pub fn grid(&self) -> Result<Grid, String> {
        match self.shape {
            Shape::Sphere => Grid::new(self.radius_m, self.voxel_size_m),
            shape => Grid::plane(shape, self.plane_size_m, self.voxel_size_m),
        }
    }
}

/// Result of an exact ray cast on the base grid.
/// The level column a walk is in (`Planet::kind_at`): its field height and
/// brushes, looked up once.
struct RayColumn {
    key: (u8, i32, i32, u32),
    volume: Arc<VolumeColumn>,
    large: Vec<FaceBrush>,
    recent: Vec<FaceBrush>,
}

/// Brushes kept exact before the oldest are sealed. Recent brushes draw
/// only at levels whose cells they span; sealed ones bake into cells whose
/// coarser levels show accumulated sculpting from any height, so the window
/// is short: a long recent window let a heavily dug area vanish from a few
/// hundred metres up.
pub const RECENT_BRUSHES: usize = 64;
/// Brushes sealed at a time.
const SEAL_BATCH: usize = 16;
/// Largest brush baked into base cells, radius in half base cells (32
/// cells): larger ones would bake millions of cells, and bake at their
/// [`bake_level`] instead.
pub const BAKE_RADIUS_HALF: u32 = 64;

/// The level a sealed large brush bakes at: the first whose cells it spans
/// at most [`BAKE_RADIUS_HALF`] half cells of. Below it the brush stays
/// analytic (`Edits::large`); there and above, baked cells hold it, so a
/// coarse chunk reads bricks, not every large brush over it (10000 digs 8 m
/// across cost a regenerated chunk 0.7 ms of brushes).
pub fn bake_level(fb: &FaceBrush) -> u32 {
    (0..31)
        .find(|&level| (fb.radius_half >> level) <= BAKE_RADIUS_HALF)
        .unwrap_or(31)
}
/// Seals kept undoable (the latest `UNDO_SEALS * SEAL_BATCH` sealed
/// brushes undo without rebuilding the world).
const UNDO_SEALS: usize = 128;
/// Baked column changes kept for renderers to catch up with.
const CHANGES_KEPT: usize = 8192;
/// ...and the columns they list, at most (about 12 MB): a decoration batch
/// changes tens of thousands of columns.
const CHANGES_COLUMNS_KEPT: usize = 1 << 20;

/// A world's edits in three layers, applied to the terrain in this order
/// (see `edit_store`):
///
/// 1. `large`: sealed brushes too large to bake into base cells, analytic
///    below their [`bake_level`];
/// 2. `baked`: every other sealed brush, as baked cell edits (with coarser
///    levels built from the base level, so accumulated small edits show at
///    any distance), and the large ones from their bake level up;
/// 3. `recent`: the latest brushes, exact.
///
/// Sealing keeps the order exact: a small brush bakes on top of the store;
/// a large one is applied to the cells already baked (it came after them),
/// baked over its bake level's cells, and joins `large`, which is applied
/// under the store. Every cell then reads `recent(baked(large(terrain)))`,
/// the brushes in their order.
///
/// Memory is bounded by the edits' effect, not their count: sealed brushes
/// leave only their baked cells (and large ones their analytic record), a
/// hash of the history, and for the latest seals what they replaced, so
/// they can be undone.
#[derive(Clone, Default)]
pub struct Edits {
    pub large: EditLog,
    pub baked: EditStore,
    pub recent: EditLog,
    /// History hash after each recent brush.
    recent_hash: Vec<u64>,
    /// Brushes sealed and the history hash after them: the baked state.
    sealed_count: usize,
    sealed_hash: u64,
    /// Shared by copies until one seals (a snapshot copies pointers).
    undo_seals: Arc<VecDeque<Arc<SealUndo>>>,
    changes: Arc<VecDeque<BakedChange>>,
}

/// A seal and what it replaced, to undo it.
#[derive(Clone)]
struct SealUndo {
    brushes: Vec<(Brush, u64)>,
    store: StoreUndo,
    /// The large brushes before the seal.
    large: EditLog,
    before: BakedState,
}

/// What the baked store holds: the brushes sealed and the history hash
/// after them. The store is a function of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct BakedState {
    pub sealed: usize,
    pub hash: u64,
}

/// A baked column a change touched, and the heights (`bk`) of the bricks it
/// held before the change or holds after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChangedColumn {
    pub key: ColumnKey,
    pub bricks: [i32; 2],
}

/// Baked columns a change touched, between two baked states. A seal or its
/// undo restates brushes renderers already follow (the world is the same);
/// a batch sealed at once changes the world (`world`).
#[derive(Clone)]
pub struct BakedChange {
    pub before: BakedState,
    pub after: BakedState,
    pub columns: Arc<[ChangedColumn]>,
    pub world: bool,
}

/// The baked columns changed since a state: restated, and changed in the
/// world (see [`BakedChange`]).
#[derive(Clone, Debug, Default)]
pub struct BakedChanges {
    pub restated: Vec<ChangedColumn>,
    pub world: Vec<ChangedColumn>,
}

impl Edits {
    /// Brushes applied (every layer).
    pub fn len(&self) -> usize {
        self.sealed_count + self.recent_hash.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Brushes sealed (the first `sealed_len` of the history).
    pub fn sealed_len(&self) -> usize {
        self.sealed_count
    }
    /// Brushes [`Planet::undo`] can still remove.
    pub fn undoable(&self) -> usize {
        self.recent_hash.len()
            + self
                .undo_seals
                .iter()
                .map(|u| u.brushes.len())
                .sum::<usize>()
    }
    /// Hash of the whole history: equal hashes, equal edits.
    pub fn hash(&self) -> u64 {
        self.recent_hash.last().copied().unwrap_or(self.sealed_hash)
    }
    /// Hash of the first `n` brushes, for `n` from [`Self::sealed_len`] to
    /// [`Self::len`] (only the sealed brushes' total is kept).
    pub fn prefix_hash(&self, n: usize) -> Option<u64> {
        match n.checked_sub(self.sealed_count)? {
            0 => Some(self.sealed_hash),
            r => self.recent_hash.get(r - 1).copied(),
        }
    }
    /// Brush `index` of the history, while it is recent.
    pub fn brush(&self, index: usize) -> Option<Brush> {
        let r = index.checked_sub(self.sealed_count)?;
        (r < self.recent.len()).then(|| self.recent.resolved(r as u32).brush)
    }
    /// The baked state (see [`BakedState`]).
    pub fn baked_state(&self) -> BakedState {
        BakedState {
            sealed: self.sealed_count,
            hash: self.sealed_hash,
        }
    }
    /// Baked columns changed since baked state `state`, or `None` when that
    /// state is not in the kept changes (long ago, or another history).
    pub fn baked_changes_since(&self, state: BakedState) -> Option<BakedChanges> {
        let mut out = BakedChanges::default();
        if state == self.baked_state() {
            return Some(out);
        }
        let start = self.changes.iter().rposition(|c| c.before == state)?;
        for change in self.changes.range(start..) {
            let into = if change.world {
                &mut out.world
            } else {
                &mut out.restated
            };
            into.extend(change.columns.iter().copied());
        }
        Some(out)
    }
    fn record(&mut self, brush: &Brush) {
        self.recent_hash
            .push(crate::edits::brush_hash(self.hash(), brush));
    }
    fn changed(&mut self, before: BakedState, columns: Vec<ChangedColumn>, world: bool) {
        let after = self.baked_state();
        let changes = Arc::make_mut(&mut self.changes);
        changes.push_back(BakedChange {
            before,
            after,
            columns: columns.into(),
            world,
        });
        let mut kept: usize = changes.iter().map(|c| c.columns.len()).sum();
        while changes.len() > 1 && (changes.len() > CHANGES_KEPT || kept > CHANGES_COLUMNS_KEPT) {
            if let Some(old) = changes.pop_front() {
                kept -= old.columns.len();
            }
        }
    }
    /// Edits from their layers (a snapshot): `large` and `baked` hold the
    /// first `sealed.0` brushes of a history with hash `sealed.1`, `recent`
    /// the rest. Nothing of it is undoable.
    pub(crate) fn from_parts(
        grid: &Grid,
        large: &[Brush],
        baked: EditStore,
        sealed: (usize, u64),
        recent: &[Brush],
    ) -> Result<Self, String> {
        let mut edits = Edits {
            large: Self::log(grid, large.iter().copied())?,
            baked,
            recent: Self::log(grid, recent.iter().copied())?,
            sealed_count: sealed.0,
            sealed_hash: sealed.1,
            ..Default::default()
        };
        for brush in recent {
            edits.record(brush);
        }
        Ok(edits)
    }
    /// A recent log holding `brushes`.
    fn log(grid: &Grid, brushes: impl Iterator<Item = Brush>) -> Result<EditLog, String> {
        let mut log = EditLog::default();
        for brush in brushes {
            log.push(grid, brush)?;
        }
        Ok(log)
    }
}

/// The columns a recorded change of `store` replaced, with their bricks'
/// heights before and after it.
fn changed_columns(change: &StoreUndo, store: &EditStore) -> Vec<ChangedColumn> {
    change
        .spans(store)
        .into_iter()
        .map(|(key, bricks)| ChangedColumn { key, bricks })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub cell: Cell,
    /// Last empty cell before the hit (for building).
    pub previous: Cell,
    pub distance: f64,
    /// Outward normal of the entered cell face (planet-centred frame).
    pub normal: DVec3,
}

/// A base cell as [`Planet::sample_brick`] gives it: its `kind` and
/// `material` as `Planet::sample_kind` (features included), and whether
/// its ground (`Matter::Ground`) is solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrickCell {
    pub kind: u32,
    pub material: u32,
    pub ground: bool,
}

/// How a ray's cell walk ended (`Planet::walk_cells`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Walked {
    Hit(RayHit),
    /// Out of the world, or past the ray's length, without a hit.
    Miss,
    /// Stopped this far along, where the walk's source could not tell.
    Stopped(f64),
}

/// What a CPU query of the world counts as solid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Matter {
    /// Everything drawn: the terrain, its edits and the features (trees,
    /// rocks, structures). Physics, picks and placement.
    All,
    /// The ground: the terrain and its edits, features passed through. An
    /// editor camera flies through a forest and stops at the ground.
    Ground,
}

pub struct Planet {
    /// Shared by copies (with features, hundreds of kilobytes).
    recipe: Arc<PlanetRecipe>,
    /// `record::recipe_fingerprint` of the recipe, once: a recipe with
    /// features serializes to hundreds of kilobytes (renderers compare it
    /// every frame).
    fingerprint: u64,
    grid: Grid,
    field: Arc<dyn TerrainField>,
    edits: Edits,
    revision: u64,
    /// Highest radius any add brush reaches.
    edit_top: f64,
    /// Lowest radius any remove brush reaches.
    edit_bottom: f64,
    /// Column heights and feature placements: functions of the terrain
    /// alone, so every copy of the world (a snapshot) shares them.
    heights: Arc<Mutex<rustc_hash::FxHashMap<(u8, i32, i32, u32), i32>>>,
    features: Arc<CompiledFeatures>,
    placements: Arc<Mutex<PlacementCache>>,
    /// The field's volume layer (`docs/volumes.md`), its regions' cave
    /// networks and the volume columns queries read (functions of the
    /// terrain alone, shared like the caches above).
    volumes: Option<VolumeLayer>,
    networks: Arc<Mutex<rustc_hash::FxHashMap<(u8, i32, i32), Arc<Vec<Primitive>>>>>,
    volume_columns: Arc<Mutex<rustc_hash::FxHashMap<(u8, i32, i32, u32), Arc<VolumeColumn>>>>,
}

/// A copy shares everything but what its edits change: the recipe, field,
/// features and caches by pointer, the edits' layers structurally
/// (`Edits`), so publishing a snapshot costs microseconds and its queries
/// find the caches warm.
impl Clone for Planet {
    fn clone(&self) -> Self {
        Self {
            recipe: Arc::clone(&self.recipe),
            fingerprint: self.fingerprint,
            grid: self.grid,
            field: Arc::clone(&self.field),
            edits: self.edits.clone(),
            revision: self.revision,
            edit_top: self.edit_top,
            edit_bottom: self.edit_bottom,
            heights: Arc::clone(&self.heights),
            features: Arc::clone(&self.features),
            placements: Arc::clone(&self.placements),
            volumes: self.volumes.clone(),
            networks: Arc::clone(&self.networks),
            volume_columns: Arc::clone(&self.volume_columns),
        }
    }
}

impl Planet {
    pub fn new(mut recipe: PlanetRecipe) -> Result<Self, String> {
        // Name the registered generator version (0 accepts it), so saved
        // edits record the terrain they were made against.
        if recipe.terrain.version == 0 {
            recipe.terrain.version = terrain::find(&recipe.terrain.generator, 0)
                .ok_or_else(|| format!("unknown terrain generator {}", recipe.terrain.generator))?
                .info()
                .version;
        }
        let grid = recipe.grid()?;
        let field = terrain::build(&recipe.terrain, &grid)?;
        let features = Arc::new(CompiledFeatures::new(
            &recipe.features,
            &grid,
            recipe.terrain.seed,
        )?);
        let volumes = field.volumes().filter(|v| v.caves() || v.cliffs()).cloned();
        Ok(Self {
            fingerprint: crate::record::recipe_fingerprint(&recipe),
            recipe: Arc::new(recipe),
            grid,
            field,
            edits: Edits::default(),
            revision: 0,
            edit_top: 0.0,
            edit_bottom: f64::INFINITY,
            heights: Default::default(),
            features,
            placements: Default::default(),
            volumes,
            networks: Default::default(),
            volume_columns: Default::default(),
        })
    }
    pub fn recipe(&self) -> &PlanetRecipe {
        &self.recipe
    }
    /// The recipe's fingerprint (`record::recipe_fingerprint`).
    pub fn recipe_fingerprint(&self) -> u64 {
        self.fingerprint
    }
    /// The world's features compiled (`crate::features`).
    pub fn features(&self) -> &CompiledFeatures {
        &self.features
    }
    /// What the features make of level cell `(i, j, k)` (`features::apply`):
    /// the near spawns' stamps over the far spawns' canopies.
    pub fn features_at(&self, level: u32, face: u8, i: i32, j: i32, k: i32) -> Option<Option<u32>> {
        if self.features.is_empty() {
            return None;
        }
        let top = |f: u8, i: i32, j: i32| {
            features::ground(
                &self.grid,
                &*self.field,
                f,
                i,
                j,
                0,
                self.column_height(f, i, j, 0),
            )
        };
        let near = {
            let mut cache = self.placements.lock().expect("placements");
            if cache.len() > 1 << 20 {
                cache.clear();
            }
            FeatureEval::new(&self.features, &self.grid, &*self.field, &top, &mut cache)
                .coarse_cell(face, level, i, j, k)
        };
        if near.is_some() {
            return near;
        }
        let ground = features::ground(
            &self.grid,
            &*self.field,
            face,
            i,
            j,
            level,
            self.column_height(face, i, j, level),
        );
        self.features
            .far_cover(&self.grid, &*self.field, face, level, i, j, ground)
            .and_then(|cover| CompiledFeatures::far_cell(level, k, cover))
    }
    /// Layers features stand over the ground at most.
    fn features_above(&self) -> i32 {
        self.features_above_of(Matter::All)
    }
    /// [`Self::features_above`] of `matter`: none when features pass.
    fn features_above_of(&self, matter: Matter) -> i32 {
        if self.features.is_empty() || matter == Matter::Ground {
            0
        } else {
            self.features.above
        }
    }
    pub fn grid(&self) -> &Grid {
        &self.grid
    }
    /// The terrain generator's field for this world's grid.
    pub fn field(&self) -> &dyn TerrainField {
        &*self.field
    }
    pub fn edits(&self) -> &Edits {
        &self.edits
    }
    /// Conservative radial bounds of the edits: the top of every addition
    /// and the bottom of every removal.
    pub(crate) fn edit_bounds(&self) -> (f64, f64) {
        (self.edit_top, self.edit_bottom)
    }
    /// Replace the edits (a loaded snapshot) with their bounds.
    pub(crate) fn restore_edits(&mut self, edits: Edits, (top, bottom): (f64, f64)) {
        self.edits = edits;
        self.edit_top = top;
        self.edit_bottom = bottom;
        self.revision += 1;
    }
    /// Increments with every applied or undone edit.
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn apply(&mut self, brush: Brush) -> Result<u32, String> {
        self.edits.recent.push(&self.grid, brush)?;
        self.edits.record(&brush);
        let id = (self.edits.len() - 1) as u32;
        if self.edits.recent.len() > RECENT_BRUSHES {
            self.seal(SEAL_BATCH)?;
        }
        // A cube brush reaches sqrt(3) radii from its centre.
        let reach = brush.radius * 1.7321 + self.grid.voxel_size();
        let centre = self.grid.radial(DVec3::from_array(brush.center));
        match brush.op {
            crate::edits::BrushOp::Add => self.edit_top = self.edit_top.max(centre + reach),
            crate::edits::BrushOp::Remove => {
                self.edit_bottom = self.edit_bottom.min(centre - reach)
            }
            _ => {}
        }
        self.revision += 1;
        Ok(id)
    }

    /// Bake `brushes` into the baked store in order (large ones over the
    /// cells already baked, at their bake level and into the large layer):
    /// what the store replaced.
    fn bake(&mut self, brushes: &[Brush]) -> Result<StoreUndo, String> {
        let resolved: Vec<Vec<FaceBrush>> = brushes
            .iter()
            .map(|b| b.resolve(&self.grid))
            .collect::<Result<_, _>>()?;
        let edits = &mut self.edits;
        edits.baked.begin_record();
        let mut changed = Vec::new();
        for (brush, faces) in brushes.iter().zip(&resolved) {
            if faces.iter().all(|fb| fb.radius_half <= BAKE_RADIUS_HALF) {
                for fb in faces {
                    changed.extend(edits.baked.bake(&self.grid, fb, brush.op));
                }
            } else {
                for fb in faces {
                    changed.extend(edits.baked.apply_over(&self.grid, fb, brush.op));
                    changed.extend(
                        edits
                            .baked
                            .bake_at(&self.grid, fb, brush.op, bake_level(fb)),
                    );
                }
                edits
                    .large
                    .push(&self.grid, *brush)
                    .expect("resolved above");
            }
        }
        for brush in brushes {
            self.reach(brush);
        }
        let edits = &mut self.edits;
        edits.baked.rebuild_coarse(changed, self.grid.levels());
        Ok(edits.baked.end_record())
    }

    /// The edits' bounds grown over `brush`.
    fn reach(&mut self, brush: &Brush) {
        // A cube brush reaches sqrt(3) radii from its centre.
        let reach = brush.radius * 1.7321 + brush.height + self.grid.voxel_size();
        let centre = self.grid.radial(DVec3::from_array(brush.center));
        match brush.op {
            crate::edits::BrushOp::Add => self.edit_top = self.edit_top.max(centre + reach),
            crate::edits::BrushOp::Remove => {
                self.edit_bottom = self.edit_bottom.min(centre - reach)
            }
            _ => {}
        }
    }

    /// Seal every recent brush now (a snapshot's or a benchmark's: the
    /// world is the same, its brushes baked).
    pub fn seal_all(&mut self) -> Result<(), String> {
        let n = self.edits.recent.len();
        if n > 0 {
            self.seal(n)?;
        }
        Ok(())
    }

    /// Undo the latest brush: a recent one, or one of the latest sealed ones
    /// (its seal is undone first); `None` past them
    /// ([`Edits::undoable`]).
    pub fn undo(&mut self) -> Option<Brush> {
        if self.edits.recent.is_empty() {
            self.unseal()?;
        }
        let brush = self.edits.recent.pop(&self.grid)?;
        self.edits.recent_hash.pop();
        self.revision += 1;
        Some(brush)
    }

    /// Seal the `n` oldest recent brushes (see [`Edits`]).
    fn seal(&mut self, n: usize) -> Result<(), String> {
        let brushes: Vec<Brush> = self.edits.recent.brushes().copied().collect();
        let n = n.min(brushes.len());
        let before = self.edits.baked_state();
        let large = self.edits.large.clone();
        let store = self.bake(&brushes[..n])?;
        let edits = &mut self.edits;
        let hashes: Vec<u64> = edits.recent_hash.drain(..n).collect();
        edits.sealed_count += n;
        edits.sealed_hash = *hashes.last().unwrap_or(&edits.sealed_hash);
        edits.recent = Edits::log(&self.grid, brushes[n..].iter().copied())?;
        let columns = changed_columns(&store, &edits.baked);
        let undo_seals = Arc::make_mut(&mut edits.undo_seals);
        undo_seals.push_back(Arc::new(SealUndo {
            brushes: brushes[..n].iter().copied().zip(hashes).collect(),
            store,
            large,
            before,
        }));
        if undo_seals.len() > UNDO_SEALS {
            undo_seals.pop_front();
        }
        edits.changed(before, columns, false);
        Ok(())
    }

    /// Undo the latest seal: its bricks and large brushes go back, its
    /// brushes become recent again.
    fn unseal(&mut self) -> Option<()> {
        let undo = Arc::make_mut(&mut self.edits.undo_seals).pop_back()?;
        let undo = Arc::try_unwrap(undo).unwrap_or_else(|shared| (*shared).clone());

        let edits = &mut self.edits;
        let before = edits.baked_state();
        let undone = undo.store.clone();
        edits.baked.restore(undo.store);
        edits.large = undo.large;
        let recent: Vec<Brush> = edits.recent.brushes().copied().collect();
        edits.recent = Edits::log(
            &self.grid,
            undo.brushes.iter().map(|(b, _)| *b).chain(recent),
        )
        .expect("brushes resolved when applied");
        edits
            .recent_hash
            .splice(0..0, undo.brushes.iter().map(|(_, h)| *h));
        (edits.sealed_count, edits.sealed_hash) = (undo.before.sealed, undo.before.hash);
        let columns = changed_columns(&undone, &edits.baked);
        edits.changed(before, columns, false);
        Some(())
    }
    /// Conservative bound on terrain surface height above the datum (m).
    pub fn max_terrain_height(&self) -> f64 {
        f64::from(self.field.height_range().1) / f64::from(HEIGHT_ONE)
    }
    pub fn min_terrain_height(&self) -> f64 {
        f64::from(self.field.height_range().0) / f64::from(HEIGHT_ONE)
    }
    /// Radius below which every cell is solid (terrain and removals).
    pub fn inner_radius(&self) -> f64 {
        let caves = f64::from(self.volume_bounds().0) / f64::from(HEIGHT_ONE);
        (self.grid.radius() + self.min_terrain_height() - caves - self.grid.voxel_size() * 4.0)
            .min(self.edit_bottom)
    }
    /// Outer radius that bounds every solid cell (terrain and additions).
    pub fn outer_radius(&self) -> f64 {
        (self.grid.radius()
            + self.max_terrain_height()
            + self.overhang_height()
            + f64::from(self.features_above() + 4) * self.grid.voxel_size())
        .max(self.edit_top + self.grid.voxel_size())
    }
    /// Largest rise (m) of generated overhangs over the heightfield.
    pub fn overhang_height(&self) -> f64 {
        f64::from(self.volume_bounds().1) / f64::from(HEIGHT_ONE)
    }
    /// Largest depth (mm) under the surface and height over it at which
    /// any level's cells may differ from the heightfield
    /// (`VolumeLayer::bounds`).
    pub fn volume_bounds(&self) -> (i32, i32) {
        self.volumes.as_ref().map_or((0, 0), VolumeLayer::bounds)
    }
    /// The world's volume layer (`docs/volumes.md`), if it has caves or
    /// cliff shapes.
    pub fn volumes(&self) -> Option<&VolumeLayer> {
        self.volumes.as_ref()
    }
    /// Region `(ri, rj)` of `face`'s cave network
    /// (`VolumeLayer::network`), its reference ground read from the field.
    pub fn cave_network(&self, face: u8, ri: i32, rj: i32) -> Arc<Vec<Primitive>> {
        let Some(v) = &self.volumes else {
            return Arc::default();
        };
        let key = (face, ri, rj);
        if let Some(n) = self.networks.lock().expect("networks").get(&key) {
            return Arc::clone(n);
        }
        let level = v.reference_level();
        let mut ground = [(0, 0); SITES];
        for (s, g) in v.sites(face, ri, rj).iter().zip(&mut ground) {
            if s.used {
                let (i, j) = (s.x >> level, s.y >> level);
                let height = self.column_height(face, i, j, level);
                let surface = self.field.surface(Column { face, i, j, level }, height) & 0xff;
                *g = (height, self.field.cave_gate(surface, height));
            }
        }
        let network = Arc::new(v.network(face, ri, rj, &ground));
        let mut cache = self.networks.lock().expect("networks");
        if cache.len() > 1 << 16 {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&network));
        network
    }
    /// What the volume layer reads of level column `(i, j)` once: its
    /// height, cliff shape and the cave primitives over it, and the cells
    /// below and above its top they may change (`tile_volumes_main`).
    pub fn volume_column(&self, face: u8, i: i32, j: i32, level: u32) -> Arc<VolumeColumn> {
        let key = (face, i, j, level);
        if let Some(c) = self
            .volume_columns
            .lock()
            .expect("volume columns")
            .get(&key)
        {
            return Arc::clone(c);
        }
        let height = self.column_height(face, i, j, level);
        let top = terrain::top_cells(&self.grid, height, level);
        let mut c = VolumeColumn {
            height,
            top,
            x: q2_center(i, level),
            y: q2_center(j, level),
            ..Default::default()
        };
        if let Some(v) = &self.volumes {
            let half = (1 << level) >> 1;
            let (bi, bj) = ((i << level) + half, (j << level) + half);
            if v.cliffs() && level < CLIFF_LEVELS {
                let slope = self.material_slope(face, bi, bj);
                c.cliff = v.cliff(self.grid.domain_point(face, i, j, level), slope, level);
                c.below = c.cliff.cells;
                c.above = c.cliff.cells;
            }
            if v.caves() && level as i32 <= v.deepest_level(true) {
                let rough = v.roughness();
                for (ri, rj) in v.owners(bi, bj) {
                    for p in self.cave_network(face, ri, rj).iter() {
                        if v.resolved(p, level) && p.covers(c.x, c.y, rough) {
                            c.primitives.push(*p);
                        }
                    }
                }
                let deepest = v.max_depth_cells(level);
                for p in &c.primitives {
                    let low = p.z_range(rough).0.div_euclid(4i64 << level) as i32;
                    c.below = c.below.max((top - low).clamp(0, deepest));
                }
            }
        }
        let c = Arc::new(c);
        let mut cache = self.volume_columns.lock().expect("volume columns");
        if cache.len() > 1 << 18 {
            cache.clear();
        }
        cache.insert(key, Arc::clone(&c));
        c
    }
    /// Generated kind of level cell `k` of a column before edits (0 air,
    /// 1 solid): the heightfield, its cliff shape and its caves
    /// (`chunk_columns_main`).
    pub fn column_kind(
        &self,
        c: &VolumeColumn,
        face: u8,
        i: i32,
        j: i32,
        k: i32,
        level: u32,
    ) -> u32 {
        let Some(v) = &self.volumes else {
            return terrain::terrain_kind(c.top, k);
        };
        if c.is_heightfield() || k < c.top - c.below || k >= c.top + c.above {
            return terrain::terrain_kind(c.top, k);
        }
        let mut solid = k < c.top;
        if c.cliff.rise > 0 && k >= c.top - c.cliff.cells && k < c.top + c.cliff.cells {
            solid = v.cliff_solid(
                &c.cliff,
                self.grid.volume_point(face, i, j, k, level),
                c.height,
                k,
                level,
            );
        }
        if solid
            && c.primitives
                .iter()
                .any(|p| v.carves(p, c.x, c.y, k, level, c.height))
        {
            solid = false;
        }
        u32::from(solid)
    }
    /// [`Self::column_kind`] of level cell `(i, j, k)`.
    pub fn generated_kind(&self, face: u8, i: i32, j: i32, k: i32, level: u32) -> u32 {
        let c = self.volume_column(face, i, j, level);
        self.column_kind(&c, face, i, j, k, level)
    }
    /// Generated top of a column (level cells): the first air above its
    /// highest generated solid cell, edits excluded. The heightfield's top
    /// outside the volume's extent; material depth counts from it.
    pub fn generated_top(&self, face: u8, i: i32, j: i32, level: u32) -> i32 {
        let c = self.volume_column(face, i, j, level);
        if c.is_heightfield() {
            return c.top;
        }
        (c.top - c.below..c.top + c.above)
            .rev()
            .find(|&k| self.column_kind(&c, face, i, j, k, level) == 1)
            .map_or(c.top - c.below, |k| k + 1)
    }
    /// Band-limited surface height of a level column, in height units.
    pub fn column_height(&self, face: u8, i: i32, j: i32, level: u32) -> i32 {
        let key = (face, i, j, level);
        if let Ok(cache) = self.heights.lock() {
            if let Some(&h) = cache.get(&key) {
                return h;
            }
        }
        let h = self
            .field
            .height(crate::terrain::Column { face, i, j, level });
        if let Ok(mut cache) = self.heights.lock() {
            if cache.len() > 1 << 20 {
                cache.clear();
            }
            cache.insert(key, h);
        }
        h
    }
    /// First air layer above the column (level cells).
    pub fn column_top(&self, face: u8, i: i32, j: i32, level: u32) -> i32 {
        terrain::top_cells(&self.grid, self.column_height(face, i, j, level), level)
    }
    /// A layer at least [`Self::column_top`]'s, of the natural terrain
    /// (`TerrainField::height_bound`: cheaper, for bounds).
    pub fn column_top_bound(&self, face: u8, i: i32, j: i32, level: u32) -> i32 {
        // Cached with the heights, keyed past the grid's levels.
        let key = (face, i, j, level + 64);
        let cached = self.heights.lock().ok().and_then(|c| c.get(&key).copied());
        let h = cached.unwrap_or_else(|| {
            let h = self
                .field
                .height_bound(crate::terrain::Column { face, i, j, level });
            if let Ok(mut cache) = self.heights.lock() {
                if cache.len() > 1 << 20 {
                    cache.clear();
                }
                cache.insert(key, h);
            }
            h
        });
        terrain::top_cells(&self.grid, h, level)
    }
    /// The brushes of `log` that can change level cell `(i, j, k)` (`k`
    /// `None`: any cell of the column), in order.
    fn face_brushes(
        log: &EditLog,
        face: u8,
        i: i32,
        j: i32,
        k: Option<i32>,
        level: u32,
    ) -> Vec<FaceBrush> {
        let range = |v: i32| [i64::from(v) << level, ((i64::from(v) + 1) << level) - 1];
        log.query(face, range(i), range(j), k.map_or(ALL_LAYERS, range), level)
            .into_iter()
            .map(|(id, index)| log.resolved(id).faces[index as usize])
            .collect()
    }
    /// [`Self::face_brushes`] of the large layer: those below their bake
    /// level (baked cells hold them above).
    fn large_brushes(
        &self,
        face: u8,
        i: i32,
        j: i32,
        k: Option<i32>,
        level: u32,
    ) -> Vec<FaceBrush> {
        let mut brushes = Self::face_brushes(&self.edits.large, face, i, j, k, level);
        brushes.retain(|fb| level < bake_level(fb));
        brushes
    }
    /// Canonical `(kind, material)` of a level cell: kind 0 air, 1 solid.
    /// Material 0 on a solid cell means "terrain rule".
    pub fn sample_kind(&self, level: u32, face: u8, i: i32, j: i32, k: i32) -> (u32, u32) {
        self.sample_kind_of(level, face, i, j, k, Matter::All)
    }
    /// [`Self::sample_kind`] of `matter`.
    fn sample_kind_of(
        &self,
        level: u32,
        face: u8,
        i: i32,
        j: i32,
        k: i32,
        matter: Matter,
    ) -> (u32, u32) {
        let kind = self.generated_kind(face, i, j, k, level);
        let features = match matter {
            Matter::All => self.features_at(level, face, i, j, k),
            Matter::Ground => None,
        };
        let (kind, material) = features::apply(features, kind, 0);
        let center = [
            center_half(i, level),
            center_half(j, level),
            center_half(k, level),
        ];
        let point = || self.grid.volume_point(face, i, j, k, level);
        let large = self.large_brushes(face, i, j, Some(k), level);
        let (kind, material) = apply(large.into_iter(), center, point, kind, material);
        let (kind, material) = self
            .edits
            .baked
            .cell(face, level, i, j, k)
            .apply(kind, material);
        apply(
            Self::face_brushes(&self.edits.recent, face, i, j, Some(k), level).into_iter(),
            center,
            point,
            kind,
            material,
        )
    }
    /// Every base cell of the `n`^3 brick at `origin` of `face` (index
    /// `((dj * n) + di) * n + dk`) as [`Self::sample_kind`] and
    /// [`Self::solid_of`] `Matter::Ground` give it: each column's height and
    /// brushes looked up once, and the features near the brick listed once
    /// (`FeatureEval::brick`). The same cells, exactly, as one query each,
    /// at a fraction of their cost. `with_features: false` leaves the
    /// features out (every cell its ground), cheaper still.
    pub fn sample_brick(
        &self,
        face: u8,
        origin: [i32; 3],
        n: i32,
        with_features: bool,
    ) -> Vec<BrickCell> {
        let [i0, j0, k0] = origin;
        let cells = (n * n * n) as usize;
        let mut features = vec![None; cells];
        let with_features = with_features && !self.features.is_empty();
        if with_features {
            let top = |f: u8, i: i32, j: i32| {
                features::ground(
                    &self.grid,
                    &*self.field,
                    f,
                    i,
                    j,
                    0,
                    self.column_height(f, i, j, 0),
                )
            };
            let mut cache = self.placements.lock().expect("placements");
            if cache.len() > 1 << 20 {
                cache.clear();
            }
            FeatureEval::new(&self.features, &self.grid, &*self.field, &top, &mut cache).brick(
                face,
                origin,
                n,
                &mut features,
            );
        }
        let mut out = Vec::with_capacity(cells);
        for j in j0..j0 + n {
            for i in i0..i0 + n {
                let height = self.column_height(face, i, j, 0);
                let large = self.large_brushes(face, i, j, None, 0);
                let recent = Self::face_brushes(&self.edits.recent, face, i, j, None, 0);
                // The far spawns' cover where no near placement holds a
                // cell (`features_at`).
                let far = with_features
                    .then(|| {
                        self.features.far_cover(
                            &self.grid,
                            &*self.field,
                            face,
                            0,
                            i,
                            j,
                            features::ground(&self.grid, &*self.field, face, i, j, 0, height),
                        )
                    })
                    .flatten();
                let column = (((j - j0) * n + (i - i0)) * n) as usize;
                let volume = self.volume_column(face, i, j, 0);
                for k in k0..k0 + n {
                    let generated = self.column_kind(&volume, face, i, j, k, 0);
                    let near = features[column + (k - k0) as usize];
                    let cell_features = near
                        .or_else(|| far.and_then(|cover| CompiledFeatures::far_cell(0, k, cover)));
                    let center = [center_half(i, 0), center_half(j, 0), center_half(k, 0)];
                    let point = || self.grid.volume_point(face, i, j, k, 0);
                    let baked = self.edits.baked.cell(face, 0, i, j, k);
                    let edited = |(kind, material): (u32, u32)| {
                        let (kind, material) =
                            apply(large.iter().copied(), center, point, kind, material);
                        let (kind, material) = baked.apply(kind, material);
                        apply(recent.iter().copied(), center, point, kind, material)
                    };
                    let ground = edited((generated, 0));
                    let (kind, material) = match cell_features {
                        None => ground,
                        some => edited(features::apply(some, generated, 0)),
                    };
                    out.push(BrickCell {
                        kind,
                        material,
                        ground: ground.0 == 1,
                    });
                }
            }
        }
        out
    }
    /// [`Self::kind`] for cells walked by a ray: the column's top and
    /// brushes are looked up once per column, not per cell.
    fn kind_in(&self, column: &mut Option<RayColumn>, cell: Cell, matter: Matter) -> u32 {
        self.kind_at(column, 0, cell.face, cell.i, cell.j, cell.k, matter)
    }
    /// Canonical kind of level cell `(i, j, k)` (as [`Self::sample_kind`]),
    /// for walks: the column's height and brushes are looked up once per
    /// column. The last edit that removes or adds at the cell decides its
    /// kind (paint keeps it), so the field is sampled only where none does:
    /// a walk down a dug pit or tunnel crosses carved cells without
    /// evaluating the rock (and its caves) above them.
    fn kind_at(
        &self,
        column: &mut Option<RayColumn>,
        level: u32,
        face: u8,
        i: i32,
        j: i32,
        k: i32,
        matter: Matter,
    ) -> u32 {
        let key = (face, i, j, level);
        if column.as_ref().is_none_or(|c| c.key != key) {
            *column = Some(RayColumn {
                key,
                volume: self.volume_column(face, i, j, level),
                large: self.large_brushes(face, i, j, None, level),
                recent: Self::face_brushes(&self.edits.recent, face, i, j, None, level),
            });
        }
        let c = column.as_ref().expect("filled above");
        let center = [
            center_half(i, level),
            center_half(j, level),
            center_half(k, level),
        ];
        let mut q = None;
        let point = || self.grid.volume_point(face, i, j, k, level);
        let mut decided = |brushes: &[FaceBrush]| {
            brushes
                .iter()
                .rev()
                .find(|b| b.op() < 2 && b.contains(center, || *q.get_or_insert_with(&point)))
                .map(|b| b.op())
        };
        if let Some(kind) = decided(&c.recent) {
            return kind;
        }
        match self.edits.baked.cell(face, level, i, j, k).state() {
            1 => return 0,
            crate::edit_store::CellEdit::SOLID => return 1,
            _ => {}
        }
        if let Some(kind) = decided(&c.large) {
            return kind;
        }
        if matter == Matter::All {
            if let Some(v) = self.features_at(level, face, i, j, k) {
                return u32::from(v.is_some());
            }
        }
        self.column_kind(&c.volume, face, i, j, k, level)
    }
    /// Slope (eighths of a cell per cell) that classifies the materials of
    /// base column (i, j), whatever level draws it: central differences of
    /// level-4 heights two cells (3.2 m) each way, in base layers (as
    /// `tile_slopes_main` in chunk_generate.wgsl evaluates them).
    ///
    /// Block slopes of level-4 cells are interpolated between their centres
    /// (in 32nds, truncated as the shader does).
    pub fn material_slope(&self, face: u8, i: i32, j: i32) -> i32 {
        const LEVEL: u32 = 4;
        // Central differences two level-4 cells each way, in base layers
        // (Q16 level-4 cells in the shader), and the gradient's length,
        // rounded down exactly (`gradient_length` in the shader): the larger
        // difference alone measured slopes across the axes short, and a
        // material threshold on it traced squares on the face's grid.
        let block = |mi: i32, mj: i32| {
            let top = |x: i32, y: i32| {
                terrain::top_cells(&self.grid, self.column_height(face, x, y, LEVEL), 0)
            };
            let di = (top(mi + 2, mj) - top(mi - 2, mj))
                .unsigned_abs()
                .min(46_340);
            let dj = (top(mi, mj + 2) - top(mi, mj - 2))
                .unsigned_abs()
                .min(46_340);
            (((di * di + dj * dj).isqrt() as i32) << (16 - LEVEL)) / 32_768
        };
        let (ri, rj) = (i * 2 + 1 - 16, j * 2 + 1 - 16);
        let (ai, aj) = (ri >> 5, rj >> 5);
        let (wi, wj) = (ri - (ai << 5), rj - (aj << 5));
        ((32 - wi) * (32 - wj) * block(ai, aj)
            + wi * (32 - wj) * block(ai + 1, aj)
            + (32 - wi) * wj * block(ai, aj + 1)
            + wi * wj * block(ai + 1, aj + 1))
            >> 10
    }
    /// Canonical kind at a base cell.
    pub fn kind(&self, cell: Cell) -> u32 {
        self.sample_kind(0, cell.face, cell.i, cell.j, cell.k).0
    }
    pub fn solid(&self, cell: Cell) -> bool {
        self.kind(cell) == 1
    }
    /// Whether base cell `cell` is solid `matter`.
    pub fn solid_of(&self, cell: Cell, matter: Matter) -> bool {
        self.sample_kind_of(0, cell.face, cell.i, cell.j, cell.k, matter)
            .0
            == 1
    }
    /// Resolved material of a base cell (0 for air).
    pub fn material(&self, cell: Cell) -> u32 {
        let (kind, material) = self.sample_kind(0, cell.face, cell.i, cell.j, cell.k);
        match kind {
            0 => material::AIR,
            _ if material != 0 => material,
            _ => self.terrain_material(cell),
        }
    }
    /// The terrain's own top over base column `(i, j)`: one over its highest
    /// solid cell, caves and overhangs included, edits and features
    /// ignored. A host deciding features from it decides a region the same
    /// whenever it does.
    pub fn terrain_top(&self, face: u8, i: i32, j: i32) -> i32 {
        self.generated_top(face, i, j, 0)
    }
    /// The terrain's rock: its material 64 cells under the middle of the
    /// first face. What a renderer shows where a ray meets ground it has
    /// not generated yet (an edit uncovering it).
    pub fn rock_material(&self) -> u32 {
        let face = self.grid.faces()[0];
        let middle = if self.grid.shape() == Shape::InfinitePlane {
            0
        } else {
            (self.grid.cells() / 2) as i32
        };
        let top = self.terrain_top(face, middle, middle);
        match self.terrain_material(Cell::new(face, middle, middle, top - 64)) {
            material::AIR => material::STONE,
            m => m,
        }
    }
    /// The material the terrain gives base cell `cell` were it solid there,
    /// edits and features ignored.
    pub fn terrain_material(&self, cell: Cell) -> u32 {
        let slope = self.material_slope(cell.face, cell.i, cell.j);
        // Materials vary in 3D (`shade` samples the cell's volume point): a
        // cliff's voxels are not one column's colour.
        let q = self.grid.volume_point(cell.face, cell.i, cell.j, cell.k, 0);
        let top_height = self.column_height(cell.face, cell.i, cell.j, 0);
        // Depth counts from the generated top: overhangs and the rock
        // around caves lie below it.
        let generated = self.generated_top(cell.face, cell.i, cell.j, 0);
        let depth = (generated - 1 - cell.k).max(0);
        let surface = self.field.surface(
            crate::terrain::Column {
                face: cell.face,
                i: cell.i,
                j: cell.j,
                level: 0,
            },
            top_height,
        ) & 0xff;
        self.field
            .ground_material(q, surface, top_height, depth, slope, cell.k)
            & material::ID
    }
    /// Exact base-grid ray cast. Stops at the first cell for which `stop`
    /// returns true given its kind. `max_distance` may be infinite; the ray
    /// is clipped to the world's shell (or slab and edges on a plane).
    pub fn raycast_with(
        &self,
        origin: DVec3,
        direction: DVec3,
        max_distance: f64,
        stop: impl Fn(u32) -> bool,
    ) -> Option<RayHit> {
        let mut column = None;
        match self.walk_cells(origin, direction, max_distance, |cell| {
            Some(stop(self.kind_in(&mut column, cell, Matter::All)))
        }) {
            Walked::Hit(hit) => Some(hit),
            Walked::Miss | Walked::Stopped(_) => None,
        }
    }
    /// The exact base-grid walk of a ray, clipped to the world's shell (or
    /// slab and edges on a plane): `visit` is given each cell in order and
    /// says whether the ray hits it (`Some(true)`), passes it
    /// (`Some(false)`) or cannot tell (`None`, which stops the walk there).
    /// What [`Self::raycast_with`] walks, over any source of cells (cached
    /// query bricks: `crate::queries`).
    pub fn walk_cells(
        &self,
        origin: DVec3,
        direction: DVec3,
        max_distance: f64,
        mut visit: impl FnMut(Cell) -> Option<bool>,
    ) -> Walked {
        if self.grid.is_plane() {
            return self.walk_plane(origin, direction, max_distance, visit);
        }
        let d = direction.normalize();
        let grid = &self.grid;
        let outer = self.outer_radius();
        let b = origin.dot(d);
        let c = origin.length_squared() - outer * outer;
        let mut t = 0.0f64;
        if c > 0.0 {
            let disc = b * b - c;
            if b >= 0.0 || disc < 0.0 {
                return Walked::Miss;
            }
            t = -b - disc.sqrt();
        }
        let limit = max_distance.min(-b + (b * b - c).max(0.0).sqrt());
        let s = grid.voxel_size();
        let eps = s * 1e-6;
        let (mut cell, _) = grid.locate(origin + d * (t + eps));
        let mut previous = cell;
        let mut normal = -d;
        let n = grid.cells();
        for _ in 0..4_000_000 {
            if t > limit {
                return Walked::Miss;
            }
            match visit(cell) {
                Some(true) => {
                    return Walked::Hit(RayHit {
                        cell,
                        previous,
                        distance: t,
                        normal,
                    })
                }
                Some(false) => {}
                None => return Walked::Stopped(t),
            }
            let [fn_, fa, fb] = face_axes(cell.face);
            let mut best = f64::INFINITY;
            let mut step = (0usize, 0i32, DVec3::ZERO);
            for (axis, (u, index)) in [(fa, cell.i), (fb, cell.j)].into_iter().enumerate() {
                for (offset, dir) in [(1, 1), (0, -1)] {
                    let angle = grid.angle(f64::from(index + offset));
                    let m = u * angle.cos() - fn_ * angle.sin();
                    let dm = d.dot(m);
                    // Leaving through the upper plane needs dm > 0, lower dm < 0.
                    // That exit plane is never behind the ray: a crossing
                    // rounded behind `t` (grazing planes, 6e6 m origins) is
                    // taken now, not dropped (a dropped crossing left the
                    // index stale for the rest of the ray).
                    if (dir == 1 && dm > 0.0) || (dir == -1 && dm < 0.0) {
                        let hit = -origin.dot(m) / dm;
                        if hit < best {
                            best = hit;
                            step = (axis, dir, -m * f64::from(dir));
                        }
                    }
                }
            }
            let r_lo = grid.layer_radius(f64::from(cell.k));
            let r_hi = r_lo + s;
            let ol = origin.length();
            let c_lo = (ol - r_lo) * (ol + r_lo);
            let disc_lo = b * b - c_lo;
            let mut radial = None;
            if disc_lo >= 0.0 {
                let root = disc_lo.sqrt();
                let enter = if b < 0.0 {
                    c_lo / (-b + root)
                } else {
                    -b - root
                };
                // Descending (before the perigee at -b): the lower layer
                // crossing is the exit, wherever rounding puts it.
                if b + t < 0.0 && b + enter < 0.0 {
                    radial = Some((enter, -1));
                }
            }
            if radial.is_none() {
                let c_hi = (ol - r_hi) * (ol + r_hi);
                let root = (b * b - c_hi).max(0.0).sqrt();
                let exit = if b > 0.0 {
                    -c_hi / (b + root)
                } else {
                    -b + root
                };
                radial = Some((exit, 1));
            }
            if let Some((hit, dir)) = radial {
                if hit < best {
                    best = hit;
                    let point = origin + d * hit;
                    step = (2, dir, point.normalize() * f64::from(dir) * -1.0);
                }
            }
            if !best.is_finite() {
                return Walked::Miss;
            }
            previous = cell;
            t = best.max(t);
            normal = step.2;
            let (axis, dir, _) = step;
            let mut next = cell;
            match axis {
                0 => next.i += dir,
                1 => next.j += dir,
                _ => next.k += dir,
            }
            if !(0..n).contains(&next.i) || !(0..n).contains(&next.j) {
                // Radial layers are shared by all faces; only (face, i, j) change.
                next = grid.locate(origin + d * (t + eps.max(t * 1e-12))).0;
                next.k = cell.k;
            }
            cell = next;
        }
        Walked::Miss
    }
    /// Cartesian cell walk on a plane grid, clipped to the slab between the
    /// lowest and highest solid layers and to the plane's edges.
    fn walk_plane(
        &self,
        origin: DVec3,
        direction: DVec3,
        max_distance: f64,
        mut visit: impl FnMut(Cell) -> Option<bool>,
    ) -> Walked {
        let grid = &self.grid;
        let d = direction.normalize();
        let [n_axis, a_axis, b_axis] = face_axes(crate::grid::PLANE_FACE);
        let Some(o) = grid.face_coords(crate::grid::PLANE_FACE, origin) else {
            return Walked::Miss;
        };
        let s = grid.voxel_size();
        let v = [d.dot(a_axis) / s, d.dot(b_axis) / s, d.dot(n_axis) / s];
        let n = f64::from(grid.cells());
        let lo = [0.0, 0.0, (self.inner_radius() / s).floor()];
        let hi = [n, n, (self.outer_radius() / s).ceil()];
        // Slab clipping in index space (t in metres).
        let (mut t0, mut t1) = (0.0f64, max_distance);
        for axis in 0..3 {
            if v[axis] == 0.0 {
                if o[axis] < lo[axis] || o[axis] >= hi[axis] {
                    return Walked::Miss;
                }
                continue;
            }
            let a = (lo[axis] - o[axis]) / v[axis];
            let b = (hi[axis] - o[axis]) / v[axis];
            t0 = t0.max(a.min(b));
            t1 = t1.min(a.max(b));
        }
        if t0 > t1 {
            return Walked::Miss;
        }
        let eps = 1e-9;
        let at = |t: f64| [o[0] + v[0] * t, o[1] + v[1] * t, o[2] + v[2] * t];
        let start = at(t0 + eps);
        let mut idx = [0i64; 3];
        for axis in 0..3 {
            idx[axis] = (start[axis].floor() as i64).clamp(lo[axis] as i64, hi[axis] as i64 - 1);
        }
        let step: [i64; 3] = std::array::from_fn(|axis| {
            if v[axis] > 0.0 {
                1
            } else if v[axis] < 0.0 {
                -1
            } else {
                0
            }
        });
        let mut next: [f64; 3] = std::array::from_fn(|axis| {
            if step[axis] == 0 {
                f64::INFINITY
            } else {
                let boundary = idx[axis] as f64 + if step[axis] > 0 { 1.0 } else { 0.0 };
                (boundary - o[axis]) / v[axis]
            }
        });
        let delta: [f64; 3] = std::array::from_fn(|axis| {
            if step[axis] == 0 {
                f64::INFINITY
            } else {
                1.0 / v[axis].abs()
            }
        });
        let axes = [a_axis, b_axis, n_axis];
        let cell_of = |idx: [i64; 3]| {
            Cell::new(
                crate::grid::PLANE_FACE,
                idx[0] as i32,
                idx[1] as i32,
                idx[2] as i32,
            )
        };
        let mut t = t0;
        let mut previous = cell_of(idx);
        let mut normal = -d;
        for _ in 0..4_000_000 {
            if t > t1 {
                return Walked::Miss;
            }
            let cell = cell_of(idx);
            match visit(cell) {
                Some(true) => {
                    return Walked::Hit(RayHit {
                        cell,
                        previous,
                        distance: t,
                        normal,
                    })
                }
                Some(false) => {}
                None => return Walked::Stopped(t),
            }
            let axis = if next[0] <= next[1] && next[0] <= next[2] {
                0
            } else if next[1] <= next[2] {
                1
            } else {
                2
            };
            previous = cell;
            t = next[axis];
            next[axis] += delta[axis];
            idx[axis] += step[axis];
            normal = -axes[axis] * step[axis] as f64;
            if idx[axis] < lo[axis] as i64 || idx[axis] >= hi[axis] as i64 {
                return Walked::Miss;
            }
        }
        Walked::Miss
    }
    /// Ray cast that stops at solid cells (terrain and additions).
    pub fn raycast(&self, origin: DVec3, direction: DVec3, max_distance: f64) -> Option<RayHit> {
        self.raycast_with(origin, direction, max_distance, |kind| kind == 1)
    }
    /// [`Self::raycast`] at solid `matter`.
    pub fn raycast_of(
        &self,
        origin: DVec3,
        direction: DVec3,
        max_distance: f64,
        matter: Matter,
    ) -> Option<RayHit> {
        let mut column = None;
        match self.walk_cells(origin, direction, max_distance, |cell| {
            Some(self.kind_in(&mut column, cell, matter) == 1)
        }) {
            Walked::Hit(hit) => Some(hit),
            Walked::Miss | Walked::Stopped(_) => None,
        }
    }
    /// The hit a renderer drew `distance` along a ray from `origin` (a GPU
    /// pick: what the user aimed at). At level 0 it is exact: the drawn
    /// voxel, and the air cell in front of the face the ray `entered`
    /// (`engine::PickHit`). A coarser level drew a cell standing for many
    /// voxels: the base cells either side of the drawn surface along the
    /// ray.
    pub fn drawn_hit(
        &self,
        origin: DVec3,
        direction: DVec3,
        distance: f64,
        cell: Cell,
        level: u32,
        entered: Option<u32>,
    ) -> RayHit {
        let g = &self.grid;
        let d = direction.normalize();
        let at = origin + d * distance;
        let half = d * g.voxel_size() * 0.5;
        let (mut hit, mut previous) = (g.locate(at + half).0, g.locate(at - half).0);
        if level == 0 {
            hit = cell;
            if let Some(code) = entered {
                let step = if code & 1 == 1 { -1 } else { 1 };
                let mut before = [cell.i, cell.j, cell.k];
                before[(code >> 1) as usize] += step;
                // Across a face edge the walk's neighbour is on another face.
                if (0..2).all(|a| (0..g.cells()).contains(&before[a])) || g.is_plane() {
                    previous = Cell::new(cell.face, before[0], before[1], before[2]);
                }
            }
        }
        RayHit {
            cell: hit,
            previous,
            distance,
            normal: -d,
        }
    }
    /// A point `clearance` metres above the solid surface over `p` (a
    /// direction or any point above the ground point on a planet; any point
    /// on a plane).
    ///
    /// Walks the base column under `p` down from the highest layer that can
    /// be solid there (its generated top, the top of an Add brush over it or
    /// of a baked solid cell), with the column's edits queried once: a few
    /// cells, not a ray from the world's outer radius through the edit index
    /// cell by cell.
    pub fn surface_point(&self, p: DVec3, clearance: f64) -> DVec3 {
        self.surface_point_of(p, clearance, Matter::All)
    }
    /// [`Self::surface_point`] over the solid `matter`.
    pub fn surface_point_of(&self, p: DVec3, clearance: f64, matter: Matter) -> DVec3 {
        let g = &self.grid;
        let (cell, _) = g.locate(g.at_radial(p, g.radius()));
        let (face, i, j) = (cell.face, cell.i, cell.j);
        let mut column = None;
        // Fills the column's height and brushes.
        self.kind_in(&mut column, Cell::new(face, i, j, 0), matter);
        let c = column.as_ref().expect("filled above");
        let added = c
            .large
            .iter()
            .chain(&c.recent)
            .filter(|b| b.op() == 1)
            .map(|b| b.k_hi.div_euclid(2) + 1)
            .max()
            .unwrap_or(i32::MIN);
        let baked = self
            .edits
            .baked
            .solid_top(face, 0, i, j)
            .unwrap_or(i32::MIN);
        let mut k = (self.generated_top(face, i, j, 0) + self.features_above_of(matter))
            .max(added)
            .max(baked);
        let floor = ((self.inner_radius() - g.radius()) / g.voxel_size()).floor() as i32;
        while k > floor && self.kind_in(&mut column, Cell::new(face, i, j, k - 1), matter) != 1 {
            k -= 1;
        }
        g.at_radial(p, g.layer_radius(f64::from(k)) + clearance)
    }
    /// Distance from `eye` to the nearest possible solid cell, conservative.
    pub fn air_clearance(&self, eye: DVec3) -> f64 {
        let r = self.grid.radial(eye);
        let above_outer = r - self.outer_radius();
        if above_outer > 0.0 {
            return above_outer;
        }
        let (cell, _) = self.grid.locate(eye);
        let top = self.column_top(cell.face, cell.i, cell.j, 0);
        let ground = (f64::from(cell.k - top - self.features_above()) * self.grid.voxel_size()
            - self.overhang_height())
        .max(0.0);
        // Additions stand above the generated ground: a near plane a
        // quarter of the height over it clipped a wall built beside the eye.
        self.addition_clearance(eye, cell, ground)
    }
    /// `limit`, or less where an addition (an Add brush or a baked solid
    /// cell) may lie within `limit` of `eye`, conservatively.
    fn addition_clearance(&self, eye: DVec3, cell: Cell, limit: f64) -> f64 {
        let g = &self.grid;
        if limit <= 0.0 || self.edit_top < g.radial(eye) - limit {
            return limit;
        }
        let base = if g.is_plane() {
            g.voxel_size()
        } else {
            g.delta() * g.radius() * 0.7
        };
        let reach = (limit / base).ceil() as i64 + 1;
        let (i, j) = (i64::from(cell.i), i64::from(cell.j));
        let mut clearance = limit;
        for log in [&self.edits.large, &self.edits.recent] {
            for (id, index) in log.query(
                cell.face,
                [i - reach, i + reach],
                [j - reach, j + reach],
                ALL_LAYERS,
                0,
            ) {
                if log.resolved(id).faces[index as usize].op() != 1 {
                    continue;
                }
                let brush = &log.resolved(id).brush;
                // A box's farthest corner bounds it whatever its frame.
                let extent = match brush.shape {
                    crate::edits::BrushShape::Sphere => brush.radius,
                    crate::edits::BrushShape::Cube => {
                        let half_height = if brush.height > 0.0 {
                            brush.height
                        } else {
                            brush.radius
                        };
                        (2.0 * brush.radius * brush.radius + half_height * half_height).sqrt()
                    }
                };
                let distance = (eye.distance(DVec3::from_array(brush.center)) - extent).max(0.0);
                clearance = clearance.min(distance);
            }
        }
        // Baked additions: the highest solid cell of every store column in
        // reach (base columns while few, else the coarsest level that keeps
        // them few, whose cells are majorities of their children).
        let mut level = 0;
        while ((reach >> level) * 2 / 8 + 1).pow(2) > 1024 {
            level += 1;
        }
        let columns = (reach >> level) / 8 + 1;
        let (ci, cj) = (
            i64::from(cell.i >> level) / 8,
            i64::from(cell.j >> level) / 8,
        );
        let height = g.height(eye);
        for a in ci - columns..=ci + columns {
            for b in cj - columns..=cj + columns {
                let key = ColumnKey {
                    face: cell.face,
                    level: level as u8,
                    ci: a as i32,
                    cj: b as i32,
                };
                let Some(column) = self.edits.baked.column(&key) else {
                    continue;
                };
                let Some(&(bk, _)) = column.last() else {
                    continue;
                };
                let top = f64::from((bk + 1) * crate::grid::BRICK) * g.level_size(level);
                clearance = clearance.min((height - top).max(0.0));
            }
        }
        clearance
    }
    /// Height of `eye` above the ground directly below it, along the local
    /// vertical: the generated ground's top over open terrain, and the floor
    /// of the hollow below the eye in the air of a pit, tunnel or cave under
    /// that top (negative when buried in solid rock). Unlike
    /// [`Self::air_clearance`] this is not a bound on the distance to all
    /// terrain: it is the altitude a camera or vehicle moves by.
    ///
    /// The hollow's floor is found by a walk down the column whose stride
    /// grows with the distance walked (a step per 1/16 of it, so about a
    /// hundred cells to 400 m), refined to the exact air-to-solid layer;
    /// a solid slab thinner than the stride there can be stepped over.
    pub fn ground_height(&self, eye: DVec3) -> f64 {
        self.ground_height_of(eye, Matter::All)
    }
    /// [`Self::ground_height`] over the solid `matter`.
    pub fn ground_height_of(&self, eye: DVec3, matter: Matter) -> f64 {
        const REACH: i32 = 4096;
        let g = &self.grid;
        let (cell, _) = g.locate(eye);
        // The generated top: overhang lips and cave mouths included.
        let top = self.generated_top(cell.face, cell.i, cell.j, 0);
        let height = g.height(eye);
        let mut column = None;
        let solid = |column: &mut Option<RayColumn>, k: i32| {
            self.kind_in(column, Cell::new(cell.face, cell.i, cell.j, k), matter) == 1
        };
        if cell.k >= top {
            // Over the terrain: features may stand on it.
            let mut k = (top + self.features_above_of(matter)).min(cell.k);
            while k > top && !solid(&mut column, k - 1) {
                k -= 1;
            }
            return height - f64::from(k) * g.voxel_size();
        }
        if solid(&mut column, cell.k) {
            return height - f64::from(top) * g.voxel_size();
        }
        let floor = ((self.inner_radius() - g.radius()) / g.voxel_size()).floor() as i32;
        let (mut air, mut k) = (cell.k, cell.k - 1);
        while k > floor && cell.k - k <= REACH {
            if solid(&mut column, k) {
                // Air at `air`, solid at `k`: the topmost solid layer between.
                let mut lo = k;
                while air - lo > 1 {
                    let mid = (air + lo) / 2;
                    if solid(&mut column, mid) {
                        lo = mid;
                    } else {
                        air = mid;
                    }
                }
                return height - f64::from(lo + 1) * g.voxel_size();
            }
            air = k;
            k -= 1 + (cell.k - k) / 16;
        }
        height - f64::from(air) * g.voxel_size()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edits::{BrushOp, BrushShape};

    fn planet() -> Planet {
        Planet::new(PlanetRecipe::default()).unwrap()
    }

    /// Over a feature (a tree on Earth), every query of all matter stops at
    /// it; the ground's pass through it to the terrain: what an editor
    /// camera flies through and lands on.
    #[test]
    fn ground_matter_passes_features() {
        let planet = Planet::new(PlanetRecipe {
            radius_m: EARTH_RADIUS,
            terrain: crate::layers::TerrainLayers::earth().source(7),
            features: crate::feature_recipe::FeatureRecipe::earth().grow(0.1, 7),
            ..Default::default()
        })
        .unwrap();
        let (g, face) = (planet.grid(), 2u8);
        let c0 = g.cells() / 2;
        let top = |f: u8, i: i32, j: i32| {
            features::ground(
                g,
                planet.field(),
                f,
                i,
                j,
                0,
                planet.column_height(f, i, j, 0),
            )
        };
        let mut cache = PlacementCache::default();
        let mut eval = FeatureEval::new(&*planet.features, g, planet.field(), &top, &mut cache);
        // A feature's solid cell over the terrain, in the column of its anchor.
        let (cell, ground) = (0..planet.features.spawns.len())
            .flat_map(|s| eval.placements_near(s, face, [c0, c0 + 4000], [c0, c0 + 4000]))
            .find_map(|p| {
                let ground = planet.terrain_top(face, p.i, p.j);
                (ground..ground + 400)
                    .map(|k| Cell::new(face, p.i, p.j, k))
                    .find(|c| matches!(planet.features_at(0, face, c.i, c.j, c.k), Some(Some(_))))
                    .map(|c| (c, ground))
            })
            .expect("a feature over the terrain");
        assert!(
            planet.solid_of(cell, Matter::All) && planet.solid(cell),
            "{cell:?}"
        );
        assert!(
            !planet.solid_of(cell, Matter::Ground),
            "the ground has no feature at {cell:?}"
        );
        // From above: all matter lands on the feature, the ground on the terrain.
        let eye = g.position(
            face,
            [
                f64::from(cell.i) + 0.5,
                f64::from(cell.j) + 0.5,
                f64::from(ground + 600),
            ],
        );
        let over_ground = planet.ground_height_of(eye, Matter::Ground);
        assert!(
            (over_ground - 600.0 * g.voxel_size()).abs() < 1e-4,
            "{over_ground}"
        );
        assert!(
            planet.ground_height_of(eye, Matter::All)
                <= over_ground - f64::from(cell.k + 1 - ground) * g.voxel_size() + 1e-4
        );
        let landed = planet.surface_point_of(eye, 0.0, Matter::Ground);
        assert!((g.height(landed) - f64::from(ground) * g.voxel_size()).abs() < 1e-4);
    }

    /// Earth without caves and overhangs: a pure heightfield, for
    /// column-top invariants.
    fn heightfield(recipe: PlanetRecipe) -> Planet {
        let terrain = crate::layers::TerrainLayers::earth()
            .heightfield()
            .source(TerrainSource::default().seed);
        Planet::new(PlanetRecipe { terrain, ..recipe }).unwrap()
    }

    /// Diagnostic: surface material shares of a mountain flank as each
    /// level draws it (shading's coarse-cell rule), against level 0.
    #[test]
    fn ground_height_is_altitude_above_the_column_below() {
        let p = heightfield(PlanetRecipe::default());
        for dir in [
            DVec3::new(0.1, 1.0, 0.2),
            DVec3::new(0.9, 0.4, -0.3),
            DVec3::new(-0.2, -0.7, 0.8),
        ] {
            for h in [0.5, 12.0, 3000.0] {
                let eye = p.surface_point(dir, h);
                let measured = p.ground_height(eye);
                assert!(
                    (measured - h).abs() <= p.grid().voxel_size() * 1.01,
                    "{dir} {h}: {measured}"
                );
                // A conservative clearance never exceeds it.
                assert!(p.air_clearance(eye) <= measured + p.grid().voxel_size());
            }
        }
    }

    #[test]
    fn terrain_bounds_hold_for_sampled_cells() {
        let flat = crate::layers::TerrainLayers::flat_at(3.3).source(7);
        for (terrain, voxel) in [
            (TerrainSource::default(), 0.1),
            (TerrainSource::default(), 0.3),
            (TerrainSource::default(), 1.0),
            (flat, 0.1),
        ] {
            let p = Planet::new(PlanetRecipe {
                voxel_size_m: voxel,
                terrain: terrain.clone(),
                ..Default::default()
            })
            .unwrap();
            let worst = terrain::check_field(&p, 6_000)
                .unwrap_or_else(|e| panic!("{} at {voxel} m: {e}", terrain.generator));
            eprintln!(
                "{} {voxel}: margins {:?}\nworst {worst:?}",
                terrain.generator,
                &p.field().bound_margins()[..p.grid().levels() as usize]
            );
        }
    }

    fn plane(shape: Shape) -> Planet {
        heightfield(PlanetRecipe {
            shape,
            plane_size_m: 3_000.0,
            ..Default::default()
        })
    }

    #[test]
    fn plane_raycast_down_hits_the_column_top_and_matches_edits() {
        for shape in [Shape::Plane, Shape::InfinitePlane] {
            let mut p = plane(shape);
            let g = *p.grid();
            let face = crate::grid::PLANE_FACE;
            for &(dx, dz) in &[(0.0, 0.0), (731.3, -412.9), (-1_100.0, 1_200.5)] {
                let ground = p.surface_point(DVec3::new(dx, 0.0, dz), 0.0);
                let (cell, _) = g.locate(ground + DVec3::Y * 0.01);
                let top = p.column_top(face, cell.i, cell.j, 0);
                assert_eq!(cell.k, top, "surface point sits on the first air layer");
                let eye = ground + DVec3::Y * 20.0;
                let hit = p.raycast(eye, -DVec3::Y, 1000.0).expect("hit");
                assert_eq!(hit.cell, Cell::new(face, cell.i, cell.j, top - 1));
                assert!((hit.distance - 20.0).abs() < g.voxel_size() * 1.01);
                assert_eq!(hit.normal, DVec3::Y);
            }
            // Oblique rays agree with a cell walk through a carved hole.
            let centre = p.surface_point(DVec3::new(50.0, 0.0, 60.0), -0.45);
            p.apply(Brush {
                center: centre.to_array(),
                radius: 0.45,
                shape: BrushShape::Sphere,
                op: BrushOp::Remove,
                material: 0,
                height: 0.0,
            })
            .unwrap();
            let eye = centre + DVec3::new(0.3, 30.0, 0.0);
            let hit = p.raycast(eye, centre - eye, 100.0).expect("hit");
            assert!(p.solid(hit.cell));
            assert!(!p.solid(hit.previous));
            // Nothing past the edge of a finite plane.
            if shape == Shape::Plane {
                assert!(p
                    .raycast(DVec3::new(5_000.0, 10.0, 0.0), -DVec3::Y, 1000.0)
                    .is_none());
            }
        }
    }

    /// A point placed above the ground reads that height, wherever the
    /// ground is (overhang lips and cave mouths included).
    #[test]
    fn ground_height_is_height_above_the_generated_ground() {
        let p = planet();
        for dir in [
            DVec3::Y,
            DVec3::new(0.3, 1.0, -0.2),
            DVec3::new(-0.7, 0.2, 0.68),
            DVec3::new(0.1, -0.4, 0.9),
        ] {
            let eye = p.surface_point(dir.normalize(), 2.0);
            let h = p.ground_height(eye);
            assert!((h - 2.0).abs() < 0.2, "{dir}: {h}");
        }
    }

    /// Level selection measures the eye's height over the terrain as
    /// edited: inside a dig, over its floor; additions count only where
    /// they are.
    #[test]
    fn air_clearance_counts_additions() {
        let mut p = heightfield(PlanetRecipe::default());
        let g = *p.grid();
        let up = DVec3::new(-0.3, 1.0, 0.4).normalize();
        let eye = p.surface_point(up, 40.0);
        assert!(p.air_clearance(eye) > 30.0);
        let side = up.any_orthonormal_vector();
        let wall = eye + side * 8.0;
        p.apply(Brush {
            center: wall.to_array(),
            radius: 6.0,
            shape: BrushShape::Sphere,
            op: BrushOp::Add,
            material: 3,
            height: 0.0,
        })
        .unwrap();
        let near = p.air_clearance(eye);
        assert!(near <= 2.0 + 1e-9, "a ball 2 m away: {near} m");
        // Sealed into the store, small additions still count.
        let mut q = heightfield(PlanetRecipe::default());
        for n in 0..(crate::planet::RECENT_BRUSHES + crate::planet::SEAL_BATCH) as i32 {
            let at = wall + up * (f64::from(n) * 0.3 - 6.0);
            q.apply(Brush {
                center: at.to_array(),
                radius: 0.3,
                shape: BrushShape::Cube,
                op: BrushOp::Add,
                material: 3,
                height: 0.0,
            })
            .unwrap();
        }
        assert!(q.edits().sealed_len() > 0);
        let near = q.air_clearance(eye);
        assert!(near <= 8.0, "a sealed column 8 m away: {near} m");
        let _ = g;
    }

    /// In a dug pit the camera's altitude is its height above the pit's
    /// floor, not a depth below the generated ground it no longer stands on.
    #[test]
    fn ground_height_in_a_pit_is_height_above_its_floor() {
        let mut p = planet();
        let g = *p.grid();
        let up = DVec3::new(0.2, 1.0, -0.1).normalize();
        let ground = p.surface_point(up, 0.0);
        // A 30 m deep shaft 12 m wide, its floor flat.
        let floor = g.radial(ground) - 30.0;
        p.apply(Brush {
            center: g.at_radial(up, floor + 20.0).to_array(),
            radius: 6.0,
            shape: BrushShape::Cube,
            op: BrushOp::Remove,
            material: 0,
            height: 20.0,
        })
        .unwrap();
        for above in [0.5, 4.0, 25.0] {
            let eye = g.at_radial(up, floor + above);
            let h = p.ground_height(eye);
            assert!(
                (h - above).abs() <= g.voxel_size() * 1.01,
                "{above} m over the floor: {h}"
            );
        }
        // Over open ground nearby and buried in rock it is as before.
        let side = up.any_orthonormal_vector();
        let open = p.surface_point((up + side * 0.001).normalize(), 3.0);
        assert!((p.ground_height(open) - 3.0).abs() < 0.2);
        let buried = p.surface_point((up + side * 0.001).normalize(), -5.0);
        assert!(p.ground_height(buried) < -4.0);
    }

    #[test]
    fn raycast_down_hits_the_column_top() {
        let p = heightfield(PlanetRecipe::default());
        let g = *p.grid();
        for &(face, fi, fj) in &[(4u8, 0.31, 0.62), (0, 0.9, 0.1), (3, 0.5, 0.5)] {
            let i = (f64::from(g.cells()) * fi) as i32;
            let j = (f64::from(g.cells()) * fj) as i32;
            let top = p.column_top(face, i, j, 0);
            let dir = g.direction(face, f64::from(i) + 0.5, f64::from(j) + 0.5);
            let eye = dir * g.layer_radius(f64::from(top) + 20.0);
            let hit = p.raycast(eye, -dir, 1000.0).expect("hit");
            assert_eq!(hit.cell, Cell::new(face, i, j, top - 1));
            assert!((hit.distance - 20.0 * g.voxel_size()).abs() < 1e-6);
        }
    }

    #[test]
    fn brushes_carve_and_fill_exactly() {
        let mut p = planet();
        let g = *p.grid();
        let (face, i, j) = (4u8, g.cells() / 3, g.cells() / 5);
        let top = p.column_top(face, i, j, 0);
        let centre = g.cell_center(Cell::new(face, i, j, top - 1));
        p.apply(Brush {
            center: centre.to_array(),
            radius: 1.0,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        })
        .unwrap();
        assert!(!p.solid(Cell::new(face, i, j, top - 1)));
        assert!(!p.solid(Cell::new(face, i, j, top - 9)));
        assert!(p.solid(Cell::new(face, i, j, top - 12)));
        p.apply(Brush {
            center: centre.to_array(),
            radius: 0.25,
            shape: BrushShape::Cube,
            op: BrushOp::Add,
            material: material::BRICK,
            height: 0.0,
        })
        .unwrap();
        assert!(p.solid(Cell::new(face, i, j, top - 1)));
        assert_eq!(p.material(Cell::new(face, i, j, top - 1)), material::BRICK);
        assert!(p.undo().is_some());
        assert!(!p.solid(Cell::new(face, i, j, top - 1)));
    }

    #[test]
    fn oblique_raycast_matches_cell_walk_through_carved_hole() {
        let mut p = planet();
        let g = *p.grid();
        let (face, i, j) = (2u8, g.cells() / 2 + 17, g.cells() / 2 - 40);
        let top = p.column_top(face, i, j, 0);
        let centre = g.cell_center(Cell::new(face, i, j, top - 5));
        p.apply(Brush {
            center: centre.to_array(),
            radius: 0.45,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        })
        .unwrap();
        let up = centre.normalize();
        let eye = centre + up * 30.0 + up.any_orthonormal_vector() * 0.3;
        let hit = p.raycast(eye, centre - eye, 100.0).expect("hit");
        assert!(p.solid(hit.cell));
        assert!(!p.solid(hit.previous));
        let n = g.voxel_size();
        assert!(
            g.cell_center(hit.cell)
                .distance(eye + (centre - eye).normalize() * hit.distance)
                < n * 1.8
        );
    }

    /// Level-0 cells read the same through the edit layers (large, baked,
    /// recent, with brushes sealed many times) as every brush replayed in
    /// order over the terrain.
    #[test]
    fn edit_layers_match_brushes_replayed_in_order() {
        let mut p = heightfield(PlanetRecipe::default());
        let g = *p.grid();
        let (face, i0, j0) = (2u8, g.cells() / 2 + 8, g.cells() / 2 - 24);
        let top = p.column_top(face, i0 + 20, j0 + 20, 0);
        let mut seed = 0x9e37_79b9u64;
        let mut next = move || {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        let ops = [BrushOp::Remove, BrushOp::Add, BrushOp::Paint];
        let mut applied = Vec::new();
        for n in 0..805 {
            let cell = Cell::new(
                face,
                i0 + (next() * 40.0) as i32,
                j0 + (next() * 40.0) as i32,
                top - 16 + (next() * 24.0) as i32,
            );
            // Every 50th brush is too large to bake.
            let radius = if n % 50 == 7 {
                4.0
            } else {
                0.05 + next() * 0.6
            };
            let shape = if next() < 0.5 {
                BrushShape::Sphere
            } else {
                BrushShape::Cube
            };
            // A third of the cubes are flat-topped boxes.
            let height = if shape == BrushShape::Cube && next() < 0.33 {
                0.05 + next() * 0.4
            } else {
                0.0
            };
            let brush = Brush {
                center: g.cell_center(cell).to_array(),
                radius,
                shape,
                op: ops[(next() * 3.0) as usize],
                material: 1 + (next() * 20.0) as u32,
                height,
            };
            p.apply(brush).unwrap();
            applied.push(brush);
        }
        let edits = p.edits();
        assert!(
            !edits.baked.is_empty()
                && !edits.large.is_empty()
                && edits.recent.len() <= RECENT_BRUSHES
        );
        check_replay(&p, &applied, face, i0, j0, top);
        // Undo past several seals: they come undone, exactly.
        assert!(
            p.edits().undoable() >= 400,
            "{} undoable",
            p.edits().undoable()
        );
        for n in 0..400 {
            p.undo().unwrap_or_else(|| {
                panic!(
                    "undo {n} of 400: {} undoable, {} recent, {} sealed",
                    p.edits().undoable(),
                    p.edits().recent.len(),
                    p.edits().sealed_len()
                )
            });
        }
        applied.truncate(applied.len() - 400);
        assert_eq!(p.edits().hash(), crate::edits::history_hash(&applied));
        check_replay(&p, &applied, face, i0, j0, top);
    }

    /// Every level-0 cell of a 40x40x40 block reads what `applied` replayed
    /// in order over the terrain gives.
    fn check_replay(p: &Planet, applied: &[Brush], face: u8, i0: i32, j0: i32, top: i32) {
        let g = *p.grid();
        let faces: Vec<FaceBrush> = applied
            .iter()
            .flat_map(|b| b.resolve(&g).unwrap())
            .filter(|fb| fb.face() == face)
            .collect();
        let mut edited = 0;
        let mut column = None;
        for i in i0..i0 + 40 {
            for j in j0..j0 + 40 {
                for k in top - 24..top + 16 {
                    let kind = p.generated_kind(face, i, j, k, 0);
                    let center = [center_half(i, 0), center_half(j, 0), center_half(k, 0)];
                    let expected = apply(
                        faces.iter().copied(),
                        center,
                        || g.volume_point(face, i, j, k, 0),
                        kind,
                        0,
                    );
                    assert_eq!(
                        p.sample_kind(0, face, i, j, k),
                        expected,
                        "cell ({i}, {j}, {k})"
                    );
                    assert_eq!(p.solid(Cell::new(face, i, j, k)), expected.0 == 1);
                    // The walks' kind, decided by the last edit at the cell.
                    assert_eq!(
                        p.kind_in(&mut column, Cell::new(face, i, j, k), Matter::All),
                        expected.0,
                        "walked cell ({i}, {j}, {k})"
                    );
                    edited += usize::from(expected != (kind, 0));
                }
            }
        }
        assert!(edited > 1000, "{edited} cells edited");
    }

    /// The walks' kinds are the canonical ones at every level.
    #[test]
    fn sealed_large_brushes_read_the_same_at_their_bake_level() {
        // A dig 8 m across, sealed (it bakes at level 2), on unedited ground:
        // at its bake level every cell reads as the brush applied
        // analytically to the terrain, and below it the brush stays
        // analytic.
        let mut p = heightfield(PlanetRecipe::default());
        let unedited = p.clone();
        let g = *p.grid();
        let (face, i0, j0) = (2u8, g.cells() / 2 + 16, g.cells() / 2 - 32);
        let top = p.column_top(face, i0, j0, 0);
        let centre = g.cell_center(Cell::new(face, i0, j0, top));
        let dig = Brush {
            center: centre.to_array(),
            radius: 8.0,
            shape: BrushShape::Sphere,
            op: BrushOp::Remove,
            material: 0,
            height: 0.0,
        };
        p.apply(dig).unwrap();
        // Far away small brushes push it out of the recent window.
        let far = g.cell_center(Cell::new(face, i0 + 4000, j0, top));
        for _ in 0..RECENT_BRUSHES + SEAL_BATCH {
            p.apply(Brush {
                center: far.to_array(),
                radius: 0.1,
                shape: BrushShape::Cube,
                op: BrushOp::Paint,
                material: 3,
                height: 0.0,
            })
            .unwrap();
        }
        assert_eq!(p.edits().large.len(), 1, "the dig is sealed and large");
        let faces = dig.resolve(&g).unwrap();
        let level = bake_level(&faces[0]);
        assert_eq!(level, 2);
        assert!(p.edits().baked.brick_counts()[level as usize] > 0);
        let (ci, cj, ck) = (i0 >> level, j0 >> level, top >> level);
        let reach = 30;
        let mut inside = 0;
        for i in ci - reach..=ci + reach {
            for j in cj - reach..=cj + reach {
                for k in ck - reach..=ck + reach {
                    let center = [
                        center_half(i, level),
                        center_half(j, level),
                        center_half(k, level),
                    ];
                    let point = || g.volume_point(face, i, j, k, level);
                    let natural = unedited.sample_kind(level, face, i, j, k);
                    let want = apply(faces.iter().copied(), center, point, natural.0, natural.1);
                    assert_eq!(
                        p.sample_kind(level, face, i, j, k),
                        want,
                        "level {level} cell ({i}, {j}, {k})"
                    );
                    inside += usize::from(natural.0 == 1 && want.0 == 0);
                }
            }
        }
        assert!(inside > 1000, "{inside} cells dug");
        assert_eq!(
            p.large_brushes(face, i0 >> (level - 1), j0 >> (level - 1), None, level - 1)
                .len(),
            1
        );
        assert!(p.large_brushes(face, ci, cj, None, level).is_empty());
    }

    #[test]
    fn sealed_small_edits_show_at_coarse_levels() {
        let mut p = heightfield(PlanetRecipe::default());
        let unedited = p.clone();
        let g = *p.grid();
        let (face, i0, j0) = (2u8, g.cells() / 2 + 16, g.cells() / 2 - 32);
        let top = (i0..i0 + 16)
            .flat_map(|i| (j0..j0 + 16).map(move |j| (i, j)))
            .map(|(i, j)| p.column_top(face, i, j, 0))
            .min()
            .unwrap();
        // Level-2 layers fully inside [k_lo, top - 1).
        let k_lo = top - 16;
        for i in i0..i0 + 16 {
            for j in j0..j0 + 16 {
                for k in k_lo..top {
                    let centre = g.cell_center(Cell::new(face, i, j, k));
                    p.apply(Brush {
                        center: centre.to_array(),
                        radius: g.voxel_size() * 0.3,
                        shape: BrushShape::Cube,
                        op: BrushOp::Remove,
                        material: 0,
                        height: 0.0,
                    })
                    .unwrap();
                }
            }
        }
        assert!(p.edits().baked.brick_counts()[2] > 0);
        // The last row of brushes (i0 + 12..) may still be recent.
        let (ci, cj) = (i0 >> 2, j0 >> 2);
        let mut checked = 0;
        for i in ci..ci + 3 {
            for j in cj..cj + 4 {
                for k in (k_lo + 3) >> 2..top >> 2 {
                    if (k << 2) < k_lo || (k << 2) + 3 >= top {
                        continue;
                    }
                    assert_eq!(
                        unedited.sample_kind(2, face, i, j, k).0,
                        1,
                        "level-2 cell ({i}, {j}, {k}) before"
                    );
                    assert_eq!(
                        p.sample_kind(2, face, i, j, k).0,
                        0,
                        "level-2 cell ({i}, {j}, {k})"
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked >= 12 * 2, "{checked} cells checked");
    }
}

#[cfg(test)]
mod scaling {
    use super::*;
    use crate::edits::{BrushOp, BrushShape};

    /// Cost of what sculpting does per stamp, as edits pile up in one area
    /// (editor strokes): copy the world, apply a stamp, cast the editor's
    /// aim ray, find the ground. Run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn sculpt_query_costs() {
        let mut planet = Planet::new(PlanetRecipe::default()).unwrap();
        let ground = planet.surface_point(DVec3::new(0.3, 1.0, 0.2), 0.0);
        let up = ground.normalize();
        let side = up.any_orthonormal_vector();
        let ahead = up.cross(side);
        let eye = ground + up * 8.0;
        let time = |f: &mut dyn FnMut()| {
            let t = std::time::Instant::now();
            for _ in 0..20 {
                f();
            }
            t.elapsed().as_secs_f64() * 1000.0 / 20.0
        };
        let mut stamp = 0usize;
        for target in [0usize, 100, 400, 1000, 2000] {
            while stamp < target {
                let angle = stamp as f64 * 0.6 / 6.0;
                let p = ground + (side * angle.cos() + ahead * angle.sin()) * 6.0;
                planet
                    .apply(Brush {
                        center: (p - up * 0.3).to_array(),
                        radius: 1.0,
                        shape: BrushShape::Sphere,
                        op: BrushOp::Remove,
                        material: 0,
                        height: 0.0,
                    })
                    .unwrap();
                stamp += 1;
            }
            let aim = (ground + side * 6.0 - eye).normalize();
            let clone = time(&mut || {
                std::hint::black_box(planet.clone());
            });
            let mut copy = planet.clone();
            let apply = time(&mut || {
                copy.apply(Brush {
                    center: (ground - up * 0.3).to_array(),
                    radius: 1.0,
                    shape: BrushShape::Sphere,
                    op: BrushOp::Remove,
                    material: 0,
                    height: 0.0,
                })
                .unwrap();
            });
            let raycast = time(&mut || {
                std::hint::black_box(planet.raycast(eye, aim, 200.0));
            });
            let surface = time(&mut || {
                std::hint::black_box(planet.surface_point(ground + side * 6.0, 0.0));
            });
            let (cell, _) = planet.grid().locate(ground + side * 6.0 - up * 0.5);
            let solid = time(&mut || {
                std::hint::black_box(planet.solid(cell));
            });
            eprintln!("SCULPT_COST {stamp:5} edits: clone {clone:.3} ms, apply {apply:.3} ms, aim raycast {raycast:.3} ms, surface_point {surface:.3} ms, solid {solid:.4} ms");
        }
    }

    /// Cost of copying a world with many edits (what a shared world pays per
    /// appended edit). Run with `--ignored --nocapture`.
    #[test]
    #[ignore]
    fn clone_cost_with_many_edits() {
        let mut planet = Planet::new(PlanetRecipe {
            shape: Shape::Plane,
            plane_size_m: 4096.0,
            ..Default::default()
        })
        .unwrap();
        for n in [1_000usize, 10_000, 50_000] {
            let appending = std::time::Instant::now();
            let before = planet.edits().len();
            while planet.edits().len() < n {
                let k = planet.edits().len() as f64;
                let p = DVec3::new((k * 7.31) % 900.0 - 450.0, 2.0, (k * 3.17) % 900.0 - 450.0);
                planet
                    .apply(Brush {
                        center: p.to_array(),
                        radius: 0.05,
                        shape: BrushShape::Cube,
                        op: BrushOp::Add,
                        material: 13,
                        height: 0.0,
                    })
                    .unwrap();
            }
            let per_edit = appending.elapsed().as_secs_f64() * 1e6 / (n - before) as f64;
            let t = std::time::Instant::now();
            let copies = 20;
            for _ in 0..copies {
                std::hint::black_box(planet.clone());
            }
            eprintln!(
                "{n} edits: clone {:.3} ms, append {per_edit:.1} us/edit",
                t.elapsed().as_secs_f64() * 1000.0 / copies as f64
            );
        }
    }
}
