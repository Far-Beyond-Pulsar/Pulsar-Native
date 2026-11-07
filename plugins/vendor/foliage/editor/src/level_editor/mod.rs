//! Foliage in the level editor, registered through its extension points
//! (`ui_level_editor::extend`):
//! - the **Foliage** tool mode: the palette of foliage sets and its brush
//!   ([`panel::FoliageSetsPanel`]), Unreal's Foliage mode beside its
//!   Landscape mode (the voxel worlds' Terrain mode is the voxel plugin's);
//! - the palette saved with the level (the level file's editor section,
//!   key `foliage_sets`);
//! - the level's global wind ([`wind`]): its Create entry and World Settings
//!   section.

pub mod brush;
pub mod foliage_sets;
pub mod panel;
pub mod wind;

use std::sync::{Arc, LazyLock};

use gpui::AppContext as _;
use ui_level_editor::extend::{EditorData, PluginState as _, ToolModeRegistration};
use ui_level_editor::{
    LevelEditorState, ModeLayout, ModePanelContext, StatusReadout, ToolMode, ToolModeContext,
    ToolModeId, ToolPointerEvent, ToolPointerResult,
};

pub use brush::{FoliageBrush, FoliageDomain, FoliageTool};

ui_level_editor::plugin_state!(FoliageDomain);

/// The foliage tool's state (its default before the tool was used).
pub fn domain(state: &LevelEditorState) -> &FoliageDomain {
    static DEFAULT: LazyLock<FoliageDomain> = LazyLock::new(FoliageDomain::default);
    state
        .editor
        .plugins
        .get::<FoliageDomain>()
        .unwrap_or(&DEFAULT)
}

/// Edit the foliage tool's state. The palette saves with the level, so a
/// change to it marks the level unsaved.
pub fn edit_foliage(state: &mut LevelEditorState, edit: impl FnOnce(&mut FoliageDomain)) {
    let foliage = state.editor.plugins.get_mut::<FoliageDomain>();
    let before = foliage.foliage_sets.clone();
    edit(foliage);
    if before != foliage.foliage_sets {
        state.scene.has_unsaved_changes = true;
    }
}

// ── The Foliage mode ────────────────────────────────────────────────────────

/// The Foliage mode's id.
pub const FOLIAGE_MODE: ToolModeId = ToolModeId("foliage");

#[derive(Clone, Copy, Default)]
pub struct FoliageMode;

impl ToolMode for FoliageMode {
    fn id(&self) -> ToolModeId {
        FOLIAGE_MODE
    }
    fn label(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.Foliage").to_string()
    }
    fn icon(&self) -> ui::IconName {
        ui::IconName::Leaf
    }
    fn description(&self) -> String {
        rust_i18n::t!("LevelEditor.ToolMode.FoliageDesc").to_string()
    }
    fn on_mode_entered(&mut self, _ctx: &mut ToolModeContext) {}
    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {}
    fn layout(&self) -> ModeLayout {
        ModeLayout {
            show_right_dock: true,
            dock_width: Some(380.0),
        }
    }
    fn build_panels(
        &self,
        ctx: &mut ModePanelContext<'_, '_>,
    ) -> Vec<Arc<dyn ui::dock::PanelView>> {
        let state = ctx.state.clone();
        let window = &mut *ctx.window;
        let panel = ctx
            .cx
            .new(|cx| panel::FoliageSetsPanel::new(state, window, cx));
        vec![Arc::new(panel)]
    }
    fn status(&self, _ctx: &ToolModeContext) -> Option<StatusReadout> {
        Some(StatusReadout {
            text: rust_i18n::t!("LevelEditor.Terrain.Status").to_string(),
            tooltip: Some(rust_i18n::t!("LevelEditor.Terrain.NavigationHint").to_string()),
        })
    }
    fn on_pointer(
        &mut self,
        _event: &ToolPointerEvent,
        _ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        ToolPointerResult::PassThrough
    }
    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(*self)
    }
}

inventory::submit! {
    ToolModeRegistration {
        origin: pulsar_plugins::origin!(),
        order: 30,
        make: || Box::new(FoliageMode),
    }
}

// ── The palette in the level file ───────────────────────────────────────────

inventory::submit! {
    EditorData {
        origin: pulsar_plugins::origin!(),
        key: "foliage_sets",
        save: |state| serde_json::to_value(&domain(state).foliage_sets).ok(),
        load: |state, value| {
            let library = value
                .and_then(|value| {
                    serde_json::from_value::<foliage_sets::FoliageSetLibrary>(value.clone())
                        .map_err(|error| tracing::warn!("Foliage palette not read: {error}"))
                        .ok()
                })
                .unwrap_or_default();
            state.editor.plugins.get_mut::<FoliageDomain>().foliage_sets = library;
        },
    }
}

// ── The panel kit, in this plugin's text and state ──────────────────────────

/// The level editor's panel kit (`ui_level_editor::extend::panel_kit`) over
/// the foliage tool's state, with this plugin's text.
pub(crate) mod kit {
    use std::sync::Arc;

    use gpui::{Context, IntoElement};
    use ui::IconName;
    use ui_level_editor::extend::panel_kit;

    pub use panel_kit::SharedState;

    use super::{FoliageDomain, edit_foliage};

    pub fn section(theme: &ui::Theme, label_key: &'static str) -> impl IntoElement {
        panel_kit::section(theme, rust_i18n::t!(label_key).to_string())
    }

    pub struct ToolSpec {
        pub id: &'static str,
        pub icon: IconName,
        pub label_key: &'static str,
        pub active: bool,
        pub apply: Arc<dyn Fn(&mut FoliageDomain) + Send + Sync>,
    }

    pub fn tool_grid(
        theme: &ui::Theme,
        state: SharedState,
        tools: Vec<ToolSpec>,
    ) -> impl IntoElement {
        let tools = tools
            .into_iter()
            .map(|tool| {
                let apply = tool.apply;
                panel_kit::ToolSpec {
                    id: tool.id,
                    icon: tool.icon,
                    label: rust_i18n::t!(tool.label_key).to_string(),
                    active: tool.active,
                    apply: Arc::new(move |state| edit_foliage(state, |foliage| apply(foliage))),
                }
            })
            .collect();
        panel_kit::tool_grid(theme, state, tools)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stepper_row<V, F>(
        state: SharedState,
        cx: &mut Context<V>,
        control_id: String,
        label: String,
        value: f32,
        min: f32,
        max: f32,
        step: f32,
        apply: F,
    ) -> impl IntoElement
    where
        V: 'static,
        F: Fn(&mut FoliageDomain, f32) + Clone + Send + Sync + 'static,
    {
        panel_kit::stepper_row(
            state,
            cx,
            control_id,
            label,
            value,
            min,
            max,
            step,
            move |state, value| edit_foliage(state, |foliage| apply(foliage, value)),
        )
    }

    pub fn checkbox_row<F>(
        state: SharedState,
        control_id: String,
        label: String,
        checked: bool,
        apply: F,
    ) -> impl IntoElement
    where
        F: Fn(&mut FoliageDomain, bool) + Send + Sync + 'static,
    {
        panel_kit::checkbox_row(state, control_id, label, checked, move |state, on| {
            edit_foliage(state, |foliage| apply(foliage, on))
        })
    }
}
