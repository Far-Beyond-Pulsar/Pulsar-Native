use crate::components::FileManagerDrawer;

impl FileManagerDrawer {
    fn is_mesh_thumbable_ext(ext: &str) -> bool {
        matches!(ext, "fbx" | "gltf" | "glb" | "obj" | "usd" | "usda")
    }

    fn is_thumbable_ext(ext: &str) -> bool {
        matches!(
            ext,
            "fbx"
                | "gltf"
                | "glb"
                | "obj"
                | "usd"
                | "usda"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "tga"
                | "bmp"
                | "gif"
        )
    }

    pub(crate) fn ensure_thumbnail(
        &mut self,
        path: &std::path::Path,
        cx: &mut gpui::Context<Self>,
    ) {
        if path.is_dir() {
            return;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if (!Self::is_thumbable_ext(&ext) && !Self::is_mesh_thumbable_ext(&ext))
            || self.thumbnails.contains_key(path)
        {
            return;
        }
        self.thumbnails.insert(path.to_path_buf(), None);
        let abs = path.to_path_buf();
        let root = self.thumbnail_cache_root.clone();
        let (tx, rx) = smol::channel::bounded::<Option<std::sync::Arc<image::RgbaImage>>>(1);
        if Self::is_mesh_thumbable_ext(&ext) {
            // The shared image-thumbnail worker intentionally handles image
            // decoding only. Meshes need a real Helio render so primitives and
            // imported models show their geometry instead of a generic icon.
            let mesh_path = abs.clone();
            std::thread::Builder::new()
                .name("mesh-thumbnail".into())
                .spawn(move || {
                    let cache_file = mesh_thumbnail_cache_path(&mesh_path, &root);
                    let rgba = cache_file
                        .as_ref()
                        .and_then(|cache_file| {
                            image::open(cache_file)
                                .ok()
                                .map(|image| std::sync::Arc::new(image.into_rgba8()))
                        })
                        .or_else(|| {
                            let result = helio_snapshot::render_snapshot(
                                &mesh_path,
                                helio_snapshot::SnapshotConfig {
                                    width: 128,
                                    height: 128,
                                    fit_margin: 1.12,
                                    ..Default::default()
                                },
                            );
                            match result {
                                Ok(image) => {
                                    if let Some(cache_file) = cache_file {
                                        if let Some(parent) = cache_file.parent() {
                                            let _ = std::fs::create_dir_all(parent);
                                        }
                                        let _ = image.save_with_format(
                                            &cache_file,
                                            ::image::ImageFormat::Png,
                                        );
                                    }
                                    Some(std::sync::Arc::new(image))
                                }
                                Err(error) => {
                                    tracing::debug!(
                                        "mesh thumbnail render failed for {:?}: {error}",
                                        mesh_path
                                    );
                                    None
                                }
                            }
                        });
                    let _ = smol::block_on(tx.send(rgba));
                })
                .expect("failed to spawn mesh-thumbnail worker");
        } else {
            engine_fs::thumbnails::service().request(abs.clone(), root, move |rgba| {
                smol::block_on(tx.send(rgba));
            });
        }
        cx.spawn(async move |this, cx| {
            let Ok(maybe) = rx.recv().await else {
                return;
            };
            let img = maybe.map(|rgba| {
                std::sync::Arc::new(gpui::RenderImage::new(smallvec::smallvec![
                    image::Frame::new((*rgba).clone().into())
                ]))
            });
            let _ = cx.update(|cx| {
                this.update(cx, |d, cx| {
                    d.thumbnails.insert(abs, img);
                    cx.notify();
                })
            });
        })
        .detach();
    }

    pub fn set_thumbnail_cache_root(&mut self, root: std::path::PathBuf) {
        self.thumbnail_cache_root = root;
    }
}

fn mesh_thumbnail_cache_path(
    asset_path: &std::path::Path,
    cache_root: &std::path::Path,
) -> Option<std::path::PathBuf> {
    use std::hash::{Hash, Hasher};

    let metadata = std::fs::metadata(asset_path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    asset_path.to_string_lossy().hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    modified.hash(&mut hasher);
    Some(
        cache_root
            .join(".pulsar")
            .join("thumbnails")
            .join(format!("mesh-{:016x}.png", hasher.finish())),
    )
}
