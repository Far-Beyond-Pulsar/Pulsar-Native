//! Project-backed asset source for animation graph and montage compilation.
//!
//! Assets are addressed by stable id: the project file index maps `(kind, id)`
//! to a file, the file is read and parsed with the asset's own codec. Nothing is
//! cached across compilations here; the compiled graph is the cache. The derived
//! Pose Search index is stored under `.pulsar/cache/posedb/`, keyed by the
//! database's settings and its source clips, and never written beside a source.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pulsar_reflection::{
    AnimationGraphAssetKind, AnimationMontageAssetKind, AnimationSequenceAssetKind, AssetKind, AssetRef, BlendSpaceAssetKind,
    IkRetargeterAssetKind, IkRigAssetKind, PoseDatabaseAssetKind, RetargetProfileAssetKind, SkeletonAssetKind,
};
use skeletal_assets::anim_graph::AnimationGraphAsset;
use skeletal_assets::assets::{AnimSeqAsset, SkeletonAsset};
use skeletal_assets::blend_space::BlendSpaceAsset;
use skeletal_assets::ik_retarget::IkRetargetAsset;
use skeletal_assets::ik_rig::IkRigAsset;
use skeletal_assets::montage::AnimationMontageAsset;
use skeletal_assets::pose_db::{check_index, derived_index_path, encode_index, IndexKey, IndexState, PoseDbAsset, INDEX_BUILDER_VERSION};
use skeletal_assets::retarget::RetargetProfileAsset;
use skeletal_assets::runtime::graph::compile::AssetSource;

pub struct ProjectAnimSource {
    root: PathBuf,
    cache: RefCell<HashMap<(&'static str, String), Option<Arc<dyn std::any::Any + Send + Sync>>>>,
}

impl ProjectAnimSource {
    /// A source for the open project, or `None` when no project is open.
    pub fn open() -> Option<Self> {
        let root = PathBuf::from(engine_state::get_project_path()?);
        Some(Self::at(root))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root, cache: RefCell::new(HashMap::new()) }
    }

    fn path(&self, kind: &'static str, id: &str) -> Option<PathBuf> {
        let index = engine_fs::open_project_file_index(&self.root).ok().flatten()?;
        let relative = index.path_for_asset_id_kind(kind, id).ok().flatten()?;
        Some(self.root.join(relative))
    }

    fn text(&self, kind: &'static str, id: &str) -> Option<String> {
        let Some(path) = self.path(kind, id) else {
            tracing::warn!(kind, id, "animation asset id is not indexed in this project");
            return None;
        };
        match skeletal_assets::assets::read_text(&path) {
            Ok(t) => Some(t),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "could not read animation asset");
                None
            }
        }
    }

    fn load<T: 'static + Send + Sync>(&self, kind: &'static str, id: &str, parse: impl FnOnce(&str) -> Result<T, String>) -> Option<Arc<T>> {
        let key = (kind, id.to_string());
        if let Some(hit) = self.cache.borrow().get(&key) {
            return hit.clone().and_then(|a| a.downcast::<T>().ok());
        }
        let loaded = self.text(kind, id).and_then(|text| match parse(&text) {
            Ok(v) => Some(Arc::new(v)),
            Err(error) => {
                tracing::warn!(kind, id, %error, "could not parse animation asset");
                None
            }
        });
        self.cache.borrow_mut().insert(key, loaded.clone().map(|a| a as Arc<dyn std::any::Any + Send + Sync>));
        loaded
    }
}

impl AssetSource for ProjectAnimSource {
    fn skeleton(&self, id: &str) -> Option<Arc<SkeletonAsset>> {
        self.load(SkeletonAssetKind::KIND, id, SkeletonAsset::parse)
    }
    fn sequence(&self, id: &str) -> Option<Arc<AnimSeqAsset>> {
        self.load(AnimationSequenceAssetKind::KIND, id, AnimSeqAsset::parse)
    }
    fn blend_space(&self, id: &str) -> Option<Arc<BlendSpaceAsset>> {
        self.load(BlendSpaceAssetKind::KIND, id, |t| BlendSpaceAsset::parse(t).map_err(|e| e.to_string()))
    }
    fn montage(&self, id: &str) -> Option<Arc<AnimationMontageAsset>> {
        self.load(AnimationMontageAssetKind::KIND, id, |t| AnimationMontageAsset::parse(t).map_err(|e| e.to_string()))
    }
    fn graph(&self, id: &str) -> Option<Arc<AnimationGraphAsset>> {
        self.load(AnimationGraphAssetKind::KIND, id, |t| AnimationGraphAsset::parse(t).map_err(|e| e.to_string()))
    }
    fn ik_rig(&self, id: &str) -> Option<Arc<IkRigAsset>> {
        self.load(IkRigAssetKind::KIND, id, |t| IkRigAsset::parse(t).map_err(|e| e.to_string()))
    }
    fn retarget_profile(&self, id: &str) -> Option<Arc<RetargetProfileAsset>> {
        self.load(RetargetProfileAssetKind::KIND, id, |t| RetargetProfileAsset::parse(t).map_err(|e| e.to_string()))
    }
    fn ik_retargeter(&self, id: &str) -> Option<Arc<IkRetargetAsset>> {
        self.load(IkRetargeterAssetKind::KIND, id, |t| IkRetargetAsset::parse(t).map_err(|e| e.to_string()))
    }
    fn pose_db(&self, id: &str) -> Option<Arc<PoseDbAsset>> {
        self.load(PoseDatabaseAssetKind::KIND, id, |t| PoseDbAsset::parse(t).map_err(|e| e.to_string()))
    }

    fn cached_pose_index(&self, database: &str, key: &str) -> Option<Vec<u8>> {
        let id: AssetRef<PoseDatabaseAssetKind> = AssetRef::parse(database)?;
        let want = IndexKey { builder_version: INDEX_BUILDER_VERSION, hash: key.to_string() };
        let bytes = std::fs::read(derived_index_path(&self.root, &id, &want)).ok()?;
        match check_index(&bytes, &want) {
            IndexState::Current(payload) => Some(payload),
            IndexState::Stale { .. } | IndexState::Corrupt => None,
        }
    }

    fn store_pose_index(&self, database: &str, key: &str, payload: &[u8]) {
        let Some(id) = AssetRef::<PoseDatabaseAssetKind>::parse(database) else { return };
        let k = IndexKey { builder_version: INDEX_BUILDER_VERSION, hash: key.to_string() };
        let path = derived_index_path(&self.root, &id, &k);
        let write = || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // write beside the target and rename: a reader never sees half an index
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, encode_index(&k, payload))?;
            std::fs::rename(&tmp, &path)
        };
        if let Err(error) = write() {
            tracing::warn!(%error, path = %path.display(), "could not store the derived pose index");
        }
    }
}

/// The project file an animation graph reference resolves to.
pub fn graph_path(root: &Path, id: &str) -> Option<PathBuf> {
    ProjectAnimSource::at(root.to_path_buf()).path(AnimationGraphAssetKind::KIND, id)
}
