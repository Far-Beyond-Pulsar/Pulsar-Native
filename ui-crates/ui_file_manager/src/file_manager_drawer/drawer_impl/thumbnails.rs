impl FileManagerDrawer {
    /// Supported extensions — mirrors engine_fs::thumbnails.
    fn is_thumbable_ext(ext: &str) -> bool {
        matches!(
            ext,
            "fbx" | "gltf" | "glb" | "obj" | "usd" | "usda"
                | "png" | "jpg" | "jpeg" | "webp" | "tga" | "bmp" | "gif"
        )
    }

    /// Queue a thumbnail request for `path` unless one is already in-flight or
    /// the result (success or "unsupported") has been cached.
    /// No-op for folders or unsupported file types.
    pub(super) fn ensure_thumbnail(&mut self, path: &std::path::Path, cx: &mut gpui::Context<Self>) {
        if path.is_dir() {
            return;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();

        if !Self::is_thumbable_ext(&ext) {
            return;
        }

        // Any entry in the map (Some or None) means we've already handled this path.
        if self.thumbnails.contains_key(path) {
            return;
        }

        // Mark as in-flight so we don't re-queue.
        self.thumbnails.insert(path.to_path_buf(), None);

        let abs_path = path.to_path_buf();
        let cache_root = self.thumbnail_cache_root.clone();
        let (tx, rx) = smol::channel::bounded::<Option<std::sync::Arc<image::RgbaImage>>>(1);

        engine_fs::thumbnails::service().request(abs_path.clone(), cache_root, move |rgba| {
            smol::block_on(tx.send(rgba)).ok();
        });

        cx.spawn(async move |this, cx| {
            let Ok(maybe_rgba) = rx.recv().await else {
                return;
            };

            let render_image = maybe_rgba.map(|rgba| {
                std::sync::Arc::new(gpui::RenderImage::new(
                    smallvec::smallvec![image::Frame::new((*rgba).clone().into())],
                ))
            });

            cx.update(|cx| {
                this.update(cx, |drawer, cx| {
                    // Insert Some(Some(img)) for success, Some(None) for unsupported —
                    // either way the key exists so we won't retry.
                    drawer.thumbnails.insert(abs_path, render_image);
                    cx.notify();
                })
                .ok();
            })
            .ok();
        })
        .detach();
    }

    /// Update the cache root when the project changes.
    pub fn set_thumbnail_cache_root(&mut self, root: std::path::PathBuf) {
        self.thumbnail_cache_root = root;
    }
}
