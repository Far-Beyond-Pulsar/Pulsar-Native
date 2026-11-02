//! The CPU voxel world of a terrain entity, and its scripting surface.
//!
//! A terrain component names a registered generator, its seed and settings,
//! and an ordered edit journal. [`world_recipe`] turns that into the world
//! both the renderer and gameplay code use, so a block script, the sculpt
//! tool and the renderer always agree on every cell. Positions are world
//! metres; the terrain entity sits at the origin.
//!
//! Scripts (blueprints) call the world methods on
//! [`VoxelTerrainComponent`]: blocks (`get_block`, `set_block`,
//! `fill_sphere`, `fill_cube`, `raycast_distance`), and every setting of
//! the world, read and written: `world_shape`, `planet_radius`,
//! `plane_size`, `voxel_size`, `seed`, `air`, `air_density`, `air_tint_*`,
//! `air_haze`, `is_editable` and their `set_*`; its features by preset
//! (`use_feature_preset`). Material ids are the
//! engine terrain materials (`helio_pass_voxel_planet::terrain::material`);
//! 0 is air. They shape the built-in generator's layer stack through
//! [`VoxelTerrainLayersComponent`]: `use_preset`, `add_layer`,
//! `remove_layer`, `layer_count` and the per-layer setters; the world
//! rebuilds from the changed settings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::DVec3;
/// Terrain material ids (`material::GRASS`, ...) and their names.
pub use helio_pass_voxel_planet::terrain::material;
use helio_pass_voxel_planet::{
    Brush, BrushOp, BrushShape, Planet, PlanetRecipe, TerrainSource, grid::Shape, terrain,
};
use helio_voxel_data::{
    VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape, VoxelEditBase, VoxelEditJournal,
};
use pulsar_scene_model::components::Transform;
use pulsar_scenedb::{Entity, World};

use helio_pass_voxel_planet::feature_recipe::FeatureRecipe;

use super::{
    BlockData, BlockMaterialChange, VoxelAir, feature_recipe, VoxelLayerKind, VoxelTerrainComponent,
    VoxelTerrainLayer, VoxelTerrainLayersComponent, VoxelTerrainStack, VoxelWorldShape,
};

/// Base colour (linear 0..1 sRGB-encoded components, as authored) of material
/// `id` in the built-in appearance, for editor palettes.
pub fn material_colour(id: u32) -> [f32; 3] {
    let table = helio_pass_voxel_planet::terrain::TerrainAppearance::default();
    let colour = table
        .materials
        .get(id as usize)
        .map_or([0.5; 4], |m| m.colour);
    [colour[0], colour[1], colour[2]]
}

/// The world of a terrain form, generator and features (grown with the
/// generator's seed in cells of the voxel size).
pub fn world_recipe(
    shape: VoxelWorldShape,
    planet_radius: f64,
    plane_size: f64,
    voxel_size: f64,
    source: TerrainSource,
    features: &FeatureRecipe,
) -> PlanetRecipe {
    let mut recipe = ground_recipe(shape, planet_radius, plane_size, voxel_size, source);
    recipe.features = features.grow(voxel_size, recipe.terrain.seed);
    recipe
}

/// Fingerprint of the ground a terrain makes, its features included: its
/// edits belong to it (`VoxelEditJournal::made_on`). The features count as
/// their recipe (never grown here, so it is cheap every frame); a world
/// without features keeps its recipe's own fingerprint.
pub fn ground_fingerprint(
    shape: VoxelWorldShape,
    planet_radius: f64,
    plane_size: f64,
    voxel_size: f64,
    source: TerrainSource,
    features: &FeatureRecipe,
) -> u64 {
    let ground = ground_recipe(shape, planet_radius, plane_size, voxel_size, source).fingerprint();
    if features.is_empty() {
        return ground;
    }
    let recipe = serde_json::to_string(features).expect("a feature recipe serializes");
    recipe
        .bytes()
        .fold(ground, |h, byte| (h ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3))
        .max(1)
}

/// The world without its features.
fn ground_recipe(
    shape: VoxelWorldShape,
    planet_radius: f64,
    plane_size: f64,
    voxel_size: f64,
    source: TerrainSource,
) -> PlanetRecipe {
    PlanetRecipe {
        shape: match shape {
            VoxelWorldShape::Sphere => Shape::Sphere,
            VoxelWorldShape::Plane => Shape::Plane,
            VoxelWorldShape::InfinitePlane => Shape::InfinitePlane,
        },
        radius_m: planet_radius,
        plane_size_m: plane_size,
        voxel_size_m: voxel_size,
        terrain: source,
        ..PlanetRecipe::default()
    }
}

/// A journal edit as a world brush.
pub fn planet_brush(edit: &VoxelBrushEdit) -> Brush {
    Brush {
        center: edit.center,
        radius: edit.radius,
        shape: match edit.shape {
            VoxelBrushShape::Sphere => BrushShape::Sphere,
            VoxelBrushShape::Cube => BrushShape::Cube,
        },
        op: match edit.op {
            VoxelBrushOp::Remove => BrushOp::Remove,
            VoxelBrushOp::Add => BrushOp::Add,
            VoxelBrushOp::Paint => BrushOp::Paint,
        },
        material: edit.material,
        height: edit.height,
    }
}

/// Engine class of the component holding a generator's settings, as the
/// generator declares it.
pub fn generator_settings_component(id: &str, version: u32) -> Option<String> {
    terrain::find(id, version).and_then(|generator| generator.info().settings_component)
}

/// The generator settings (JSON) of a terrain instance: its owner object's
/// settings component serialized when present, else `generator_parameters`.
pub fn generator_settings(
    world: &World,
    entity: Entity,
    component: &VoxelTerrainComponent,
) -> String {
    let owner = pulsar_scene_model::attachments::owner_of(world, entity).unwrap_or(entity);
    generator_settings_component(&component.generator.id, component.generator.version)
        .and_then(|class| {
            let settings =
                pulsar_world_registry::instances::resolve_instance(world, owner, &class, 0)?;
            pulsar_world_registry::get_world_component_as_engine_class(&class, world, settings)
        })
        .and_then(|settings| settings.to_json().ok())
        .map(|json| json.to_string())
        .unwrap_or_else(|| component.generator_parameters.clone())
}

/// Whether the terrain is streamed from a registered terrain generator (as
/// opposed to externally supplied sample data).
pub fn is_generated(component: &VoxelTerrainComponent) -> bool {
    terrain::find(&component.generator.id, component.generator.version).is_some()
}

/// The recipe of a terrain instance, with its owner's uniform scale, without
/// its features, and the features' recipe (grown when a world is built).
fn entity_recipe(
    world: &World,
    entity: Entity,
    component: &VoxelTerrainComponent,
) -> Result<(PlanetRecipe, FeatureRecipe), String> {
    let transform = pulsar_scene_model::attachments::owner_component::<Transform>(world, entity)
        .copied()
        .unwrap_or_default();
    if transform.position.iter().any(|v| v.abs() > 1.0e-6)
        || transform.rotation.iter().any(|v| v.abs() > 1.0e-5)
    {
        return Err("a voxel world is centred on the world origin; move the entity to (0, 0, 0) without rotation".into());
    }
    let [sx, sy, sz] = transform.scale.map(f64::from);
    if !sx.is_finite() || sx <= 0.0 || (sx - sy).abs() > 1.0e-5 || (sx - sz).abs() > 1.0e-5 {
        return Err("voxel worlds require a positive uniform scale".into());
    }
    let source = TerrainSource {
        generator: component.generator.id.clone(),
        version: component.generator.version,
        seed: component.seed,
        settings: generator_settings(world, entity, component),
    };
    Ok((
        ground_recipe(
            component.shape,
            component.planet_radius * sx,
            component.plane_size * sx,
            component.voxel_size * sx,
            source,
        ),
        feature_recipe(&component.features),
    ))
}

/// `recipe` with `features` grown with its seed in cells of its voxel size.
fn grown(recipe: PlanetRecipe, features: &FeatureRecipe) -> PlanetRecipe {
    PlanetRecipe {
        features: features.grow(recipe.voxel_size_m, recipe.terrain.seed),
        ..recipe
    }
}

struct CachedWorld {
    recipe: PlanetRecipe,
    features: FeatureRecipe,
    edits: VoxelEditJournal,
    planet: Arc<Planet>,
}

/// Worlds by scene and terrain entity (scenes live side by side: the
/// editor's and a play session's, or tests'). A newer journal that only
/// appends edits extends the cached world.
static WORLDS: Mutex<Option<HashMap<(usize, u64), CachedWorld>>> = Mutex::new(None);

/// The CPU world of a terrain instance entity: its generated terrain with
/// every journal edit applied.
pub fn terrain_world(world: &World, entity: Entity) -> Result<Arc<Planet>, String> {
    let component = world
        .get::<VoxelTerrainComponent>(entity)
        .ok_or("the entity has no voxel terrain")?;
    if !component.enabled || !pulsar_scene_model::attachments::is_enabled(world, entity) {
        return Err("the voxel terrain is disabled".into());
    }
    if !is_generated(component) {
        return Err(format!(
            "unknown terrain generator {} v{}",
            component.generator.id, component.generator.version
        ));
    }
    let (recipe, features) = entity_recipe(world, entity, component)?;
    let mut worlds = WORLDS.lock().unwrap_or_else(|e| e.into_inner());
    let worlds = worlds.get_or_insert_with(HashMap::new);
    // A scene's address names it while it lives; a later scene at the same
    // address only finds a world whose recipe and edits match its own.
    let key = (world as *const World as usize, entity.bits());
    if let Some(cached) = worlds.get_mut(&key) {
        if cached.recipe == recipe
            && cached.features == features
            && component.edits.starts_with(&cached.edits)
        {
            let start = cached.edits.len();
            if component.edits.len() > start {
                // A journal extended elsewhere (an editor's strokes): the
                // world extended in place, or a copy of it (a copy shares
                // its caches and most of its edits: microseconds).
                let planet = Arc::make_mut(&mut cached.planet);
                for edit in component.edits.iter_from(start) {
                    planet.apply(planet_brush(edit))?;
                }
                cached.edits = component.edits.clone();
            }
            return Ok(Arc::clone(&cached.planet));
        }
    }
    let planet = Arc::new(journal_planet(grown(recipe.clone(), &features), &component.edits)?);
    if worlds.len() >= 16 && !worlds.contains_key(&key) {
        worlds.clear();
    }
    worlds.insert(
        key,
        CachedWorld {
            recipe,
            features,
            edits: component.edits.clone(),
            planet: Arc::clone(&planet),
        },
    );
    Ok(planet)
}

/// Edits a level keeps listed (undoable) when it folds older ones into its
/// journal's base ([`compact_journal`]).
pub const LISTED_EDITS: usize = 4096;

/// The world of `recipe` with a journal's edits: its base snapshot loaded,
/// then the listed edits applied.
pub fn journal_planet(recipe: PlanetRecipe, edits: &VoxelEditJournal) -> Result<Planet, String> {
    journal_planet_to(recipe, edits, edits.len())
}

/// [`journal_planet`] with only the first `count` edits (not fewer than the
/// base holds).
fn journal_planet_to(
    recipe: PlanetRecipe,
    edits: &VoxelEditJournal,
    count: usize,
) -> Result<Planet, String> {
    let mut planet = match edits.base() {
        Some(base) => {
            let planet = Planet::from_snapshot(recipe, &base.snapshot)
                .map_err(|e| format!("voxel edits: {e}"))?;
            if planet.edits().len() != base.brushes {
                return Err(format!(
                    "voxel edits: the base holds {} edits, its snapshot {}",
                    base.brushes,
                    planet.edits().len()
                ));
            }
            planet
        }
        None => Planet::new(recipe)?,
    };
    let start = edits.base_len();
    for edit in edits.iter_from(start).take(count.saturating_sub(start)) {
        planet.apply(planet_brush(edit))?;
    }
    Ok(planet)
}

/// A base for `edits` folding all but its latest `keep` edits into a
/// snapshot of the world they left, or `None` when there is nothing more
/// to fold. [`VoxelEditJournal::compact`] takes it.
pub fn compact_journal(
    recipe: PlanetRecipe,
    edits: &VoxelEditJournal,
    keep: usize,
) -> Result<Option<VoxelEditBase>, String> {
    let brushes = edits.len().saturating_sub(keep);
    if brushes <= edits.base_len() {
        return Ok(None);
    }
    let planet = journal_planet_to(recipe, edits, brushes)?;
    let hash = edits.prefix_hash(brushes).expect("after the base");
    Ok(Some(VoxelEditBase {
        brushes,
        hash,
        snapshot: planet.snapshot().into(),
    }))
}

/// The terrains of `world` with more than [`LISTED_EDITS`] listed edits,
/// with what compacting them needs ([`compact_journal`]): copied out, so a
/// caller builds the bases without holding the scene.
pub fn journals_to_compact(world: &World) -> Vec<(Entity, PlanetRecipe, VoxelEditJournal)> {
    world
        .query::<&VoxelTerrainComponent>()
        .into_iter()
        .filter(|(_, c)| c.edits.len() - c.edits.base_len() > LISTED_EDITS && is_generated(c))
        .filter_map(|(entity, c)| {
            let (recipe, features) = entity_recipe(world, entity, c).ok()?;
            Some((entity, grown(recipe, &features), c.edits.clone()))
        })
        .collect()
}

/// Append edits to a terrain's journal after checking that each applies.
/// They are applied once, to the terrain's world (`terrain_world` then
/// finds them there), and the blocks each broke, placed and recoloured are
/// queued as events, counted from what it baked (`Planet::edit_yield`):
/// cell by cell for a small edit, by region past
/// `edit_yield::EXACT_CELLS` cells, so a planet-scale blast costs what a
/// small one does.
pub fn append_edits(
    world: &mut World,
    entity: Entity,
    edits: Vec<VoxelBrushEdit>,
) -> Result<(), String> {
    let planet = terrain_world(world, entity)?;
    let grid = *planet.grid();
    drop(planet);
    let component = world
        .get::<VoxelTerrainComponent>(entity)
        .ok_or("the entity has no voxel terrain")?;
    if !component.editable {
        return Err("the voxel terrain is not editable".into());
    }
    for edit in &edits {
        if edit.op != VoxelBrushOp::Remove
            && (edit.material == material::AIR || edit.material >= material::COUNT)
        {
            return Err(format!(
                "material {} is not a solid terrain material (1 to {})",
                edit.material,
                material::COUNT - 1
            ));
        }
        planet_brush(edit).resolve(&grid)?;
    }
    // In order, each brush's yield taken from the world as the brushes
    // before it left it.
    let mut broken = Vec::new();
    let mut placed = Vec::new();
    let mut changed = Vec::new();
    for counted in extend_world(world, entity, &edits)? {
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
    let mut component = world
        .get_mut::<VoxelTerrainComponent>(entity)
        .ok_or("the entity has no voxel terrain")?;
    component.edits.extend(edits);
    component.pending_block_broken.extend(broken);
    component.pending_block_placed.extend(placed);
    component.pending_block_material_changed.extend(changed);
    component.source_revision = component.source_revision.wrapping_add(1);
    Ok(())
}

/// Apply `edits` to the cached world of the terrain (in sync with its
/// journal: `terrain_world` just made it so), recording them as applied;
/// each one's yield.
fn extend_world(
    world: &World,
    entity: Entity,
    edits: &[VoxelBrushEdit],
) -> Result<Vec<helio_pass_voxel_planet::edit_yield::EditYield>, String> {
    let mut worlds = WORLDS.lock().unwrap_or_else(|e| e.into_inner());
    let key = (world as *const World as usize, entity.bits());
    let cached = worlds
        .get_or_insert_with(HashMap::new)
        .get_mut(&key)
        .ok_or("the terrain's world is not built")?;
    let planet = Arc::make_mut(&mut cached.planet);
    let mut yields = Vec::with_capacity(edits.len());
    for edit in edits {
        let (_, counted) = planet.apply_counted(planet_brush(edit))?;
        yields.push(counted);
    }
    cached.edits.extend(edits.iter().cloned());
    Ok(yields)
}


/// The edit that makes the cell containing `p` exactly `material` (0 air).
pub fn block_edit(planet: &Planet, p: DVec3, material: u32) -> VoxelBrushEdit {
    let grid = planet.grid();
    let (cell, _) = grid.locate(p);
    VoxelBrushEdit {
        center: grid.cell_center(cell).to_array(),
        // Contains only its own cell centre.
        radius: grid.voxel_size() * 0.5,
        shape: VoxelBrushShape::Cube,
        op: if material == material::AIR {
            VoxelBrushOp::Remove
        } else {
            VoxelBrushOp::Add
        },
        material,
        height: 0.0,
    }
}

/// The edit that fills (or, with material 0, clears) a sphere or cube.
pub fn shape_edit(
    center: DVec3,
    radius: f64,
    shape: VoxelBrushShape,
    material: u32,
) -> Result<VoxelBrushEdit, String> {
    if !center.is_finite() || !radius.is_finite() || radius <= 0.0 {
        return Err("a fill needs a finite centre and a positive radius".into());
    }
    Ok(VoxelBrushEdit {
        center: center.to_array(),
        radius,
        shape,
        op: if material == material::AIR {
            VoxelBrushOp::Remove
        } else {
            VoxelBrushOp::Add
        },
        material,
        height: 0.0,
    })
}

/// A camera pose framing a voxel world from `eye`: `height` metres above
/// the ground under the eye (on a finite plane, the nearest point well
/// inside it), looking along the local horizon in the direction of
/// `forward`, tilted slightly down. Returns the position and view
/// direction.
pub fn frame_view(planet: &Planet, eye: DVec3, forward: DVec3, height: f64) -> (DVec3, DVec3) {
    let grid = planet.grid();
    let column = match grid.shape() {
        Shape::Sphere => eye.try_normalize().unwrap_or(DVec3::Y) * grid.radius(),
        Shape::Plane => {
            let half = f64::from(grid.cells()) * grid.voxel_size() * 0.4;
            DVec3::new(eye.x.clamp(-half, half), 0.0, eye.z.clamp(-half, half))
        }
        Shape::InfinitePlane => DVec3::new(eye.x, 0.0, eye.z),
    };
    let position = planet.surface_point(column, height);
    let up = grid.up(position);
    let along = forward - up * forward.dot(up);
    let along = along
        .try_normalize()
        .unwrap_or_else(|| up.any_orthonormal_vector());
    let tilt = 0.2f64;
    (position, (along * tilt.cos() - up * tilt.sin()).normalize())
}

fn position(x: f64, y: f64, z: f64) -> Result<DVec3, String> {
    let p = DVec3::new(x, y, z);
    if p.is_finite() {
        Ok(p)
    } else {
        Err("positions must be finite".into())
    }
}

// Scripting surface. Blocks are the world's exact base cells.
#[pulsar_scenedb::component_methods]
impl VoxelTerrainComponent {
    /// Material of the block containing the point (0 when it is air).
    #[world_method(pure, category = "Voxel")]
    fn get_block(world: &World, entity: Entity, x: f64, y: f64, z: f64) -> Result<u32, String> {
        let planet = terrain_world(world, entity)?;
        let (cell, _) = planet.grid().locate(position(x, y, z)?);
        Ok(planet.material(cell))
    }

    /// Make the block containing the point `material` (0 removes it).
    #[world_method(category = "Voxel")]
    fn set_block(
        world: &mut World,
        entity: Entity,
        x: f64,
        y: f64,
        z: f64,
        material: u32,
    ) -> Result<(), String> {
        let planet = terrain_world(world, entity)?;
        let point = position(x, y, z)?;
        let edit = block_edit(&planet, point, material);
        append_edits(world, entity, vec![edit])
    }

    /// Fill every block whose centre lies within `radius` of the point with
    /// `material` (0 digs a hole).
    #[world_method(category = "Voxel")]
    fn fill_sphere(
        world: &mut World,
        entity: Entity,
        x: f64,
        y: f64,
        z: f64,
        radius: f64,
        material: u32,
    ) -> Result<(), String> {
        let edit = shape_edit(
            position(x, y, z)?,
            radius,
            VoxelBrushShape::Sphere,
            material,
        )?;
        append_edits(world, entity, vec![edit])
    }

    /// Fill every block whose centre lies in the cube of half size
    /// `half_size` around the point, aligned with the ground.
    #[world_method(category = "Voxel")]
    fn fill_cube(
        world: &mut World,
        entity: Entity,
        x: f64,
        y: f64,
        z: f64,
        half_size: f64,
        material: u32,
    ) -> Result<(), String> {
        let edit = shape_edit(
            position(x, y, z)?,
            half_size,
            VoxelBrushShape::Cube,
            material,
        )?;
        append_edits(world, entity, vec![edit])
    }

    /// Distance along the ray to the first solid block, or -1 when none is
    /// hit within `max_distance`.
    #[world_method(pure, category = "Voxel")]
    #[allow(clippy::too_many_arguments)]
    fn raycast_distance(
        world: &World,
        entity: Entity,
        x: f64,
        y: f64,
        z: f64,
        dx: f64,
        dy: f64,
        dz: f64,
        max_distance: f64,
    ) -> Result<f64, String> {
        let planet = terrain_world(world, entity)?;
        let direction = position(dx, dy, dz)?;
        if direction.length_squared() == 0.0 {
            return Err("the ray direction is zero".into());
        }
        Ok(planet
            .raycast(position(x, y, z)?, direction, max_distance)
            .map_or(-1.0, |hit| hit.distance))
    }

    /// Edge length of one block in metres.
    #[world_method(pure, category = "Voxel")]
    fn voxel_size(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(terrain_world(world, entity)?.grid().voxel_size())
    }
}

/// The terrain instance `entity` names: the instance itself, or the first
/// terrain instance of its owner object.
fn terrain_instance(world: &World, entity: Entity) -> Result<Entity, String> {
    if world.get::<VoxelTerrainComponent>(entity).is_some() {
        return Ok(entity);
    }
    let owner = pulsar_scene_model::attachments::owner_of(world, entity).unwrap_or(entity);
    pulsar_world_registry::instances::resolve_instance(world, owner, "VoxelTerrainComponent", 0)
        .ok_or_else(|| "the object has no voxel terrain".into())
}

fn terrain(world: &World, entity: Entity) -> Result<&VoxelTerrainComponent, String> {
    let instance = terrain_instance(world, entity)?;
    world
        .get::<VoxelTerrainComponent>(instance)
        .ok_or_else(|| "the object has no voxel terrain".into())
}

fn terrain_mut(
    world: &mut World,
    entity: Entity,
) -> Result<pulsar_scenedb::Mut<'_, VoxelTerrainComponent>, String> {
    let instance = terrain_instance(world, entity)?;
    world
        .get_mut::<VoxelTerrainComponent>(instance)
        .ok_or_else(|| "the object has no voxel terrain".into())
}

fn positive(value: f64, what: &str) -> Result<f64, String> {
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err(format!("{what} must be positive and finite"))
    }
}

fn non_negative(value: f64, what: &str) -> Result<f32, String> {
    if value.is_finite() && value >= 0.0 {
        Ok(value as f32)
    } else {
        Err(format!("{what} must be zero or more"))
    }
}

// Every setting of the world, for scripts (a game's random worlds): the
// world rebuilds from the new settings, and its sky follows its air.
#[pulsar_scenedb::component_methods]
impl VoxelTerrainComponent {
    /// The world's shape: "sphere", "plane" or "infinite_plane".
    #[world_method(pure, category = "Voxel|World")]
    fn world_shape(world: &World, entity: Entity) -> Result<String, String> {
        Ok(match terrain(world, entity)?.shape {
            VoxelWorldShape::Sphere => "sphere",
            VoxelWorldShape::Plane => "plane",
            VoxelWorldShape::InfinitePlane => "infinite_plane",
        }
        .into())
    }

    #[world_method(category = "Voxel|World")]
    fn set_world_shape(world: &mut World, entity: Entity, shape: String) -> Result<(), String> {
        let shape = match shape.to_ascii_lowercase().as_str() {
            "sphere" | "planet" => VoxelWorldShape::Sphere,
            "plane" => VoxelWorldShape::Plane,
            "infinite_plane" | "infinite" => VoxelWorldShape::InfinitePlane,
            other => {
                return Err(format!(
                    "unknown world shape {other:?} (sphere, plane, infinite_plane)"
                ));
            }
        };
        terrain_mut(world, entity)?.shape = shape;
        Ok(())
    }

    /// A sphere's radius in metres.
    #[world_method(pure, category = "Voxel|World")]
    fn planet_radius(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(terrain(world, entity)?.planet_radius)
    }

    #[world_method(category = "Voxel|World")]
    fn set_planet_radius(world: &mut World, entity: Entity, radius_m: f64) -> Result<(), String> {
        terrain_mut(world, entity)?.planet_radius = positive(radius_m, "a planet's radius")?;
        Ok(())
    }

    /// A finite plane's edge in metres.
    #[world_method(pure, category = "Voxel|World")]
    fn plane_size(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(terrain(world, entity)?.plane_size)
    }

    #[world_method(category = "Voxel|World")]
    fn set_plane_size(world: &mut World, entity: Entity, size_m: f64) -> Result<(), String> {
        terrain_mut(world, entity)?.plane_size = positive(size_m, "a plane's size")?;
        Ok(())
    }

    /// A base voxel's edge in metres, as authored (0.1 to 1).
    #[world_method(category = "Voxel|World")]
    fn set_voxel_size(world: &mut World, entity: Entity, size_m: f64) -> Result<(), String> {
        terrain_mut(world, entity)?.voxel_size =
            positive(size_m, "a voxel's size")?.clamp(0.1, 1.0);
        Ok(())
    }

    /// The generator's seed.
    #[world_method(pure, category = "Voxel|Generation")]
    fn seed(world: &World, entity: Entity) -> Result<i64, String> {
        Ok(terrain(world, entity)?.seed as i64)
    }

    #[world_method(category = "Voxel|Generation")]
    fn set_seed(world: &mut World, entity: Entity, seed: i64) -> Result<(), String> {
        terrain_mut(world, entity)?.seed = seed as u64;
        Ok(())
    }

    /// Replace the world's features with a preset's: "earth", "moon",
    /// "desert" or "none" (`FeatureRecipe::PRESETS`).
    #[world_method(category = "Voxel|Features")]
    fn use_feature_preset(world: &mut World, entity: Entity, name: String) -> Result<(), String> {
        let preset = FeatureRecipe::preset(&name).ok_or_else(|| {
            format!(
                "unknown feature preset {name:?} ({})",
                FeatureRecipe::PRESETS.join(", ")
            )
        })?;
        terrain_mut(world, entity)?.features = super::feature_spawns(&preset);
        Ok(())
    }

    /// Number of feature spawns (what grows where).
    #[world_method(pure, category = "Voxel|Features")]
    fn feature_spawn_count(world: &World, entity: Entity) -> Result<u32, String> {
        Ok(terrain(world, entity)?.features.len() as u32)
    }

    /// The world's air: "custom" (the authored sky), "none", "earthlike",
    /// "thin" or "dense".
    #[world_method(pure, category = "Voxel|Environment")]
    fn air(world: &World, entity: Entity) -> Result<String, String> {
        Ok(terrain(world, entity)?.air.name().into())
    }

    #[world_method(category = "Voxel|Environment")]
    fn set_air(world: &mut World, entity: Entity, air: String) -> Result<(), String> {
        let air = VoxelAir::from_name(&air)
            .ok_or_else(|| format!("unknown air {air:?} ({})", VoxelAir::NAMES.join(", ")))?;
        terrain_mut(world, entity)?.air = air;
        Ok(())
    }

    /// How much air (1: the preset's).
    #[world_method(pure, category = "Voxel|Environment")]
    fn air_density(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(f64::from(terrain(world, entity)?.air_density))
    }

    #[world_method(category = "Voxel|Environment")]
    fn set_air_density(world: &mut World, entity: Entity, density: f64) -> Result<(), String> {
        terrain_mut(world, entity)?.air_density = non_negative(density, "air density")?;
        Ok(())
    }

    /// The sky's tint (each channel times the preset's).
    #[world_method(category = "Voxel|Environment")]
    fn set_air_tint(
        world: &mut World,
        entity: Entity,
        red: f64,
        green: f64,
        blue: f64,
    ) -> Result<(), String> {
        let tint = [
            non_negative(red, "a tint")?,
            non_negative(green, "a tint")?,
            non_negative(blue, "a tint")?,
        ];
        terrain_mut(world, entity)?.air_tint = tint;
        Ok(())
    }

    #[world_method(pure, category = "Voxel|Environment")]
    fn air_tint_red(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(f64::from(terrain(world, entity)?.air_tint[0]))
    }

    #[world_method(pure, category = "Voxel|Environment")]
    fn air_tint_green(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(f64::from(terrain(world, entity)?.air_tint[1]))
    }

    #[world_method(pure, category = "Voxel|Environment")]
    fn air_tint_blue(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(f64::from(terrain(world, entity)?.air_tint[2]))
    }

    /// Haze and dust (1: the preset's).
    #[world_method(pure, category = "Voxel|Environment")]
    fn air_haze(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(f64::from(terrain(world, entity)?.air_haze))
    }

    #[world_method(category = "Voxel|Environment")]
    fn set_air_haze(world: &mut World, entity: Entity, haze: f64) -> Result<(), String> {
        terrain_mut(world, entity)?.air_haze = non_negative(haze, "haze")?;
        Ok(())
    }

    /// How smooth the world's surfaces are lit (0: blocks, 1: the smooth
    /// surface its voxels approximate).
    #[world_method(pure, category = "Voxel|Appearance")]
    fn smoothness(world: &World, entity: Entity) -> Result<f64, String> {
        Ok(f64::from(terrain(world, entity)?.smoothness))
    }

    #[world_method(category = "Voxel|Appearance")]
    fn set_smoothness(world: &mut World, entity: Entity, smoothness: f64) -> Result<(), String> {
        if !smoothness.is_finite() {
            return Err("smoothness must be finite".into());
        }
        terrain_mut(world, entity)?.smoothness = smoothness.clamp(0.0, 1.0) as f32;
        Ok(())
    }

    /// Whether the world can be dug and built.
    #[world_method(pure, category = "Voxel|Editing")]
    fn is_editable(world: &World, entity: Entity) -> Result<bool, String> {
        Ok(terrain(world, entity)?.editable)
    }

    #[world_method(category = "Voxel|Editing")]
    fn set_editable(world: &mut World, entity: Entity, editable: bool) -> Result<(), String> {
        terrain_mut(world, entity)?.editable = editable;
        Ok(())
    }
}

/// The layers instance `entity` names: the instance itself, or the first
/// layers instance of its owner object (a sibling component or the object).
fn layers_instance(world: &World, entity: Entity) -> Result<Entity, String> {
    if world.get::<VoxelTerrainLayersComponent>(entity).is_some() {
        return Ok(entity);
    }
    let owner = pulsar_scene_model::attachments::owner_of(world, entity).unwrap_or(entity);
    pulsar_world_registry::instances::resolve_instance(
        world,
        owner,
        "VoxelTerrainLayersComponent",
        0,
    )
    .ok_or_else(|| "the object has no terrain layers".into())
}

fn layers_mut(
    world: &mut World,
    entity: Entity,
) -> Result<pulsar_scenedb::Mut<'_, VoxelTerrainLayersComponent>, String> {
    let instance = layers_instance(world, entity)?;
    world
        .get_mut::<VoxelTerrainLayersComponent>(instance)
        .ok_or_else(|| "the object has no terrain layers".into())
}

fn layer_mut(stack: &mut VoxelTerrainStack, index: u32) -> Result<&mut VoxelTerrainLayer, String> {
    let count = stack.layers.len();
    stack
        .layers
        .get_mut(index as usize)
        .ok_or_else(|| format!("layer {index} out of {count}"))
}

// Scripting surface of the layer stack: a game shapes (or randomizes) its
// worlds by changing layers; the terrain rebuilds from the new settings.
#[pulsar_scenedb::component_methods]
impl VoxelTerrainLayersComponent {
    /// Replace the stack with a preset: "earth", "moon", "desert" or "flat"
    /// (`VoxelTerrainStack::PRESETS`).
    #[world_method(category = "Voxel")]
    fn use_preset(world: &mut World, entity: Entity, name: String) -> Result<(), String> {
        let preset = VoxelTerrainStack::preset(&name).ok_or_else(|| {
            format!(
                "unknown terrain preset {name:?} ({})",
                VoxelTerrainStack::PRESETS.join(", ")
            )
        })?;
        layers_mut(world, entity)?.stack = preset;
        Ok(())
    }

    /// Number of layers in the stack (enabled or not).
    #[world_method(pure, category = "Voxel")]
    fn layer_count(world: &World, entity: Entity) -> Result<u32, String> {
        let instance = layers_instance(world, entity)?;
        let stack = world
            .get::<VoxelTerrainLayersComponent>(instance)
            .ok_or("the object has no terrain layers")?;
        Ok(stack.stack.layers.len() as u32)
    }

    /// Append a layer of `kind` (Hills, Mountains, Craters, ...) with its
    /// default parameters; returns its index.
    #[world_method(category = "Voxel")]
    fn add_layer(world: &mut World, entity: Entity, kind: String) -> Result<u32, String> {
        let kind: VoxelLayerKind = serde_json::from_value(serde_json::Value::String(kind.clone()))
            .map_err(|_| format!("unknown layer kind {kind:?}"))?;
        let mut component = layers_mut(world, entity)?;
        component.stack.layers.push(VoxelTerrainLayer::new(kind));
        Ok(component.stack.layers.len() as u32 - 1)
    }

    #[world_method(category = "Voxel")]
    fn remove_layer(world: &mut World, entity: Entity, index: u32) -> Result<(), String> {
        let mut component = layers_mut(world, entity)?;
        layer_mut(&mut component.stack, index)?;
        component.stack.layers.remove(index as usize);
        Ok(())
    }

    #[world_method(category = "Voxel")]
    fn set_layer_enabled(
        world: &mut World,
        entity: Entity,
        index: u32,
        enabled: bool,
    ) -> Result<(), String> {
        layer_mut(&mut layers_mut(world, entity)?.stack, index)?.enabled = enabled;
        Ok(())
    }

    /// The layer's main height in metres (amplitude, depth or plateau height).
    #[world_method(category = "Voxel")]
    fn set_layer_height(
        world: &mut World,
        entity: Entity,
        index: u32,
        height_m: f64,
    ) -> Result<(), String> {
        layer_mut(&mut layers_mut(world, entity)?.stack, index)?.height_m = height_m;
        Ok(())
    }

    /// The layer's scale in kilometres (first wavelength or largest feature).
    #[world_method(category = "Voxel")]
    fn set_layer_scale(
        world: &mut World,
        entity: Entity,
        index: u32,
        scale_km: f64,
    ) -> Result<(), String> {
        layer_mut(&mut layers_mut(world, entity)?.stack, index)?.scale_km = scale_km;
        Ok(())
    }

    /// The share of the surface the layer covers (regions, basins, craters).
    #[world_method(category = "Voxel")]
    fn set_layer_coverage(
        world: &mut World,
        entity: Entity,
        index: u32,
        coverage: f64,
    ) -> Result<(), String> {
        layer_mut(&mut layers_mut(world, entity)?.stack, index)?.coverage = coverage;
        Ok(())
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;

    /// A level saved with its old edits folded into a base loads the same
    /// world as every edit replayed, and keeps taking edits.
    #[test]
    fn compacted_journals_save_and_load_the_same_world() {
        let recipe = PlanetRecipe {
            shape: Shape::Plane,
            plane_size_m: 512.0,
            ..PlanetRecipe::default()
        };
        let ground = Planet::new(recipe.clone())
            .unwrap()
            .surface_point(DVec3::ZERO, 0.0);
        let edit = |n: usize| VoxelBrushEdit {
            center: (ground
                + DVec3::new(
                    (n % 70) as f64 * 0.13 - 4.0,
                    (n % 9) as f64 * 0.1 - 0.6,
                    (n / 70) as f64 * 0.11 - 4.0,
                ))
            .to_array(),
            radius: if n % 400 == 3 {
                4.0
            } else {
                0.15 + (n % 4) as f64 * 0.1
            },
            shape: if n % 2 == 0 {
                VoxelBrushShape::Sphere
            } else {
                VoxelBrushShape::Cube
            },
            op: [VoxelBrushOp::Remove, VoxelBrushOp::Add, VoxelBrushOp::Paint][n % 3],
            material: 1 + (n % 9) as u32,
            height: 0.0,
        };
        let full: VoxelEditJournal = (0..LISTED_EDITS + 900).map(edit).collect();
        let replayed = journal_planet(recipe.clone(), &full).unwrap();

        let mut saved = full.clone();
        let base = compact_journal(recipe.clone(), &saved, LISTED_EDITS)
            .unwrap()
            .expect("old edits to fold");
        assert_eq!(base.brushes, 900);
        assert!(saved.compact(base));
        assert_eq!(saved, full);
        assert!(
            compact_journal(recipe.clone(), &saved, LISTED_EDITS)
                .unwrap()
                .is_none(),
            "nothing more to fold"
        );
        let json = serde_json::to_string(&saved).unwrap();
        let mut loaded: VoxelEditJournal = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.listed().count(), LISTED_EDITS);

        let world = journal_planet(recipe.clone(), &loaded).unwrap();
        assert_eq!(
            (world.edits().len(), world.edits().hash()),
            (replayed.edits().len(), replayed.edits().hash())
        );
        let g = *world.grid();
        let (cell, _) = g.locate(ground);
        for di in -40..40 {
            for dk in -60..20 {
                let c = helio_pass_voxel_planet::Cell::new(
                    cell.face,
                    cell.i + di,
                    cell.j + di / 2,
                    cell.k + dk,
                );
                assert_eq!(world.material(c), replayed.material(c), "{c:?}");
            }
        }
        // Edits continue on the loaded journal.
        loaded.push(edit(7));
        assert!(loaded.starts_with(&full));
        let mut more = full.clone();
        more.push(edit(7));
        assert_eq!(
            journal_planet(recipe.clone(), &loaded)
                .unwrap()
                .edits()
                .hash(),
            journal_planet(recipe, &more).unwrap().edits().hash()
        );
    }
}
