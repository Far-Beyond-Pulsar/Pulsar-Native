use gpui::{prelude::*, *};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ui::dropdown::{AssetSearchableList, AssetSearchableListEvent};

#[derive(Debug, Clone, Copy)]
pub struct AssetPickedEvent;

#[derive(Clone)]
struct AssetItem {
    /// Filename component shown as the primary title (e.g. "SM_Cube.fbx").
    display_name: String,
    /// Full relative asset path shown as the secondary description.
    path: String,
    /// Rendered thumbnail — `None` while loading or unavailable.
    thumbnail: Option<Arc<gpui::RenderImage>>,
}

#[derive(Clone, Debug)]
pub enum AssetQuery {
    Extension(String),
    FileType(String),
}

impl AssetQuery {
    pub fn extension(ext: impl Into<String>) -> Self {
        Self::Extension(ext.into())
    }

    pub fn file_type(id: impl Into<String>) -> Self {
        Self::FileType(id.into())
    }
}

pub struct MeshAssetPicker {
    searchable_list: Entity<AssetSearchableList<AssetItem>>,
    /// Source-of-truth item list — thumbnails are written here as they load.
    items: Vec<AssetItem>,
    /// Tracks which paths have already had a thumbnail task spawned so we don't double-spawn.
    thumbnail_requested: std::collections::HashSet<String>,
    /// Project root — used to resolve relative project asset paths.
    project_root: Option<PathBuf>,
    /// Engine assets root — used to resolve relative builtin paths (e.g. "meshes/primitives/SM_Cube.fbx").
    engine_assets_root: PathBuf,
    /// Root passed to engine_fs thumbnail cache. For project assets this is the
    /// project root; for engine builtins it falls back to cwd (alongside `assets/`).
    thumbnail_cache_root: PathBuf,
    selected_path: String,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for MeshAssetPicker {}
impl EventEmitter<AssetPickedEvent> for MeshAssetPicker {}

impl Focusable for MeshAssetPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.searchable_list.read(cx).focus_handle(cx)
    }
}

impl MeshAssetPicker {
    pub fn new(
        selected_path: impl Into<String>,
        builtins: Vec<String>,
        project_root: Option<PathBuf>,
        queries: Vec<AssetQuery>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let selected_path = selected_path.into();
        let mut assets = BTreeSet::new();

        for builtin in builtins {
            assets.insert(normalize_asset_path(&builtin));
        }

        if let Some(root) = project_root.as_ref() {
            for path in query_assets(root, &queries) {
                assets.insert(path);
            }
        }

        let items: Vec<AssetItem> = assets
            .into_iter()
            .map(|path| {
                let display_name = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                AssetItem {
                    display_name,
                    path,
                    thumbnail: None,
                }
            })
            .collect();

        // Engine assets root: current working directory + "assets" (where builtins live).
        let engine_assets_root = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("assets");

        // Thumbnail cache root: project root if available, else cwd.
        let thumbnail_cache_root = project_root.clone().unwrap_or_else(|| {
            engine_assets_root
                .parent()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        });

        let searchable_list = cx.new(|cx| {
            AssetSearchableList::new(
                window,
                cx,
                items.clone(),
                |item| item.display_name.clone(),
                |item| item.path.clone(),
            )
            .with_image_getter(|item| item.thumbnail.clone().map(ImageSource::Render))
            .with_empty_text("No matching assets")
            .with_max_width(px(360.0))
            .with_max_height(px(420.0))
        });

        let subscriptions = vec![cx.subscribe(
            &searchable_list,
            |this, _, event: &AssetSearchableListEvent<AssetItem>, cx| {
                if let AssetSearchableListEvent::Select(item) = event {
                    this.selected_path = item.path.clone();
                    // Pre-warm thumbnail for the newly selected asset so the
                    // trigger field updates immediately on next render.
                    let path = item.path.clone();
                    this.ensure_thumbnail(path, cx);
                    cx.emit(AssetPickedEvent);
                    cx.emit(DismissEvent);
                }
            },
        )];

        let mut picker = Self {
            searchable_list,
            items,
            thumbnail_requested: std::collections::HashSet::new(),
            project_root,
            engine_assets_root,
            thumbnail_cache_root,
            selected_path,
            _subscriptions: subscriptions,
        };

        // Pre-warm the thumbnail for the currently selected asset so the
        // trigger field shows the image without needing to open the dropdown.
        let sel = picker.selected_path.clone();
        if !sel.is_empty() {
            picker.ensure_thumbnail(sel, cx);
        }

        picker
    }

    pub fn selected_path(&self) -> &str {
        &self.selected_path
    }

    /// Point the picker at a path chosen elsewhere, without emitting
    /// [`AssetPickedEvent`].
    ///
    /// Used when the owning editor learns the value changed externally (a mesh
    /// dropped into the viewport, an undo) and needs the picker's highlight to
    /// follow along.
    pub fn set_selected_path(&mut self, path: impl Into<String>) {
        self.selected_path = path.into();
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Public accessors
    // ─────────────────────────────────────────────────────────────────────────

    /// Return the cached thumbnail for `path` if one has already been loaded.
    /// Returns `None` while still loading or for unsupported file types.
    pub fn thumbnail_for_path(&self, path: &str) -> Option<Arc<gpui::RenderImage>> {
        self.items
            .iter()
            .find(|item| item.path == path)
            .and_then(|item| item.thumbnail.clone())
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Thumbnail loading
    // ─────────────────────────────────────────────────────────────────────────

    /// Resolves a relative asset path to an absolute filesystem path.
    /// Tries: as-is (if already absolute) → project_root/path → engine_assets_root/path.
    fn resolve_path(&self, relative: &str) -> Option<PathBuf> {
        let p = Path::new(relative);
        if p.is_absolute() && p.exists() {
            return Some(p.to_path_buf());
        }
        if let Some(root) = &self.project_root {
            let candidate = root.join(relative);
            if candidate.exists() {
                return Some(candidate);
            }
        }
        let candidate = self.engine_assets_root.join(relative);
        if candidate.exists() {
            return Some(candidate);
        }
        None
    }

    /// Spawn a background thumbnail task for `path` if not already in flight.
    fn ensure_thumbnail(&mut self, path: String, cx: &mut Context<Self>) {
        if self.thumbnail_requested.contains(&path) {
            return;
        }
        self.thumbnail_requested.insert(path.clone());

        let Some(abs_path) = self.resolve_path(&path) else {
            // Path can't be resolved yet — skip silently.
            return;
        };

        let cache_root = self.thumbnail_cache_root.clone();
        let (tx, rx) = smol::channel::bounded::<Option<Arc<image::RgbaImage>>>(1);

        engine_fs::thumbnails::service().request(abs_path, cache_root, move |rgba| {
            smol::block_on(tx.send(rgba));
        });

        let path_key = path.clone();
        cx.spawn(async move |this, cx| {
            let Ok(Some(rgba)) = rx.recv().await else {
                return;
            };

            let render_image = Arc::new(gpui::RenderImage::new(smallvec::smallvec![
                image::Frame::new((*rgba).clone().into())
            ]));

            let _ = cx.update(|cx| {
                this.update(cx, |picker, cx| {
                    if let Some(item) = picker.items.iter_mut().find(|i| i.path == path_key) {
                        item.thumbnail = Some(render_image);
                    }
                    let updated = picker.items.clone();
                    picker.searchable_list.update(cx, |list, cx| {
                        list.set_items(updated, cx);
                    });
                    cx.notify();
                })
            });
        })
        .detach();
    }
}

impl Render for MeshAssetPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Mirror the file drawer: request thumbnails for each item as it becomes
        // visible (i.e. when the picker is open and rendered). The
        // thumbnail_requested set makes repeated render passes no-ops.
        let paths: Vec<String> = self.items.iter().map(|i| i.path.clone()).collect();
        for path in paths {
            self.ensure_thumbnail(path, cx);
        }
        self.searchable_list.clone()
    }
}

fn query_assets(project_root: &Path, queries: &[AssetQuery]) -> Vec<String> {
    let extensions: std::collections::HashSet<String> = queries
        .iter()
        .filter_map(|q| match q {
            AssetQuery::Extension(ext) => Some(ext.trim_start_matches('.').to_ascii_lowercase()),
            _ => None,
        })
        .collect();

    if extensions.is_empty() {
        return vec![];
    }

    // Single manifest walk — far cheaper than one walk per extension.
    let Ok(entries) = engine_fs::virtual_fs::manifest(project_root) else {
        return vec![];
    };

    let mut out = BTreeSet::new();
    for entry in entries {
        if entry.is_dir {
            continue;
        }
        let matches = std::path::Path::new(&entry.path)
            .extension()
            .and_then(|x| x.to_str())
            .map(|x| extensions.contains(x.to_ascii_lowercase().as_str()))
            .unwrap_or(false);
        if matches {
            out.insert(normalize_asset_path(&entry.path));
        }
    }

    out.into_iter().collect()
}

fn normalize_asset_path(path: impl AsRef<str>) -> String {
    path.as_ref().replace('\\', "/")
}
