//! Splitting the viewport into several, each its own dock panel.
//!
//! The toolbar's two split buttons divide the centre of the level editor's dock
//! into two or four viewports. Each is a [`ViewportPanelWrapper`] with its own
//! `GpuRenderer`, camera, input and overlays; all of them render the one scene
//! the editor owns (the same sharing Play-In-Editor uses for its renderer). The
//! panels are ordinary dock panels, so the user can resize them, drag them into
//! tabs of a single panel, or close them.
//!
//! The new viewports start looking through a fixed set of cameras: the original
//! stays the perspective view, the others look from the side, from above and
//! from the front.

use std::sync::Arc;

use engine_backend::scene::SceneWorldExt;
use engine_backend::subsystems::render::{
    CameraSync, EditorCameraState, HelioEditorMailbox, CAMERA_PIVOT_DISTANCE,
};
use gpui::*;
use ui::dock::{DockItem, PanelView};

use super::*;
use crate::state::CameraMode;
use crate::ui::viewport::view_settings::new_view_state;
use crate::ViewportPanelWrapper;

/// A viewport added by a split: what the editor needs to keep it in step with
/// the primary one.
#[derive(Clone)]
pub(crate) struct ExtraViewport {
    pub gpu: Arc<Mutex<GpuRenderer>>,
    pub mailbox: HelioEditorMailbox,
    pub panel: WeakEntity<ViewportPanelWrapper>,
}

/// The viewports added by splits. Holds them weakly: closing a viewport's tab
/// ends it, and it drops out of the list.
#[derive(Clone, Default)]
pub(crate) struct ExtraViewports(Arc<parking_lot::Mutex<Vec<ExtraViewport>>>);

impl ExtraViewports {
    /// The viewports that are still open.
    pub(crate) fn live(&self) -> Vec<ExtraViewport> {
        let mut viewports = self.0.lock();
        viewports.retain(|viewport| viewport.panel.upgrade().is_some());
        viewports.clone()
    }

    fn replace(&self, viewports: Vec<ExtraViewport>) {
        *self.0.lock() = viewports;
    }
}

/// How the centre of the dock is divided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ViewportLayout {
    Single,
    /// Perspective | side.
    Half,
    /// Perspective | side over top | front.
    Quad,
}

impl ViewportLayout {
    /// What the added viewports look through, in dock order.
    fn added_views(self) -> &'static [CameraMode] {
        match self {
            Self::Single => &[],
            Self::Half => &[CameraMode::Side],
            Self::Quad => &[CameraMode::Side, CameraMode::Top, CameraMode::Front],
        }
    }

    fn of_added(count: usize) -> Self {
        match count {
            0 => Self::Single,
            1 => Self::Half,
            _ => Self::Quad,
        }
    }
}

impl LevelEditorPanel {
    pub(super) fn on_split_viewports_half(
        &mut self,
        _: &SplitViewportsHalf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_viewport_layout(ViewportLayout::Half, window, cx);
    }

    pub(super) fn on_split_viewports_quad(
        &mut self,
        _: &SplitViewportsQuad,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_viewport_layout(ViewportLayout::Quad, window, cx);
    }

    /// Switch to `layout`, or back to one viewport when it is already showing.
    fn toggle_viewport_layout(
        &mut self,
        layout: ViewportLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = ViewportLayout::of_added(self.extra_viewports.live().len());
        let target = if current == layout {
            ViewportLayout::Single
        } else {
            layout
        };
        self.apply_viewport_layout(target, window, cx);
    }

    /// Rebuild the centre of the dock as `layout`. The primary viewport (and
    /// the Game tab, when open) stays; the added viewports are made anew.
    fn apply_viewport_layout(
        &mut self,
        layout: ViewportLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let Some(primary) = self.viewport_panel.as_ref().and_then(WeakEntity::upgrade) else {
            return;
        };

        if layout != ViewportLayout::Single {
            // The original is the perspective view of the layout.
            self.shared_state
                .write()
                .editor
                .set_camera_mode(CameraMode::Perspective);
        }

        // The viewports keep one camera position between them until the user
        // unlocks one.
        // They centre on the point the perspective camera is looking at, as
        // far out as it is from what is selected: a scene built large needs
        // long boom arms.
        let camera_group = Arc::new(match self.current_editor_camera_state() {
            Some(camera) => {
                let distance = self
                    .selection_distance(camera.position)
                    .unwrap_or(CAMERA_PIVOT_DISTANCE);
                CameraSync::at(
                    EditorCameraState {
                        position: camera.position,
                        yaw: camera.yaw,
                        pitch: camera.pitch,
                    }
                    .focus_point(distance),
                    distance,
                )
            }
            None => CameraSync::default(),
        });
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.join_camera_sync(camera_group.clone());
        }
        let added: Vec<(Entity<ViewportPanelWrapper>, ExtraViewport)> = layout
            .added_views()
            .iter()
            .filter_map(|mode| self.build_extra_viewport(*mode, &camera_group, window, cx))
            .collect();

        let primary_tabs: Vec<Arc<dyn PanelView>> = {
            let mut tabs: Vec<Arc<dyn PanelView>> = vec![Arc::new(primary)];
            if let Some(game) = self.game_panel.clone() {
                tabs.push(Arc::new(game));
            }
            tabs
        };
        let added_panels: Vec<Arc<dyn PanelView>> = added
            .iter()
            .map(|(panel, _)| Arc::new(panel.clone()) as Arc<dyn PanelView>)
            .collect();

        workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area().clone();
            let weak = dock_area.downgrade();
            let tabs = |panels: Vec<Arc<dyn PanelView>>, window: &mut Window, cx: &mut App| {
                DockItem::tabs(panels, Some(0), &weak, window, cx)
            };
            let row = |items: Vec<DockItem>, window: &mut Window, cx: &mut App| {
                let sizes = vec![None; items.len()];
                DockItem::split_with_sizes(Axis::Horizontal, items, sizes, &weak, window, cx)
            };

            let mut added_panels = added_panels.into_iter();
            let center = match layout {
                ViewportLayout::Single => tabs(primary_tabs, window, cx),
                ViewportLayout::Half => {
                    let left = tabs(primary_tabs, window, cx);
                    let right = tabs(added_panels.by_ref().take(1).collect(), window, cx);
                    row(vec![left, right], window, cx)
                }
                ViewportLayout::Quad => {
                    let top_left = tabs(primary_tabs, window, cx);
                    let top_right = tabs(added_panels.by_ref().take(1).collect(), window, cx);
                    let bottom_left = tabs(added_panels.by_ref().take(1).collect(), window, cx);
                    let bottom_right = tabs(added_panels.by_ref().take(1).collect(), window, cx);
                    let top = row(vec![top_left, top_right], window, cx);
                    let bottom = row(vec![bottom_left, bottom_right], window, cx);
                    DockItem::split_with_sizes(
                        Axis::Vertical,
                        vec![top, bottom],
                        vec![None, None],
                        &weak,
                        window,
                        cx,
                    )
                }
            };
            dock_area.update(cx, |dock_area, cx| {
                dock_area.set_center(center, window, cx);
            });
        });

        self.extra_viewports
            .replace(added.into_iter().map(|(_, extra)| extra).collect());
        cx.notify();
    }

    /// How far the selected object is from `eye`, when something is selected
    /// and the camera is not on top of it.
    fn selection_distance(&self, eye: [f64; 3]) -> Option<f64> {
        let scene = self.shared_state.read().scene.shared_scene();
        let scene = scene.read();
        let id = scene.world.selected_id()?;
        let position =
            crate::scene_edit::objects::get_object_transform(&scene.world, &id)?.position;
        let distance = position
            .iter()
            .zip(eye)
            .map(|(object, eye)| (f64::from(*object) - eye).powi(2))
            .sum::<f64>()
            .sqrt();
        (distance.is_finite() && distance >= 1.0).then_some(distance)
    }

    /// A new viewport looking through `mode`, over this editor's scene.
    fn build_extra_viewport(
        &mut self,
        mode: CameraMode,
        camera_group: &Arc<CameraSync>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(Entity<ViewportPanelWrapper>, ExtraViewport)> {
        let scene_store = self.shared_state.read().scene.shared_scene();
        let mut builder = GpuRendererBuilder::new(1600, 900).scene_db(scene_store);
        if let Some(physics) = engine_backend::EngineBackend::global()
            .and_then(|backend| backend.read().get_physics_query_service())
        {
            builder = builder.physics(physics);
        }
        let gpu = Arc::new(Mutex::new(builder.build()));
        let mailbox = gpu.lock().ok().and_then(|engine| engine.editor_mailbox())?;

        mailbox.join_camera_sync(camera_group.clone());

        // Start where the primary camera is, with the tool the user has.
        if let Some(camera) = self.current_editor_camera_state() {
            mailbox.queue_camera(EditorCameraState {
                position: camera.position,
                yaw: camera.yaw,
                pitch: camera.pitch,
            });
        }
        let tool = self.shared_state.read().editor.current_tool;
        let (_, gizmo_mode) = Self::tool_to_gizmo(tool);
        mailbox.queue_gizmo(gizmo_mode);

        let shared_state = self.shared_state.clone();
        let viewport =
            cx.new(|cx| HelioViewport::new(gpu.clone(), shared_state.clone(), false, cx));
        let inner = ViewportPanel::new(
            viewport,
            self.render_enabled.clone(),
            Arc::new(InputState::new()),
            self.toolbar.clone(),
            gpu.clone(),
            new_view_state(mode),
            true,
            window,
            cx,
        );
        let panel = cx
            .new(|cx| ViewportPanelWrapper::new(inner, shared_state, gpu.clone(), cx).split_off());
        let extra = ExtraViewport {
            gpu,
            mailbox,
            panel: panel.downgrade(),
        };
        Some((panel, extra))
    }
}
