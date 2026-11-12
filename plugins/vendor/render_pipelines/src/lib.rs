//! Render Pipelines (`pulsar.render_pipelines`): the render pipelines the
//! engine ships, as `.rendergraph` assets in the plugin's folder
//! (`plugins/pulsar.render_pipelines/`).
//!
//! Helio registers no pipeline and has no default: it knows the frame (the
//! slots, draw phases and lighting model its presets are made of) and turns a
//! preset into a graph asset (`helio_presets::graphs::graph_asset`). This
//! plugin decides which pipelines ship and serves each as a file:
//!
//! | File | Preset |
//! |---|---|
//! | `Deferred.rendergraph` | the deferred model; TSR when the project enables it, FXAA otherwise |
//! | `DeferredFxaa.rendergraph` | the deferred model with FXAA |
//! | `Forward.rendergraph` | the forward model, no G-buffer |
//! | `<label>.rendergraph` | each pipeline a registered lighting model brings (the lights' HLFS) |
//!
//! **The files are made for the enabled plugins.** A kind's passes join a
//! preset where its feature placed them, so each file lists the passes the
//! project's plugins have and nothing else; a project that disables a kind
//! gets files without its nodes rather than graphs that no longer validate.
//! The folder is made again when the plugin set changes
//! (`pulsar_plugins::generation`), and a changed file is reported through the
//! folder's capability, so the engine registers it again and the renderers
//! running it rebuild. Anti-aliasing is not baked in: `Deferred` names the
//! anti-aliasing node, which follows the project's TSR setting when the graph
//! builds, so the file does not depend on project settings.
//!
//! The files are the chain form (every node on one execution chain, no data
//! wires), which builds exactly what the preset builds; the editor's wired
//! form needs a GPU probe and is not deterministic before it ran. A packaged
//! game ships the packager's snapshot of the folder.
//!
//! **Old ids.** Levels saved before the pipelines were assets name Helio's
//! old built-in ids (`helio.deferred`, `helio.fxaa`, the lighting models'
//! `helio.hlfs`, ...); the plugin's [`PipelineIdMigration`] rewrites them to
//! the files' paths when a level loads.

pulsar_plugins::installed!("pulsar-plugin.toml", build);

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use engine_backend::subsystems::render::helio_renderer::pipelines::{
    PipelineIdMigration, RENDER_GRAPH_EXTENSION,
};
use engine_backend::subsystems::render::helio_renderer::render_features;
use engine_fs::{FolderSource, FsEntry, FsMetadata, ManifestEntry, MemoryFolder, PluginFolder};
use helio_graph_asset::NodeRegistry;
use helio_presets::Preset;
use parking_lot::Mutex;

/// The plugin's id, its folder's name.
pub const PLUGIN_ID: &str = "pulsar.render_pipelines";

/// One pipeline the plugin ships: its file's stem, what it is made from and
/// the old built-in ids it replaces.
pub struct Shipped {
    pub stem: String,
    pub preset: Preset,
    pub description: String,
    pub replaces: Vec<String>,
}

/// The pipelines the plugin ships for the nodes `registry` holds: Helio's
/// presets, then each pipeline a registered lighting model brings.
pub fn shipped(registry: &NodeRegistry) -> Vec<Shipped> {
    let mut list = vec![
        Shipped {
            stem: "Deferred".into(),
            preset: Preset::Deferred,
            description: "Full deferred pipeline: shadows, GI, sky, fog, post-processing; TSR when the project enables it, FXAA otherwise.".into(),
            // `helio.deferred` was the engine's default; `helio.deferred_graph`
            // was the same nodes as data.
            replaces: vec!["helio.deferred".into(), "helio.deferred_graph".into()],
        },
        Shipped {
            stem: "DeferredFxaa".into(),
            preset: Preset::DeferredFxaa,
            description: "Deferred lighting with FXAA instead of temporal resolve.".into(),
            replaces: vec!["helio.fxaa".into()],
        },
        Shipped {
            stem: "Forward".into(),
            preset: Preset::Forward,
            description: "Forward-lit opaque geometry, no G-buffer.".into(),
            replaces: vec!["helio.forward_opaque".into(), "helio.forward_only".into()],
        },
    ];
    for model in helio_frame_api::lighting::registered_models(registry) {
        for pipeline in &model.pipelines {
            list.push(Shipped {
                stem: file_stem(pipeline.label),
                preset: Preset::Model(*pipeline),
                description: pipeline.description.to_string(),
                replaces: vec![pipeline.id.to_string()],
            });
        }
    }
    list
}

/// A file stem from a label: letters and digits kept, every other run made
/// one `_` (`HLFS + FXAA` is `HLFS_FXAA`).
fn file_stem(label: &str) -> String {
    let mut stem = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            stem.push(c);
        } else if !stem.ends_with('_') {
            stem.push('_');
        }
    }
    stem.trim_matches('_').to_owned()
}

/// The file name of `stem` in the folder.
fn file_name(stem: &str) -> String {
    format!("{stem}.{RENDER_GRAPH_EXTENSION}")
}

/// The project-relative path (the pipeline id a level stores) of the file
/// `stem`.
pub fn asset_path(stem: &str) -> String {
    format!(
        "{}/{PLUGIN_ID}/{}",
        engine_fs::virtual_fs::PLUGINS_FOLDER,
        file_name(stem)
    )
}

/// The node registry the files are made from: Helio's nodes and every enabled
/// plugin's, with no probed pins (so the files are the same whenever they are
/// made).
fn node_registry() -> NodeRegistry {
    helio_presets::graphs::registry_with(&render_features::plugin_features())
}

/// Every file of the folder for `registry`, by name.
pub fn files(registry: &NodeRegistry) -> BTreeMap<String, Vec<u8>> {
    shipped(registry)
        .into_iter()
        .map(|shipped| {
            let mut asset = helio_presets::graphs::graph_asset(shipped.preset, registry);
            asset.name = shipped.stem.clone();
            asset.description = shipped.description;
            (
                file_name(&shipped.stem),
                (asset.to_json() + "\n").into_bytes(),
            )
        })
        .collect()
}

/// The plugin's folder: the files for the enabled plugins, made again when
/// the plugin set changes.
struct Pipelines {
    files: MemoryFolder,
    /// The plugin generation the files were made for.
    made_for: Mutex<Option<u64>>,
}

impl Pipelines {
    /// Makes the files again if the plugin set changed since they were made;
    /// [`MemoryFolder`] reports each file that changed.
    fn refresh(&self) {
        let generation = pulsar_plugins::generation();
        let mut made_for = self.made_for.lock();
        if *made_for == Some(generation) {
            return;
        }
        *made_for = Some(generation);
        let fresh = files(&node_registry());
        let root = Path::new("");
        let stale: Vec<String> = self
            .files
            .list_dir(root, "")
            .map(|entries| entries.into_iter().map(|entry| entry.name).collect())
            .unwrap_or_default();
        drop(made_for);
        for name in stale {
            if !fresh.contains_key(&name) {
                self.files.delete(&name);
            }
        }
        for (name, bytes) in fresh {
            if self.files.read_file(root, &name).ok().as_deref() != Some(bytes.as_slice()) {
                self.files.write(&name, bytes);
            }
        }
    }
}

impl FolderSource for Pipelines {
    fn read_file(&self, project: &Path, path: &str) -> anyhow::Result<Vec<u8>> {
        self.refresh();
        self.files.read_file(project, path)
    }

    fn list_dir(&self, project: &Path, path: &str) -> anyhow::Result<Vec<FsEntry>> {
        self.refresh();
        self.files.list_dir(project, path)
    }

    fn metadata(&self, project: &Path, path: &str) -> Option<FsMetadata> {
        self.refresh();
        self.files.metadata(project, path)
    }

    fn manifest(&self, project: &Path, path: &str) -> Vec<ManifestEntry> {
        self.refresh();
        self.files.manifest(project, path)
    }

    fn project_independent(&self) -> bool {
        true
    }
}

fn open(folder: PluginFolder) -> Arc<dyn FolderSource> {
    Arc::new(Pipelines {
        files: MemoryFolder::new(folder),
        made_for: Mutex::new(None),
    })
}

/// The file an old built-in pipeline id now is, for the enabled plugins.
fn migrate(id: &str) -> Option<String> {
    shipped(&node_registry())
        .into_iter()
        .find(|shipped| shipped.replaces.iter().any(|old| old == id))
        .map(|shipped| asset_path(&shipped.stem))
}

/// Everything the plugin registers.
pub fn build(p: &mut pulsar_plugins::PluginBuilder) {
    // The folder of shipped pipelines. A packaged game serves the packager's
    // snapshot of it (made for the project's plugins).
    p.add(engine_fs::FileProvider {
        origin: p.origin(),
        open,
        in_game: engine_fs::InGame::Snapshot,
    });
    // Levels that name an old built-in id run the file that replaced it.
    p.add(PipelineIdMigration {
        origin: p.origin(),
        migrate,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stems_are_file_safe() {
        assert_eq!(file_stem("HLFS + FXAA"), "HLFS_FXAA");
        assert_eq!(file_stem("HLFS"), "HLFS");
        assert_eq!(
            asset_path("Deferred"),
            "plugins/pulsar.render_pipelines/Deferred.rendergraph"
        );
    }
}
