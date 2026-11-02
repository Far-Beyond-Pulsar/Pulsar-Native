//! Mesh material assets in SceneDB (SceneDB#84): what a static mesh slot's
//! material (its own assignment, or the mesh asset's default for the slot)
//! names.
//!
//! A material is a scalar surface document (`SurfaceMaterialAsset` JSON) or
//! a shader-graph folder (`shader_graph_save.json`, with the textures it
//! samples). [`StaticMeshComponent`]'s handle collector reports one
//! [`AssetId`] per slot material, so the world counts the references however
//! the value was written, loads each material on a worker thread (decoding a
//! graph's textures there too) and binds it into the component's
//! [`BoundMaterials`]. The draw derived for the mesh reads the bound
//! material; it never reads a file. A shader graph is compiled per scene
//! once its textures have slots in that scene's texture store.
//!
//! A material's source is its path as authored, normalized lexically and
//! made project-relative when it is inside the project: the loader resolves
//! it against the project, the working directory and the engine's assets,
//! as the path always was. [`material_sources_for_file`] maps a changed file
//! to the materials it is (a surface document, a graph save, or a texture a
//! loaded graph samples), for `World::asset_source_changed`.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use pulsar_scenedb::{AssetId, AssetPayload, AssetView};

use crate::components::{StaticMeshComponent, StaticMeshMaterialSlot, SurfaceMaterialAsset};
use crate::mesh_cache::ImportedSurfaceMaterial;
use crate::texture_assets::LoadedTexture;

/// The SceneDB asset kind of mesh materials.
pub const MATERIAL_ASSET_KIND: &str = "material";

/// The file a shader-graph material folder saves its graph in.
pub const SHADER_GRAPH_FILE: &str = "shader_graph_save.json";

/// A loaded material: what every slot naming it is bound to.
pub enum LoadedMaterial {
    /// A scalar surface document.
    Surface(ImportedSurfaceMaterial),
    /// A shader graph, compiled per scene (see [`GraphMaterial`]).
    Graph(GraphMaterial),
}

/// A shader-graph material: the graph and the textures it samples, decoded.
pub struct GraphMaterial {
    graph: psgc::GraphDescription,
    /// Each sampled texture, by the asset path the graph names it with.
    textures: Vec<(String, AssetId, Arc<LoadedTexture>)>,
    /// Compiled while the asset loader parses this material, which runs on a
    /// SceneDB worker. Renderer initialization must only register this source;
    /// compiling here would make the first GPU mirror attach wait on PSGC.
    compiled_snippet: String,
    /// Whether the compiled surface reads the frame clock (a `time` node).
    pub reads_time: bool,
}

/// Slot-independent compiled source plus the scene-local residency slots it
/// uses. The source is shared by every scene; only this row projection varies.
struct CompiledGraph {
    hash: u64,
    snippet: String,
}

/// The materials a mesh's slots are bound to, as the world loaded them, each
/// with its content version. Never authored or saved.
#[derive(Clone, Default)]
pub struct BoundMaterials(Vec<(AssetId, u64, Arc<LoadedMaterial>)>);

impl BoundMaterials {
    /// The bound material `id`, with its content version.
    pub fn get(&self, id: AssetId) -> Option<(u64, &Arc<LoadedMaterial>)> {
        if id.is_zero() {
            return None;
        }
        self.0
            .iter()
            .find(|(bound, _, _)| *bound == id)
            .map(|(_, version, material)| (*version, material))
    }
}

impl std::fmt::Debug for BoundMaterials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(|(id, version, _)| (id.0, version)))
            .finish()
    }
}

/// Registers the material asset kind with SceneDB: its loader, and how a
/// [`StaticMeshComponent`] takes in a loaded material. Idempotent; done on
/// the first [`material_asset_id`].
pub fn register_material_assets() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        pulsar_scenedb::register_loader(MATERIAL_ASSET_KIND, load_material_asset);
        // A graph's textures are budgeted by SceneDB's residency cache (see
        // `LoadedTexture`); the material payload is a graph and handles.
        pulsar_scenedb::register_payload_size(MATERIAL_ASSET_KIND, |payload| match payload
            .downcast_ref::<LoadedMaterial>(
        ) {
            Some(LoadedMaterial::Graph(graph)) => graph.textures.len() * 64,
            _ => 0,
        });
        pulsar_scenedb::register_binding::<StaticMeshComponent>(
            MATERIAL_ASSET_KIND,
            bind_mesh_material,
        );
    });
}

/// Lexically normalized `path`: `\` read as `/`, `.` dropped, `..` applied.
fn normalize(path: &str) -> PathBuf {
    let path = path.trim().replace('\\', "/");
    let mut normal = PathBuf::new();
    for component in Path::new(&path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normal.pop() {
                    normal.push(component);
                }
            }
            other => normal.push(other),
        }
    }
    normal
}

/// The source string of the material `path` names (see the module docs).
/// `None` for an empty path. Does not touch the file system.
pub fn material_source(path: &str) -> Option<String> {
    let normal = normalize(path);
    if normal.as_os_str().is_empty() {
        return None;
    }
    if normal.is_absolute() {
        if let Some(root) = engine_state::get_project_path() {
            if let Ok(relative) = normal.strip_prefix(normalize(&root)) {
                return Some(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    Some(normal.to_string_lossy().replace('\\', "/"))
}

/// The asset id of the material `path` names, without declaring it.
pub fn material_asset_id_of(path: &str) -> AssetId {
    material_source(path).map_or(AssetId::ZERO, |source| {
        AssetId::of(MATERIAL_ASSET_KIND, &source)
    })
}

/// The asset id of the material `path` names, declared so the world can
/// load it. [`AssetId::ZERO`] for an empty path.
pub fn material_asset_id(path: &str) -> AssetId {
    let Some(source) = material_source(path) else {
        return AssetId::ZERO;
    };
    register_material_assets();
    let id = AssetId::of(MATERIAL_ASSET_KIND, &source);
    if id.source().is_none() {
        AssetId::declare(MATERIAL_ASSET_KIND, &source);
    }
    id
}

/// The material a slot draws with, as an asset: none for an explicit
/// surface override (which ignores any material) or an empty assignment.
pub fn slot_material_id(slot: &StaticMeshMaterialSlot) -> AssetId {
    if slot.surface_override.is_some() {
        return AssetId::ZERO;
    }
    material_asset_id_of(slot.effective_material_asset())
}

/// `StaticMeshComponent`'s handle collector's material half: one declared
/// id per slot that names a material.
pub(crate) fn collect_mesh_materials(
    mesh: &StaticMeshComponent,
    out: &mut Vec<pulsar_scenedb::handle_ledger::HandleId>,
) {
    for slot in &mesh.material_slots.slots {
        if slot.surface_override.is_none() {
            let id = material_asset_id(slot.effective_material_asset());
            if !id.is_zero() {
                out.push(id.handle());
            }
        }
    }
}

/// How a mesh takes in a loaded (or failed) material: kept for the slots
/// naming it, and only the materials its slots still name are kept.
fn bind_mesh_material(mesh: &mut StaticMeshComponent, view: &AssetView) {
    let named: HashSet<AssetId> = mesh
        .material_slots
        .slots
        .iter()
        .map(slot_material_id)
        .filter(|id| !id.is_zero())
        .collect();
    let bound = &mut mesh.materials.0;
    bound.retain(|(id, _, _)| named.contains(id) && *id != view.id);
    if !named.contains(&view.id) {
        return;
    }
    match view.payload::<LoadedMaterial>() {
        Some(material) => {
            // A graph's textures the residency cache dropped start coming back
            // now, ahead of the scene that will upload them.
            if let LoadedMaterial::Graph(graph) = &*material {
                for (_, _, image) in &graph.textures {
                    image.warm();
                }
            }
            bound.push((view.id, view.version, material))
        }
        None => tracing::warn!(
            source = view.source,
            state = ?view.state,
            "static mesh material could not be loaded; its slots use the imported material"
        ),
    }
}

/// The file the material at `source` is read from: a graph folder's save,
/// or the document itself.
fn resolve_material_file(source: &str) -> PathBuf {
    let path = match engine_state::get_project_path() {
        Some(root) => crate::subsystems::resolve_asset_path(Path::new(&root), source),
        None => PathBuf::from(source),
    };
    if path.is_dir() {
        path.join(SHADER_GRAPH_FILE)
    } else {
        path
    }
}

/// The material kind's loader: runs on a SceneDB worker thread.
fn load_material_asset(source: &str) -> Result<AssetPayload, String> {
    Ok(Arc::new(load_material(source)?))
}

/// Load the material at `source` now (the loader's work, for tools that
/// show a material outside a world).
pub fn load_material(source: &str) -> Result<LoadedMaterial, String> {
    let file = resolve_material_file(source);
    let bytes = std::fs::read(&file)
        .map_err(|error| format!("could not read '{}': {error}", file.display()))?;
    if file
        .file_name()
        .is_some_and(|name| name == SHADER_GRAPH_FILE)
    {
        return load_graph(source, &bytes).map(LoadedMaterial::Graph);
    }
    let material: SurfaceMaterialAsset = serde_json::from_slice(&bytes)
        .map_err(|error| format!("'{}' is not a surface material: {error}", file.display()))?;
    if material.version != 1 {
        return Err(format!(
            "'{}' has unsupported version {}",
            file.display(),
            material.version
        ));
    }
    Ok(LoadedMaterial::Surface(ImportedSurfaceMaterial {
        base_color: material.base_color,
        roughness: material.roughness,
        metallic: material.metallic,
        emissive: material.emissive_color,
        emissive_intensity: material.emissive_intensity,
        alpha: material.alpha,
    }))
}

fn load_graph(source: &str, bytes: &[u8]) -> Result<GraphMaterial, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    // Shader graph saves may carry a line comment before their JSON body.
    let json_start = text
        .find('{')
        .ok_or_else(|| "shader graph JSON object is missing".to_string())?;
    let document: serde_json::Value = serde_json::from_str(&text[json_start..])
        .map_err(|error| format!("invalid shader graph JSON: {error}"))?;
    let graph_value = document
        .get("main_graph")
        .ok_or_else(|| "shader graph asset has no main_graph".to_string())?;
    let graph: psgc::GraphDescription = serde_json::from_value(graph_value.clone())
        .map_err(|error| format!("invalid main_graph: {error}"))?;
    let root = engine_state::get_project_path().map(PathBuf::from);
    let graph_textures = crate::material_graph::texture_assets(&graph)?;
    let mut textures = Vec::new();
    let mut texture_sources = Vec::with_capacity(graph_textures.len());
    for asset in graph_textures {
        let path = match &root {
            Some(root) if Path::new(&asset).is_relative() => root.join(&asset),
            _ => PathBuf::from(&asset),
        };
        let texture_source = crate::texture_assets::texture_source_for_file(&path);
        let image = crate::texture_assets::decode_image_file(&path)
            .map_err(|error| format!("texture '{asset}': {error}"))?;
        let id = AssetId::of(crate::texture_assets::TEXTURE_ASSET_KIND, &texture_source);
        texture_sources.push(texture_source);
        textures.push((asset, id, Arc::new(image)));
    }
    remember_texture_users(source, &texture_sources);
    // Compile against placeholder slots while the SceneDB asset loader is on
    // a worker. Graph-local ordinals produce the same slot-independent source
    // later used by the renderer, so mirror attachment never runs PSGC.
    let placeholders = textures
        .iter()
        .enumerate()
        .map(|(slot, (asset, _, _))| (asset.clone(), slot as u32))
        .collect();
    let snippet = crate::material_graph::compile_material_graph(&graph, &placeholders)?;
    Ok(GraphMaterial {
        reads_time: snippet.contains("radiant_graph_time()"),
        graph,
        textures,
        compiled_snippet: snippet,
    })
}

/// The material sources whose graphs sample each texture source, as their
/// last loads found them.
fn texture_users() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    static USERS: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    USERS.get_or_init(Default::default)
}

const MAX_TEXTURE_USER_SOURCES: usize = 8192;
const MAX_MATERIALS_PER_TEXTURE_SOURCE: usize = 1024;

fn remember_texture_users(material_source: &str, texture_sources: &[String]) {
    if let Ok(mut users) = texture_users().lock() {
        users.retain(|_, materials| {
            materials.remove(material_source);
            !materials.is_empty()
        });
        for texture_source in texture_sources {
            let materials = users
                .entry(texture_source.clone())
                .or_default();
            materials.insert(material_source.to_string());
            while materials.len() > MAX_MATERIALS_PER_TEXTURE_SOURCE {
                if let Some(victim) = materials.iter().next().cloned() {
                    materials.remove(&victim);
                } else {
                    break;
                }
            }
        }
        while users.len() > MAX_TEXTURE_USER_SOURCES {
            let victim = users
                .keys()
                .find(|source| !texture_sources.iter().any(|current| current == *source))
                .cloned();
            if let Some(victim) = victim {
                users.remove(&victim);
            } else {
                break;
            }
        }
    }
}

/// The material sources a change to the file at `path` (absolute) reloads:
/// the material it is (a surface document, or a graph save, which is its
/// folder's material), and every loaded graph that samples it as a texture.
pub fn material_sources_for_file(path: &Path) -> Vec<String> {
    let mut sources = Vec::new();
    let file = path.to_string_lossy();
    if path
        .file_name()
        .is_some_and(|name| name == SHADER_GRAPH_FILE)
    {
        if let Some(folder) = path.parent().and_then(|folder| folder.to_str()) {
            sources.extend(material_source(folder));
        }
    }
    sources.extend(material_source(&file));
    let texture_source = crate::texture_assets::texture_source_for_file(path);
    if let Ok(users) = texture_users().lock() {
        sources.extend(users.get(&texture_source).into_iter().flatten().cloned());
    }
    sources.sort();
    sources.dedup();
    sources
}

/// The material a slot draws with: an explicit surface override, the bound
/// material (`bound`, the `version` of asset `id`), or the mesh's imported
/// surface while it has none (unloaded, failed, or nothing assigned). A
/// graph is compiled against the slots its textures take in `mirror`'s
/// texture store; without a mirror it draws the imported surface.
pub fn resolve_material(
    slot: Option<&StaticMeshMaterialSlot>,
    bound: Option<(AssetId, u64, &LoadedMaterial)>,
    mirror: Option<&pulsar_scenedb::gpu::GpuMirrorHandle>,
) -> crate::components::ResolvedSlotMaterial {
    use crate::components::ResolvedSlotMaterial;
    let imported = || ResolvedSlotMaterial {
        surface: slot.map_or_else(Default::default, |slot| {
            slot.surface_override.unwrap_or(slot.imported_surface)
        }),
        material_class: helio_mats::MATERIAL_CLASS_DEFAULT,
        graph_hash: 0,
        graph_texture_slots: [helio::GpuMaterial::NO_TEXTURE; 5],
    };
    let Some(slot) = slot else {
        return imported();
    };
    if let Some(surface) = slot.surface_override {
        return ResolvedSlotMaterial {
            surface,
            material_class: helio_mats::MATERIAL_CLASS_DEFAULT,
            graph_hash: 0,
            graph_texture_slots: [helio::GpuMaterial::NO_TEXTURE; 5],
        };
    }
    match bound {
        Some((_, _, LoadedMaterial::Surface(surface))) => ResolvedSlotMaterial {
            surface: *surface,
            material_class: helio_mats::MATERIAL_CLASS_DEFAULT,
            graph_hash: 0,
            graph_texture_slots: [helio::GpuMaterial::NO_TEXTURE; 5],
        },
        Some((id, version, LoadedMaterial::Graph(graph))) => {
            let Some(mirror) = mirror else {
                return imported();
            };
            match compiled_graph(id, version, graph, mirror) {
                Ok((hash, graph_texture_slots)) => ResolvedSlotMaterial {
                    surface: slot.imported_surface,
                    material_class: helio_mats::MATERIAL_CLASS_CUSTOM,
                    graph_hash: hash,
                    graph_texture_slots,
                },
                Err(error) => {
                    tracing::warn!(%error, "could not compile a material graph; using the imported material");
                    imported()
                }
            }
        }
        None => imported(),
    }
}

/// Compiled graph sources (hash and WGSL), by material and content version.
/// Texture residency belongs to each material row, not the shader identity.
type CompiledGraphs = HashMap<(AssetId, u64), Result<CompiledGraph, String>>;
const MAX_COMPILED_GRAPHS: usize = 1024;

fn compiled_graphs() -> &'static Mutex<CompiledGraphs> {
    static COMPILED: OnceLock<Mutex<CompiledGraphs>> = OnceLock::new();
    COMPILED.get_or_init(Default::default)
}

/// Register `graph`'s textures in `mirror`'s store, register its worker-compiled
/// slot-independent source, and return the scene-local slots that the material
/// row binds. Register the source on every use because the renderer's graph
/// registry is bounded.
fn compiled_graph(
    id: AssetId,
    version: u64,
    graph: &GraphMaterial,
    mirror: &pulsar_scenedb::gpu::GpuMirrorHandle,
) -> Result<(u64, [u32; 5]), String> {
    let assets = crate::material_graph::texture_assets(&graph.graph)?;
    if assets.len() > crate::material_graph::GRAPH_TEXTURE_FIELDS.len() {
        return Err(format!(
            "material graphs support at most {} sampled textures",
            crate::material_graph::GRAPH_TEXTURE_FIELDS.len()
        ));
    }
    let bindings: HashMap<_, _> = assets
        .iter()
        .enumerate()
        .map(|(ordinal, asset)| (asset.clone(), ordinal as u32))
        .collect();
    let mut slots = [helio::GpuMaterial::NO_TEXTURE; 5];
    // What each image is for decides its format: a colour is stored as sRGB and
    // a roughness or normal map is not.
    let uses = crate::material_graph::texture_uses(&graph.graph);
    let use_of = |asset: &str| {
        uses.get(asset)
            .copied()
            .unwrap_or(crate::cooked_texture::TextureUse::Color)
    };
    // A material with several images the store does not hold yet cooks them
    // side by side on the loader threads, not one after another here.
    crate::texture_assets::prefetch_missing(
        graph
            .textures
            .iter()
            .map(|(asset, texture, image)| (*texture, use_of(asset), &**image)),
        version,
        mirror,
    );
    for (asset, texture, image) in &graph.textures {
        // A texture's GPU copy follows the material's version: the material
        // reloads when one of its textures changes.
        let slot =
            crate::texture_assets::resident_slot(*texture, version, image, use_of(asset), mirror)
                .map_err(|error| format!("texture '{asset}': {error}"))?;
        let ordinal = *bindings
            .get(asset)
            .ok_or_else(|| format!("texture '{asset}' is not sampled by the graph"))?
            as usize;
        slots[ordinal] = slot;
    }
    let mut compiled = compiled_graphs()
        .lock()
        .map_err(|_| "compiled graph cache poisoned".to_string())?;
    compiled.retain(|(cached_id, cached_version), _| {
        *cached_id != id || *cached_version == version
    });
    if compiled.len() >= MAX_COMPILED_GRAPHS && !compiled.contains_key(&(id, version)) {
        if let Some(victim) = compiled.keys().next().copied() {
            compiled.remove(&victim);
        }
    }
    let result = compiled.entry((id, version)).or_insert_with(|| {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        graph.compiled_snippet.hash(&mut hasher);
        Ok(CompiledGraph {
            hash: hasher.finish().max(1),
            snippet: graph.compiled_snippet.clone(),
        })
    });
    let compiled = result.as_ref().map_err(Clone::clone)?;
    helio_mats::register_graph_source(compiled.hash, compiled.snippet.clone());
    Ok((compiled.hash, slots))
}
