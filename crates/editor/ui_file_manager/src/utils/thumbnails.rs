use crate::components::FileManagerDrawer;

impl FileManagerDrawer {
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
        if !Self::is_thumbable_ext(&ext) || self.thumbnails.contains_key(path) {
            return;
        }
        self.thumbnails.insert(path.to_path_buf(), None);
        let abs = path.to_path_buf();
        let root = self.thumbnail_cache_root.clone();
        let (tx, rx) = smol::channel::bounded::<Option<std::sync::Arc<image::RgbaImage>>>(1);
        engine_fs::thumbnails::service().request(abs.clone(), root, move |rgba| {
            smol::block_on(tx.send(rgba));
        });
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
