//! The mocap editor: one dock workspace over one session model.

use crate::model::SessionModel;
use crate::panels::{InspectorPanel, ScenePanel, SourcesPanel, TimelinePanel, ViewportPanel};
use gpui::*;
use std::path::PathBuf;
use std::sync::Arc;
use ui::dock::{DockChannel, DockItem, Panel, PanelEvent, PanelView};
use ui::workspace::Workspace;
use ui::{h_flex, ActiveTheme};

/// The dock channel the mocap editor's panels share (others are 1 and 3 to 7).
const DOCK_CHANNEL: u32 = 8;

/// The editor panel opened on a `.mocap` file.
pub struct MocapEditor {
    path: PathBuf,
    model: Entity<SessionModel>,
    workspace: Option<Entity<Workspace>>,
    focus_handle: FocusHandle,
}

impl MocapEditor {
    /// An editor on `path`: a non-empty file opens as a take, an empty or new one starts a session.
    pub fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let model = cx.new(|_| SessionModel::new(path.clone()));
        // Re-render (and so pump) when a task or the panels change the model.
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        Self { path, model, workspace: None, focus_handle: cx.focus_handle() }
    }

    fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let workspace = cx.new(|cx| Workspace::new_with_channel("mocap-workspace", DockChannel(DOCK_CHANNEL), window, cx));
        workspace.update(cx, |workspace, cx| {
            let dock = workspace.dock_area().downgrade();
            let viewport: Arc<dyn PanelView> = Arc::new(cx.new(|cx| ViewportPanel::new(model.clone(), cx)));
            let scene: Arc<dyn PanelView> = Arc::new(cx.new(|cx| ScenePanel::new(model.clone(), cx)));
            let sources: Arc<dyn PanelView> = Arc::new(cx.new(|cx| SourcesPanel::new(model.clone(), cx)));
            let inspector: Arc<dyn PanelView> = Arc::new(cx.new(|cx| InspectorPanel::new(model.clone(), cx)));
            let timeline: Arc<dyn PanelView> = Arc::new(cx.new(|cx| TimelinePanel::new(model.clone(), cx)));
            let center = DockItem::split(
                Axis::Horizontal,
                vec![
                    DockItem::tabs(vec![viewport], Some(0), &dock, window, cx),
                    DockItem::tabs(vec![scene], Some(0), &dock, window, cx),
                ],
                &dock,
                window,
                cx,
            );
            let left = DockItem::tabs(vec![sources], Some(0), &dock, window, cx);
            let right = DockItem::tabs(vec![inspector], Some(0), &dock, window, cx);
            let bottom = DockItem::tabs(vec![timeline], Some(0), &dock, window, cx);
            workspace.initialize(center, Some(left), Some(right), Some(bottom), window, cx);
        });
        self.workspace = Some(workspace);
    }
}

impl EventEmitter<PanelEvent> for MocapEditor {}

impl Focusable for MocapEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for MocapEditor {
    fn panel_name(&self) -> &'static str {
        "Mocap Editor"
    }

    fn panel_file_path(&self, _cx: &App) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        let name = self.path.file_name().and_then(|n| n.to_str()).unwrap_or("Mocap").to_owned();
        h_flex().gap_2().items_center().child(div().text_sm().child(name)).into_any_element()
    }

    fn dump(&self, _cx: &App) -> ui::dock::PanelState {
        ui::dock::PanelState { panel_name: self.panel_name().to_string(), ..Default::default() }
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for MocapEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.workspace.is_none() {
            self.initialize_workspace(window, cx);
        }
        // One tick per frame while anything is live: results of tasks, the pose
        // bus, playback. The model notifies the panels that changed.
        let keep_going = self.model.update(cx, |model, cx| {
            if model.pump() {
                cx.notify();
            }
            model.needs_frames()
        });
        if keep_going {
            window.request_animation_frame();
        }
        let content = match &self.workspace {
            Some(workspace) => workspace.clone().into_any_element(),
            None => div().size_full().into_any_element(),
        };
        div().size_full().bg(cx.theme().background).child(content)
    }
}
