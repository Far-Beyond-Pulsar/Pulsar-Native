use std::collections::HashMap;
use std::path::{Path, PathBuf};

use helio::{MaterialId, MeshId, MeshUpload};

/// Per-component-instance foliage handles, keyed by `scene_object_id:component_index`.
///
/// Foliage types/layers/interactors are *not* re-created every sync pass (unlike
/// lights): `add_foliage_type` re-rolls GPU placement on every publication
/// generation bump, so the editor sync pass would re-roll the whole field every
/// frame. Instead the first sync registers the handles and later syncs update
/// them in place, gated by the stored hashes.
pub struct FoliageCache {
    pub map: HashMap<String, FoliageEntry>,
}

/// Handles and change-detection fingerprints for one foliage component instance.
#[derive(Clone, Copy)]
pub struct FoliageEntry {
    pub type_id: helio::FoliageTypeId,
    pub layer_id: helio::FoliageLayerId,
    pub interactor_id: helio::FoliageInteractorId,
    pub material_id: helio::MaterialId,
    pub material_hash: u64,
    pub descriptor_hash: u64,
    pub bounds_hash: u64,
    pub wind_hash: u64,
}

impl FoliageCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }
}

/// Tear down one foliage component instance from the scene, removing the type
/// (which re-rolls its placement) before its material so the material is not
/// tombstoned while referenced. Returns `false` when the key was not cached.
pub fn remove_foliage_handles(
    scene: &mut helio::Scene,
    cache: &mut FoliageCache,
    key: &str,
) -> bool {
    let Some(entry) = cache.map.remove(key) else {
        return false;
    };
    let _ = scene.remove_foliage_type(entry.type_id);
    let _ = scene.remove_foliage_layer(entry.layer_id);
    let _ = scene.remove_foliage_interactor(entry.interactor_id);
    let _ = scene.remove_material(entry.material_id);
    true
}

/// Cache of GPU-uploaded mesh geometry, keyed by the resolved asset path.
///
/// Registered as a subsystem by both the game loader and editor contexts.
/// Components check this cache before loading and uploading mesh files.
pub struct MeshCache {
    pub upload_cache: HashMap<String, (MeshId, MaterialId)>,
}

impl MeshCache {
    pub fn new() -> Self {
        Self {
            upload_cache: HashMap::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<(MeshId, MaterialId)> {
        self.upload_cache.get(key).copied()
    }

    pub fn insert(&mut self, key: String, ids: (MeshId, MaterialId)) {
        self.upload_cache.insert(key, ids);
    }
}

/// Per-object-instance scene cache, keyed by scene-object ID.
///
/// Tracks which scene objects exist per component instance so that
/// the editor can update transforms in-place instead of deleting and
/// re-inserting every frame (which would cascade-free meshes/materials
/// in the helio scene).
pub struct SceneObjectCache {
    /// scene_object_id → (ObjectId, mesh_asset_path)
    pub map: HashMap<String, (helio::ObjectId, String)>,
}

impl SceneObjectCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get(&self, scene_id: &str) -> Option<(helio::ObjectId, &str)> {
        self.map
            .get(scene_id)
            .map(|(id, path)| (*id, path.as_str()))
    }

    pub fn insert(&mut self, scene_id: String, obj_id: helio::ObjectId, mesh_asset: String) {
        self.map.insert(scene_id, (obj_id, mesh_asset));
    }

    pub fn remove(&mut self, scene_id: &str) -> Option<(helio::ObjectId, String)> {
        self.map.remove(scene_id)
    }
}

/// Per-object light handle, keyed by scene-object ID.
///
/// Helio's lights have no ref-counting and (until recently) no
/// insert-or-update entry point, so the editor sync pass used to remove
/// *every* light in the scene each pass and let each LightComponent
/// unconditionally re-insert. That forced a full scene resync to run
/// whenever anything anywhere changed, just to keep lights from vanishing.
/// Caching the LightId here lets LightComponent call `Scene::update_light`
/// in place instead, the same way SceneObjectCache does for meshes.
pub struct LightCache {
    pub map: HashMap<String, helio::LightId>,
}

impl LightCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get(&self, scene_id: &str) -> Option<helio::LightId> {
        self.map.get(scene_id).copied()
    }

    pub fn insert(&mut self, scene_id: String, id: helio::LightId) {
        self.map.insert(scene_id, id);
    }

    pub fn remove(&mut self, scene_id: &str) -> Option<helio::LightId> {
        self.map.remove(scene_id)
    }
}

/// Same shape as [`LightCache`], for reflection captures (Phase D,
/// Pulsar-Native#558). Unlike objects/lights, `helio::Scene` has no
/// `reflection_capture_by_tag` lookup at all -- there's no tag-based
/// mechanism to ask Helio "do I already have one of these for this scene
/// object", so every `ReflectionCaptureComponent` sync pass needs this
/// editor-side cache to know whether to `insert_reflection_capture` or
/// `update_reflection_capture`.
pub struct ReflectionCaptureCache {
    pub map: HashMap<String, helio::ReflectionCaptureId>,
}

impl ReflectionCaptureCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn get(&self, scene_id: &str) -> Option<helio::ReflectionCaptureId> {
        self.map.get(scene_id).copied()
    }

    pub fn insert(&mut self, scene_id: String, id: helio::ReflectionCaptureId) {
        self.map.insert(scene_id, id);
    }

    pub fn remove(&mut self, scene_id: &str) -> Option<helio::ReflectionCaptureId> {
        self.map.remove(scene_id)
    }
}

/// One side of a to-be-paired portal — this object's current pose and
/// opening size, recorded by `PortalComponent::sync_component` every sync
/// pass under the `portal_id` the level designer chose to link two
/// components together. See [`PortalLinkCache`].
#[derive(Clone, Copy, Debug)]
pub struct PortalSide {
    pub position: [f32; 3],
    pub rotation: [f32; 3],
    /// Half-width/half-height of the opening.
    pub half_extent: [f32; 2],
}

fn side_pose(side: &PortalSide) -> helio::PortalPose {
    let q = glam::Quat::from_euler(
        glam::EulerRot::YXZ,
        side.rotation[1].to_radians(),
        side.rotation[0].to_radians(),
        side.rotation[2].to_radians(),
    );
    helio::portal_pose_facing(
        glam::Vec3::from_array(side.position),
        q * glam::Vec3::NEG_Z,
        q * glam::Vec3::Y,
    )
}

/// Live-key namespaced by `portal_id` — two different `PortalComponent`s on
/// two different objects always produce different keys even if (unusually)
/// the same object hosted two portals under two different IDs.
pub fn portal_link_key(portal_id: i32, scene_object_id: &str) -> String {
    format!("portal:{portal_id}:{scene_object_id}")
}

/// What, if anything, needs to happen to the real `helio::Scene` portal for
/// one `portal_id` after a side was recorded, dropped, or swept as stale.
/// Deliberately just data — no scene/cache borrow attached — so it can be
/// computed by [`PortalLinkCache`] alone and applied moments later via
/// [`apply_portal_pair_action`]. See that function's doc for why the split.
pub enum PortalPairAction {
    /// Nothing changed (still zero or one side registered, or the pair was
    /// already up to date).
    None,
    /// Both sides are now present and no helio portal exists yet.
    Create {
        portal_id: i32,
        a: helio::PortalPose,
        b: helio::PortalPose,
        half_extent: glam::Vec2,
    },
    /// Both sides are still present and a helio portal already exists —
    /// just refresh its pose/size in place.
    Update {
        id: helio::PortalId,
        a: helio::PortalPose,
        b: helio::PortalPose,
        half_extent: glam::Vec2,
    },
    /// A side disappeared (disabled, deleted, or swept as stale) and a
    /// helio portal existed for the pair — tear it down.
    Remove { portal_id: i32, id: helio::PortalId },
}

/// Applies `action` to `scene` (create/update/remove the real portal) and
/// reports back what [`PortalLinkCache::set_active`]/`clear_active` needs to
/// be told as a result — `Some((portal_id, Some(id)))` for a newly created
/// portal, `Some((portal_id, None))` for one just removed, `None` for an
/// in-place update or no-op.
///
/// Split from `PortalLinkCache` itself (which never touches `helio::Scene`
/// directly) because a `ComponentRuntimeContext` can only hand out one
/// mutable subsystem borrow at a time (`Subsystems::get_mut` takes `&mut
/// self`) — there is no way to hold `&mut Renderer` and `&mut
/// PortalLinkCache` simultaneously through that interface. Computing the
/// action from `PortalLinkCache` alone, then applying it here against a
/// `Renderer`-borrowed scene, then (if needed) feeding the tiny result back
/// into `PortalLinkCache` as a third, separate borrow, avoids ever needing
/// two subsystems live at once. `PortalComponent::sync_component` and the
/// editor's stale-portal sweep both go through this same function.
pub fn apply_portal_pair_action(
    scene: &mut helio::Scene,
    action: PortalPairAction,
) -> Option<(i32, Option<helio::PortalId>)> {
    match action {
        PortalPairAction::None => None,
        PortalPairAction::Create {
            portal_id,
            a,
            b,
            half_extent,
        } => scene
            .add_portal(helio::PortalDescriptor { a, b, half_extent })
            .ok()
            .map(|id| (portal_id, Some(id))),
        PortalPairAction::Update {
            id,
            a,
            b,
            half_extent,
        } => {
            let _ = scene.update_portal_pose(id, a, b);
            let _ = scene.update_portal_half_extent(id, half_extent);
            None
        }
        PortalPairAction::Remove { portal_id, id } => {
            let _ = scene.remove_portal(id);
            Some((portal_id, None))
        }
    }
}

/// Pairs up to two [`PortalComponent`](crate::PortalComponent) instances
/// that share the same `portal_id` into one real `helio::Scene` portal.
///
/// Portals are inherently a *pair*, unlike every other component here (one
/// scene object → one helio actor): a lone `PortalComponent` has nothing to
/// connect to. `PortalComponent::sync_component` records this object's
/// current side into `sides` under its `portal_id` on every sync — whichever
/// side is synced *second* (order isn't guaranteed) finds the pair complete
/// and creates the real portal; if the paired object just moved, the
/// already-complete pair is updated in place instead. Losing a side (its
/// component disabled, or its object deleted/swept as stale) tears the real
/// portal back down, leaving the surviving side registered and waiting for
/// a new partner.
///
/// Keyed by `(portal_id, scene_object_id)` so either side updating doesn't
/// disturb the other, and so a *third* object claiming an already-paired ID
/// is rejected (only two may ever share one ID) rather than silently
/// replacing one side.
#[derive(Default)]
pub struct PortalLinkCache {
    /// portal_id → (scene_object_id → that object's current side).
    sides: HashMap<i32, HashMap<String, PortalSide>>,
    /// portal_id → the live helio portal, once exactly two sides are present.
    active: HashMap<i32, helio::PortalId>,
}

impl PortalLinkCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `scene_object_id`'s current side for `portal_id` and returns
    /// whatever action is now needed. Errs (without recording) if this would
    /// be a *third* distinct object claiming `portal_id`.
    pub fn record_side(
        &mut self,
        portal_id: i32,
        scene_object_id: &str,
        side: PortalSide,
    ) -> Result<PortalPairAction, String> {
        let entry = self.sides.entry(portal_id).or_default();
        if entry.len() >= 2 && !entry.contains_key(scene_object_id) {
            return Err(format!(
                "portal_id {portal_id} already links two other objects — only two PortalComponents may share an ID"
            ));
        }
        entry.insert(scene_object_id.to_string(), side);
        Ok(self.resolve(portal_id))
    }

    /// Drops `scene_object_id`'s side for `portal_id` (component disabled,
    /// or swept as stale) and returns whatever action is now needed.
    pub fn record_absent(&mut self, portal_id: i32, scene_object_id: &str) -> PortalPairAction {
        if let Some(entry) = self.sides.get_mut(&portal_id) {
            entry.remove(scene_object_id);
            if entry.is_empty() {
                self.sides.remove(&portal_id);
            }
        }
        self.resolve(portal_id)
    }

    /// Drops every side not present in `live_keys` this pass (its object was
    /// deleted, or its `PortalComponent` otherwise stopped being synced) and
    /// returns the resulting action for each affected `portal_id`.
    pub fn remove_stale(&mut self, live_keys: &pulsar_reflection::LiveKeySet) -> Vec<PortalPairAction> {
        let portal_ids: Vec<i32> = self.sides.keys().copied().collect();
        let mut actions = Vec::new();
        for portal_id in portal_ids {
            let Some(entry) = self.sides.get_mut(&portal_id) else {
                continue;
            };
            let stale: Vec<String> = entry
                .keys()
                .filter(|obj_id| !live_keys.contains(&portal_link_key(portal_id, obj_id)))
                .cloned()
                .collect();
            if stale.is_empty() {
                continue;
            }
            for obj_id in stale {
                entry.remove(&obj_id);
            }
            if entry.is_empty() {
                self.sides.remove(&portal_id);
            }
            actions.push(self.resolve(portal_id));
        }
        actions
    }

    /// Records the `PortalId` a `Create` action just produced.
    pub fn set_active(&mut self, portal_id: i32, id: helio::PortalId) {
        self.active.insert(portal_id, id);
    }

    /// Forgets the `PortalId` a `Remove` action just tore down.
    pub fn clear_active(&mut self, portal_id: i32) {
        self.active.remove(&portal_id);
    }

    /// Compares how many sides `portal_id` currently has against whether a
    /// helio portal already exists for it, and decides what — if anything —
    /// needs to change.
    fn resolve(&mut self, portal_id: i32) -> PortalPairAction {
        let pair = self.sides.get(&portal_id).and_then(|s| {
            if s.len() != 2 {
                return None;
            }
            let mut values = s.values().copied();
            Some((values.next().unwrap(), values.next().unwrap()))
        });
        let existing = self.active.get(&portal_id).copied();
        match (pair, existing) {
            (Some((a_side, b_side)), Some(id)) => PortalPairAction::Update {
                id,
                a: side_pose(&a_side),
                b: side_pose(&b_side),
                half_extent: glam::Vec2::from(a_side.half_extent),
            },
            (Some((a_side, b_side)), None) => PortalPairAction::Create {
                portal_id,
                a: side_pose(&a_side),
                b: side_pose(&b_side),
                half_extent: glam::Vec2::from(a_side.half_extent),
            },
            (None, Some(id)) => PortalPairAction::Remove { portal_id, id },
            (None, None) => PortalPairAction::None,
        }
    }
}

/// The engine's built-in assets — resolved at compile time so embedded
/// primitives (SM_Cube, SM_Sphere, etc.) are always available.
const ENGINE_ASSETS_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../assets");

macro_rules! prim_bytes {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../assets/meshes/primitives/",
            $name,
            ".fbx"
        ))
    };
}

fn embedded_primitive(stem: &str) -> Option<&'static [u8]> {
    match stem {
        "SM_Cube" => Some(prim_bytes!("SM_Cube")),
        "SM_Sphere" => Some(prim_bytes!("SM_Sphere")),
        "SM_Cylinder" => Some(prim_bytes!("SM_Cylinder")),
        "SM_Plane" => Some(prim_bytes!("SM_Plane")),
        "SM_Torus" => Some(prim_bytes!("SM_Torus")),
        _ => None,
    }
}

/// Resolve an asset path to an existing file on disk.
///
/// Checks (in order):
///  1. absolute path
///  2. project-root-relative
///  3. working-directory-relative
///  4. `cwd/assets/` (editor convention)
///  5. engine built-in assets (embedded primitives dir)
pub fn resolve_asset_path(project_root: &Path, asset: &str) -> PathBuf {
    let norm = asset.replace('\\', "/");
    let p = Path::new(&norm);

    if p.is_absolute() && p.exists() {
        return p.to_path_buf();
    }

    let proj = project_root.join(&norm);
    if proj.exists() {
        return proj;
    }

    if let Ok(cwd) = std::env::current_dir() {
        let cwd_path = cwd.join(&norm);
        if cwd_path.exists() {
            return cwd_path;
        }
        let cwd_assets = cwd.join("assets").join(&norm);
        if cwd_assets.exists() {
            return cwd_assets;
        }
    }

    let engine = Path::new(ENGINE_ASSETS_DIR).join(&norm);
    if engine.exists() {
        return engine;
    }

    proj
}

/// Load a mesh file from disk (or from embedded primitive bytes) into a
/// [`MeshUpload`] payload.
///
/// Components call this when they need to load geometry that hasn't been
/// cached yet.  The `path` should already be resolved to an absolute path
/// (use [`resolve_asset_path`] first if needed).
pub fn load_mesh_upload(path: &Path) -> Option<MeshUpload> {
    // Engine-native baked mesh asset (`.mesh`), produced at import time from the
    // source model. Load it directly — no conversion or options (issues #391/#409).
    if path.extension().and_then(|e| e.to_str()) == Some("mesh") {
        return std::fs::read(path)
            .ok()
            .and_then(|bytes| crate::mesh_cache::decode(&bytes));
    }

    let cfg = helio_asset_compat::LoadConfig {
        flip_uv_y: true,
        merge_meshes: false,
        import_scale: glam::Vec3::ONE,
    };

    // Try disk first.
    if path.exists() {
        return helio_asset_compat::load_scene_file_with_config(path, cfg)
            .ok()?
            .meshes
            .into_iter()
            .next()
            .map(|m| MeshUpload {
                vertices: m.vertices,
                indices: m.indices,
            });
    }

    // Fallback: check embedded primitives.
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if let Some(bytes) = embedded_primitive(stem) {
        return helio_asset_compat::load_scene_bytes_with_config(bytes, "fbx", None, cfg)
            .ok()?
            .meshes
            .into_iter()
            .next()
            .map(|m| MeshUpload {
                vertices: m.vertices,
                indices: m.indices,
            });
    }

    None
}
