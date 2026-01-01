use gpui::{*, prelude::FluentBuilder, actions};
use ui::{
    v_flex, h_flex, ActiveTheme, StyledExt, IconName,
    dock::{Panel, PanelEvent, DockItem, DockChannel},
    workspace::Workspace,
    button::{Button, ButtonVariants},
    divider::Divider,
    input::{InputState, TextInput},
};
use ui_types_common::{TraitAsset, TraitMethod, MethodSignature, TypeRef, TypeKind};
use std::path::PathBuf;
use std::sync::Arc;
use crate::method_editor::{MethodEditorView, MethodEditorEvent};
use crate::workspace_panels::{PropertiesPanel, MethodsPanel, CodePreviewPanel};

actions!(trait_editor, [
    Save,
    AddMethod,
    TogglePreview,
]);

#[derive(Clone, Debug)]
pub enum TraitEditorEvent {
    Modified,
    Saved,
}

pub struct TraitEditor {
    file_path: Option<PathBuf>,
    asset: Arc<parking_lot::RwLock<TraitAsset>>,
    error_message: Option<String>,
    focus_handle: FocusHandle,

    // Workspace for dock panels
    workspace: Option<Entity<Workspace>>,

    // Modified flag
    modified: bool,
}

impl TraitEditor {
    pub fn new_with_file(file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Try to load the trait data
        let (asset, error_message) = match std::fs::read_to_string(&file_path) {
            Ok(json_content) => {
                match serde_json::from_str::<TraitAsset>(&json_content) {
                    Ok(asset) => (asset, None),
                    Err(e) => (
                        Self::create_empty_asset(),
                        Some(format!("Failed to parse trait: {}", e))
                    ),
                }
            }
            Err(_) => (Self::create_empty_asset(), None),
        };

        let mut editor = Self {
            file_path: Some(file_path),
            asset: Arc::new(parking_lot::RwLock::new(asset)),
            error_message,
            focus_handle: cx.focus_handle(),
            workspace: None,
            modified: false,
        };

        // Initialize workspace with panels
        editor.initialize_workspace(window, cx);

        editor
    }

    fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        // Use channel 6 for trait editor to isolate from other editors
        let workspace = cx.new(|cx| {
            Workspace::new_with_channel(
                "trait-editor-workspace",
                DockChannel(6),
                window,
                cx,
            )
        });

        let asset_clone = self.asset.clone();

        workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area().downgrade();

            // Create Properties Panel (left)
            let properties_panel = cx.new(|cx| {
                PropertiesPanel::new(asset_clone.clone(), window, cx)
            });

            // Create Methods Panel (center)
            let methods_panel = cx.new(|cx| {
                MethodsPanel::new(asset_clone.clone(), window, cx)
            });

            // Create Code Preview Panel (right)
            let code_preview_panel = cx.new(|cx| {
                CodePreviewPanel::new(asset_clone.clone(), window, cx)
            });

            // Setup dock layout - all panels in tabs for consistency
            let center = DockItem::tabs(
                vec![Arc::new(methods_panel) as Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );
            let left = DockItem::tabs(
                vec![Arc::new(properties_panel) as Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );
            let right = DockItem::tabs(
                vec![Arc::new(code_preview_panel) as Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Initialize dock areas with custom widths
            dock_area.update(cx, |dock_area, cx| {
                dock_area.set_center(center, window, cx);
                dock_area.set_left_dock(left, Some(px(300.0)), true, window, cx);
                dock_area.set_right_dock(right, Some(px(400.0)), true, window, cx);
            });
        });

        self.workspace = Some(workspace);
    }

    fn create_empty_asset() -> TraitAsset {
        TraitAsset {
            schema_version: 1,
            type_kind: TypeKind::Trait,
            name: String::from("NewTrait"),
            display_name: String::from("New Trait"),
            description: None,
            methods: Vec::new(),
            meta: serde_json::Value::Object(serde_json::Map::new()),
        }
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        self.file_path.clone()
    }

    fn save(&mut self, _: &Save, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file_path) = &self.file_path {
            let asset = self.asset.read();
            match serde_json::to_string_pretty(&*asset) {
                Ok(json) => {
                    drop(asset); // Release the read lock before writing
                    if let Err(e) = std::fs::write(file_path, json) {
                        self.error_message = Some(format!("Failed to save: {}", e));
                    } else {
                        self.error_message = None;
                        self.modified = false;
                        tracing::info!("✅ Saved trait to {:?}", file_path);
                        cx.emit(TraitEditorEvent::Saved);
                    }
                }
                Err(e) => {
                    self.error_message = Some(format!("Failed to serialize: {}", e));
                }
            }
        }
        cx.notify();
    }
}

impl EventEmitter<TraitEditorEvent> for TraitEditor {}
impl EventEmitter<PanelEvent> for TraitEditor {}

impl Render for TraitEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(ref workspace) = self.workspace {
            workspace.clone().into_any_element()
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child("Initializing workspace...")
                .into_any_element()
        }
    }
}

impl Focusable for TraitEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for TraitEditor {
    fn panel_name(&self) -> &'static str {
        "Trait Editor"
    }

    fn title(&self, _window: &Window, _cx: &App) -> gpui::AnyElement {
        let asset = self.asset.read();
        format!(
            "{}{}",
            asset.display_name,
            if self.modified { " •" } else { "" }
        )
        .into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        let info = self.file_path.as_ref().map(|p| {
            serde_json::json!({
                "file_path": p.to_string_lossy().to_string()
            })
        }).unwrap_or(serde_json::Value::Null);

        ui::dock::PanelState {
            panel_name: self.panel_name().to_string(),
            info: ui::dock::PanelInfo::Panel(info),
            ..Default::default()
        }
    }
}
