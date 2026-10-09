//! Hierarchy dock panel.

use crate::state::LevelEditorState;
use crate::ui::HierarchyPanel;
use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme, IconName, Sizable,
};

/// Hierarchy Panel
///
/// Self-refreshing: a frame pump compares a `(store_revision, selected)`
/// signature every platform frame and notifies this view only when something
/// the tree actually displays changed. No other panel needs to know this panel
/// exists — scene mutations from any thread (UI commands, AI tools, the render
/// thread's gizmo-drag release / click-select) all advance the store revision.
pub struct HierarchyPanelWrapper {
    hierarchy: HierarchyPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    focus_handle: FocusHandle,
    /// `(store_revision, selected)` last seen by the pump/render pair.
    last_signature: (u64, Option<String>),
    pump_started: bool,
}

impl HierarchyPanelWrapper {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_signature = {
            let state = state.read();
            (state.scene.world_revision(), state.scene.selected_object())
        };
        Self {
            hierarchy: HierarchyPanel::new(),
            state,
            gpu_engine,
            focus_handle: cx.focus_handle(),
            last_signature,
            pump_started: false,
        }
    }

    fn signature(&self) -> (u64, Option<String>) {
        let state = self.state.read();
        (state.scene.world_revision(), state.scene.selected_object())
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        crate::ui::frame_pump::spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let signature = this.signature();
            if signature != this.last_signature {
                this.last_signature = signature;
                cx.notify();
            }
        });
    }
}

impl EventEmitter<PanelEvent> for HierarchyPanelWrapper {}

ui_common::panel_boilerplate!(HierarchyPanelWrapper);

impl Render for HierarchyPanelWrapper {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("hierarchy panel: render");
        let _t = gpui::render_stats::scope("hierarchy panel: render");

        self.start_pump(_window, cx);

        // Record what we are about to paint so the pump doesn't immediately
        // re-notify for a change an explicit notify (e.g. expand toggle, row
        // click) has already picked up.
        self.last_signature = self.signature();

        let _state_scope = gpui::render_stats::scope("hierarchy panel: state read");
        let state = self.state.read();
        drop(_state_scope);

        let add_button = Button::new("add_object")
            .icon(IconName::Plus)
            .ghost()
            .xsmall()
            .tooltip(t!("LevelEditor.Hierarchy.AddObject"))
            .on_click({
                let state_clone = self.state.clone();
                let gpu_engine = self.gpu_engine.clone();
                move |_, _, _cx| {
                    use crate::commands::{execute_command, SceneCommand};
                    use crate::scene_edit::{ObjectType, SceneObjectData};

                    let transform =
                        crate::ui::hierarchy::editor_camera_spawn_transform(&gpu_engine);
                    let mut state = state_clone.write();
                    let new_object = SceneObjectData {
                        id: String::new(),
                        name: "New Object".to_string(),
                        object_type: ObjectType::Empty,
                        transform,
                        visible: true,
                        locked: false,
                        parent: None,
                        children: vec![],
                        scene_path: String::new(),
                        props: Default::default(),
                        component_instances: None,
                    };
                    execute_command(
                        &mut state,
                        SceneCommand::AddObject {
                            data: new_object,
                            parent_id: None,
                        },
                    );
                }
            })
            .into_any_element();

        let wrapper_entity = cx.entity().downgrade();

        v_flex().size_full().bg(cx.theme().sidebar).p_1().child({
            let _scope = gpui::render_stats::scope("hierarchy panel: element build");
            self.hierarchy.render(
                &state,
                self.state.clone(),
                self.gpu_engine.clone(),
                wrapper_entity,
                add_button,
                cx,
            )
        })
    }
}

impl Panel for HierarchyPanelWrapper {
    fn panel_name(&self) -> &'static str {
        "hierarchy"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Hierarchy".into_any_element()
    }
}
