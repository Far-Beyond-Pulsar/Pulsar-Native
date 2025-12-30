use std::path::PathBuf;
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _}, h_flex, v_flex, scroll::ScrollbarAxis, ActiveTheme, IconName, Selectable, Sizable, StyledExt,
};

/// Asset Browser - Browse and preview project assets
pub struct AssetBrowser {
    project_path: Option<PathBuf>,
    current_folder: Option<PathBuf>,
    available_assets: Vec<AssetEntry>,
    view_mode: ViewMode,
}

#[derive(Clone, Debug)]
struct AssetEntry {
    path: PathBuf,
    name: String,
    asset_type: AssetType,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AssetType {
    Mesh,
    Material,
    Texture,
    Script,
    Audio,
    Scene,
    Folder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewMode {
    Grid,
    List,
}

impl AssetBrowser {
    pub fn new() -> Self {
        Self {
            project_path: None,
            current_folder: None,
            available_assets: Vec::new(),
            view_mode: ViewMode::Grid,
        }
    }

    pub fn set_project_path(&mut self, path: PathBuf) {
        self.project_path = Some(path.clone());
        let assets_folder = path.join("assets");
        self.current_folder = Some(assets_folder.clone());
        self.refresh_assets();
    }

    fn refresh_assets(&mut self) {
        self.available_assets.clear();

        if let Some(ref folder) = self.current_folder {
            if folder.exists() {
                if let Ok(entries) = std::fs::read_dir(folder) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        let name = path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Unknown")
                            .to_string();

                        let asset_type = if path.is_dir() {
                            AssetType::Folder
                        } else {
                            Self::determine_asset_type(&path)
                        };

                        self.available_assets.push(AssetEntry {
                            path,
                            name,
                            asset_type,
                        });
                    }
                }
            }
        }

        // Sort: folders first, then by name
        self.available_assets.sort_by(|a, b| {
            match (a.asset_type, b.asset_type) {
                (AssetType::Folder, AssetType::Folder) => a.name.cmp(&b.name),
                (AssetType::Folder, _) => std::cmp::Ordering::Less,
                (_, AssetType::Folder) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });
    }

    fn determine_asset_type(path: &PathBuf) -> AssetType {
        match path.extension().and_then(|s| s.to_str()) {
            Some("obj") | Some("fbx") | Some("gltf") | Some("glb") => AssetType::Mesh,
            Some("mat") => AssetType::Material,
            Some("png") | Some("jpg") | Some("jpeg") | Some("tga") => AssetType::Texture,
            Some("rs") | Some("lua") => AssetType::Script,
            Some("wav") | Some("mp3") | Some("ogg") => AssetType::Audio,
            Some("scene") => AssetType::Scene,
            _ => AssetType::Mesh, // Default
        }
    }

    pub fn render(&self, cx: &mut App) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap_2()
            .child(
                // Header with navigation and view mode toggle
                h_flex()
                    .w_full()
                    .p_2()
                    .justify_between()
                    .items_center()
                    .bg(cx.theme().sidebar)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Button::new("asset_back")
                                    .icon(IconName::ArrowLeft)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Back")
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(
                                        self.current_folder
                                            .as_ref()
                                            .and_then(|p| p.file_name())
                                            .and_then(|s| s.to_str())
                                            .unwrap_or("Assets")
                                            .to_string()
                                    )
                            )
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("view_grid")
                                    .icon(IconName::ViewGrid)
                                    .ghost()
                                    .xsmall()
                                    .selected(matches!(self.view_mode, ViewMode::Grid))
                            )
                            .child(
                                Button::new("view_list")
                                    .icon(IconName::List)
                                    .ghost()
                                    .xsmall()
                                    .selected(matches!(self.view_mode, ViewMode::List))
                            )
                            .child(
                                Button::new("refresh_assets")
                                    .icon(IconName::Refresh)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Refresh")
                            )
                    )
            )
            .child(
                // Asset grid/list
                div()
                    .flex_1()
                    .overflow_hidden()
                    .p_2()
                    .child(
                        v_flex()
                            .size_full()
                            .scrollable(ScrollbarAxis::Vertical)
                            .child(
                                match self.view_mode {
                                    ViewMode::Grid => self.render_grid_view(cx).into_any_element(),
                                    ViewMode::List => self.render_list_view(cx).into_any_element(),
                                }
                            )
                    )
            )
    }

    fn render_grid_view(&self, cx: &App) -> impl IntoElement {
        div()
            .w_full()
            .grid()
            .grid_cols(4)
            .gap_2()
            .children(
                self.available_assets.iter().map(|asset| {
                    self.render_asset_grid_item(asset, cx)
                })
            )
    }

    fn render_list_view(&self, cx: &App) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .children(
                self.available_assets.iter().map(|asset| {
                    self.render_asset_list_item(asset, cx)
                })
            )
    }

    fn render_asset_grid_item(&self, asset: &AssetEntry, cx: &App) -> impl IntoElement {
        v_flex()
            .gap_1()
            .p_2()
            .rounded(cx.theme().radius)
            .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
            .child(
                // Asset preview/icon (square)
                div()
                    .w_full()
                    .h(px(100.0))
                    .bg(cx.theme().muted.opacity(0.3))
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .text_2xl()
                            .child(Self::get_icon_for_asset_type(asset.asset_type))
                    )
            )
            .child(
                // Asset name
                div()
                    .w_full()
                    .text_xs()
                    .text_center()
                    .text_color(cx.theme().foreground)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(asset.name.clone())
            )
    }

    fn render_asset_list_item(&self, asset: &AssetEntry, cx: &App) -> impl IntoElement {
        h_flex()
            .w_full()
            .gap_2()
            .items_center()
            .p_2()
            .rounded(cx.theme().radius)
            .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
            .child(
                div()
                    .text_color(cx.theme().foreground)
                    .child(Self::get_icon_for_asset_type(asset.asset_type))
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(asset.name.clone())
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{:?}", asset.asset_type))
            )
    }

    fn get_icon_for_asset_type(asset_type: AssetType) -> &'static str {
        match asset_type {
            AssetType::Mesh => "🧊",
            AssetType::Material => "🎨",
            AssetType::Texture => "🖼️",
            AssetType::Script => "📜",
            AssetType::Audio => "🔊",
            AssetType::Scene => "🎬",
            AssetType::Folder => "📁",
        }
    }
}
