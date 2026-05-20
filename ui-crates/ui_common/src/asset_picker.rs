use gpui::{prelude::*, *};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use ui::{
    dropdown::{SearchableList, SearchableListEvent},
    IconName,
};

#[derive(Debug, Clone, Copy)]
pub struct AssetPickedEvent;

#[derive(Clone)]
struct AssetItem {
    path: String,
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
    searchable_list: Entity<SearchableList<AssetItem>>,
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

        println!("[MeshAssetPicker::new] builtins.len() = {}", builtins.len());
        for builtin in builtins {
            let normalized = normalize_asset_path(&builtin);
            println!(
                "[MeshAssetPicker::new] adding builtin: {} -> {}",
                builtin, normalized
            );
            assets.insert(normalized);
        }

        if let Some(root) = project_root {
            for path in query_assets(&root, &queries) {
                println!("[MeshAssetPicker::new] adding queried asset: {}", path);
                assets.insert(path);
            }
        }

        println!(
            "[MeshAssetPicker::new] total assets collected: {}",
            assets.len()
        );

        let items = assets
            .into_iter()
            .map(|path| AssetItem { path })
            .collect::<Vec<_>>();

        let searchable_list = cx.new(|cx| {
            SearchableList::new(window, cx, items, |item| item.path.clone())
                .with_empty_text("No matching assets")
                .with_max_width(px(360.0))
                .with_max_height(px(420.0))
                .with_icon_getter(|_| IconName::Code)
        });

        let subscriptions = vec![cx.subscribe(
            &searchable_list,
            |this, _, event: &SearchableListEvent<AssetItem>, cx| {
                if let SearchableListEvent::Select(item) = event {
                    this.selected_path = item.path.clone();
                    cx.emit(AssetPickedEvent);
                    cx.emit(DismissEvent);
                }
            },
        )];

        Self {
            searchable_list,
            selected_path,
            _subscriptions: subscriptions,
        }
    }

    pub fn selected_path(&self) -> &str {
        &self.selected_path
    }
}

impl Render for MeshAssetPicker {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.searchable_list.clone()
    }
}

fn query_assets(project_root: &Path, queries: &[AssetQuery]) -> Vec<String> {
    let mut out = BTreeSet::new();

    println!("[query_assets] project_root={:?}", project_root);

    let extension_queries = queries
        .iter()
        .filter_map(|q| match q {
            AssetQuery::Extension(ext) => Some(ext.trim_start_matches('.').to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();

    println!("[query_assets] extension_queries={:?}", extension_queries);

    for ext in &extension_queries {
        let found = engine_fs::virtual_fs::find_by_extension(project_root, ext);
        println!(
            "[query_assets] find_by_extension({}) returned {} files",
            ext,
            found.len()
        );
        for path in found {
            if let Ok(rel) = path.strip_prefix(project_root) {
                let normalized = normalize_asset_path(rel.to_string_lossy());
                println!("[query_assets] ✓ {}", normalized);
                out.insert(normalized);
            }
        }
    }

    out.into_iter().collect()
}

fn normalize_asset_path(path: impl AsRef<str>) -> String {
    path.as_ref().replace('\\', "/")
}
