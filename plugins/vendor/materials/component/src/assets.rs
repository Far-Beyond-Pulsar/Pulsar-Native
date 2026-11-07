//! Material assets (SceneDB#84): a scalar surface document
//! ([`SurfaceMaterialDocument`] JSON) or a shader-graph folder
//! ([`SHADER_GRAPH_FILE`], with the textures it samples).
//!
//! The plugin owns the kind ([`MATERIAL_ASSET_KIND`]): its loader runs on a
//! SceneDB worker thread, reads the document or compiles the graph (decoding
//! its textures there too) and produces a [`MaterialPayload`]; whoever draws
//! with materials binds that payload and asks it for rows
//! ([`MaterialAsset::resolve`]), without linking this crate. A graph takes
//! slots in a scene's texture store when a draw in that scene resolves it.
//!
//! A material's source is its path as authored, normalized
//! (`helio_component::asset_source`): the loader resolves it against the
//! project, the working directory and the engine's assets.
//! [`material_sources_for_file`] maps a changed file to the materials it is
//! (a surface document, a graph save, or a texture a loaded graph samples),
//! for the engine's asset reloads.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use helio_component::asset_component::{
    AssetLoaderRegistration, AssetReloadRegistration, asset_source,
};
use helio_component::texture_assets::LoadedTexture;
use helio_mats::asset::{
    MATERIAL_ASSET_KIND, MaterialAsset, MaterialPayload, ResolvedMaterial, SHADER_GRAPH_FILE,
    SurfaceMaterialDocument, SurfaceValues,
};
use pulsar_scenedb::{AssetId, AssetPayload};

/// A loaded material.
pub enum LoadedMaterial {
    /// A scalar surface document.
    Surface(SurfaceValues),
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

impl MaterialAsset for LoadedMaterial {
    fn resolve(
        &self,
        id: AssetId,
        version: u64,
        mirror: Option<&pulsar_scenedb::gpu::GpuMirrorHandle>,
        fallback: &SurfaceValues,
    ) -> Result<ResolvedMaterial, String> {
        match self {
            Self::Surface(surface) => Ok(ResolvedMaterial::surface(surface)),
            Self::Graph(graph) => {
                let mirror = mirror.ok_or("a material graph draws only in a scene")?;
                let (hash, texture_slots) = compiled_graph(id, version, graph, mirror)?;
                Ok(ResolvedMaterial {
                    row: fallback.row(texture_slots),
                    material_class: helio_mats::MATERIAL_CLASS_CUSTOM,
                    graph_hash: hash,
                })
            }
        }
    }

    fn reads_time(&self) -> bool {
        matches!(self, Self::Graph(graph) if graph.reads_time)
    }

    fn held_bytes(&self) -> usize {
        match self {
            Self::Graph(graph) => graph.textures.len() * 64,
            Self::Surface(_) => 0,
        }
    }

    fn warm(&self) {
        // A graph's textures the residency cache dropped start coming back.
        if let Self::Graph(graph) = self {
            for (_, _, image) in &graph.textures {
                image.warm();
            }
        }
    }
}

/// Bytes a loaded material holds outside SceneDB's residency cache: a
/// graph's texture handles (the images are budgeted by the cache, see
/// `LoadedTexture`).
fn payload_size(payload: &AssetPayload) -> usize {
    payload
        .downcast_ref::<MaterialPayload>()
        .map_or(0, |material| material.0.held_bytes())
}

inventory::submit! {
    AssetLoaderRegistration {
        origin: pulsar_plugins::origin!(),
        kind: MATERIAL_ASSET_KIND,
        load: load_material_asset,
        size: Some(payload_size),
    }
}

inventory::submit! {
    AssetReloadRegistration {
        origin: pulsar_plugins::origin!(),
        kind: plugin_editor_api::AssetKind::Material,
        sources_for_file: material_sources_for_file,
    }
}

// A texture reloads every loaded graph that samples it.
inventory::submit! {
    AssetReloadRegistration {
        origin: pulsar_plugins::origin!(),
        kind: plugin_editor_api::AssetKind::Texture,
        sources_for_file: graph_sources_sampling,
    }
}

/// The file the material at `source` is read from: a graph folder's save,
/// or the document itself.
fn resolve_material_file(source: &str) -> PathBuf {
    let path = match engine_state::get_project_path() {
        Some(root) => helio_component::subsystems::resolve_asset_path(Path::new(&root), source),
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
    Ok(Arc::new(MaterialPayload(Arc::new(load_material(source)?))))
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
    let material: SurfaceMaterialDocument = serde_json::from_slice(&bytes)
        .map_err(|error| format!("'{}' is not a surface material: {error}", file.display()))?;
    if material.version != SurfaceMaterialDocument::VERSION {
        return Err(format!(
            "'{}' has unsupported version {}",
            file.display(),
            material.version
        ));
    }
    Ok(LoadedMaterial::Surface(material.values()))
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
    let graph_textures = crate::graph::texture_assets(&graph)?;
    let mut textures = Vec::new();
    let mut texture_sources = Vec::with_capacity(graph_textures.len());
    for asset in graph_textures {
        let path = crate::graph::texture_path(&asset, root.as_deref());
        let texture_source = helio_component::texture_assets::texture_source_for_file(&path);
        let image = helio_component::texture_assets::decode_image_file(&path)
            .map_err(|error| format!("texture '{asset}': {error}"))?;
        let id = AssetId::of(
            helio_component::texture_assets::TEXTURE_ASSET_KIND,
            &texture_source,
        );
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
    let snippet = crate::graph::compile_material_graph(&graph, &placeholders)?;
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
            let materials = users.entry(texture_source.clone()).or_default();
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

/// The materials a change to the file at `path` (absolute) reloads: the
/// material it is (a surface document, or a graph save, which is its
/// folder's material), and every loaded graph that samples it as a texture.
pub fn material_sources_for_file(path: &Path) -> Vec<String> {
    let mut sources = Vec::new();
    let file = path.to_string_lossy();
    if path
        .file_name()
        .is_some_and(|name| name == SHADER_GRAPH_FILE)
    {
        if let Some(folder) = path.parent().and_then(|folder| folder.to_str()) {
            sources.extend(asset_source(folder));
        }
    }
    sources.extend(asset_source(&file));
    sources.extend(graph_sources_sampling(path));
    sources.sort();
    sources.dedup();
    sources
}

/// Every loaded graph material that samples the image at `path` (absolute),
/// as its last load found it.
pub fn graph_sources_sampling(path: &Path) -> Vec<String> {
    let texture_source = helio_component::texture_assets::texture_source_for_file(path);
    texture_users()
        .lock()
        .map(|users| {
            users
                .get(&texture_source)
                .into_iter()
                .flatten()
                .cloned()
                .collect()
        })
        .unwrap_or_default()
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
    let assets = crate::graph::texture_assets(&graph.graph)?;
    if assets.len() > crate::graph::GRAPH_TEXTURE_FIELDS.len() {
        return Err(format!(
            "material graphs support at most {} sampled textures",
            crate::graph::GRAPH_TEXTURE_FIELDS.len()
        ));
    }
    let bindings: HashMap<_, _> = assets
        .iter()
        .enumerate()
        .map(|(ordinal, asset)| (asset.clone(), ordinal as u32))
        .collect();
    let mut slots = [helio_mats::GpuMaterial::NO_TEXTURE; 5];
    // What each image is for decides its format: a colour is stored as sRGB and
    // a roughness or normal map is not.
    let uses = crate::graph::texture_uses(&graph.graph);
    let use_of = |asset: &str| {
        uses.get(asset)
            .copied()
            .unwrap_or(helio_component::cooked_texture::TextureUse::Color)
    };
    // A material with several images the store does not hold yet cooks them
    // side by side on the loader threads, not one after another here.
    helio_component::texture_assets::prefetch_missing(
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
        let slot = helio_component::texture_assets::resident_slot(
            *texture,
            version,
            image,
            use_of(asset),
            mirror,
        )
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
    compiled
        .retain(|(cached_id, cached_version), _| *cached_id != id || *cached_version == version);
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
