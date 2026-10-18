//! `ToolbarView` — the level editor toolbar as its own cached GPUI view.
//!
//! The toolbar used to be rendered inline by `LevelEditorPanel::render`, which
//! meant its menu, viewport controls, save buttons and their translated labels
//! were rebuilt on every frame the panel was dirty. The panel is dirty on every
//! frame Helio publishes, so that was ~60 full rebuilds a second of a toolbar
//! that changes only when the user clicks something.
//!
//! As a separate entity it can be rendered with [`gpui::AnyView::cached`], which
//! replays the previous frame's prepaint/paint ranges and skips `render()`
//! entirely while the view is clean.
//!
//! Invalidation is explicit because the toolbar's inputs live in
//! `Arc<parking_lot::RwLock<LevelEditorState>>`, which GPUI's automatic
//! entity-access tracking cannot see. A per-frame [`frame_pump`] compares a
//! [`ToolbarSignature`] — a `PartialEq` snapshot of exactly the fields the
//! toolbar renders — and notifies only on a real change.
//!
//! **If you add a state read to any toolbar sub-renderer, add the corresponding
//! field to [`ToolbarSignature`]**, or the toolbar will render stale.

use std::sync::Arc;

use engine_backend::services::gpu_renderer::GpuRenderer;
use engine_backend::subsystems::render::{HelioEditorMailbox, ViewportDebugView};
use gpui::*;
use ui::dock::PanelEvent;
use ui::dropdown::{DropdownEvent, SearchableVec};
use ui::input::{InputEvent, InputState, TextInput};

use super::snap_controls::{SnapKind, SnapPanel};
use super::view_mode_dropdown::{
    ViewModeDropdown, ViewModeOption, ViewModePickerState, ViewModeSelection,
};
use super::ToolbarPanel;
use crate::state::EditorMode;
use crate::tool_modes::ToolModeId;
use crate::ui::frame_pump::spawn_frame_pump;
use crate::ui::viewport::components::camera_selector::CameraSpeedControl;
use crate::LevelEditorState;

/// Every piece of [`LevelEditorState`] the toolbar's element tree depends on.
///
/// Kept flat so building one per frame stays simple and explicit.
#[derive(Clone, PartialEq)]
pub struct ToolbarSignature {
    // tool_mode_dropdown / mode_indicator
    tool_mode: ToolModeId,
    // mode_indicator
    editor_mode: EditorMode,
    // feature_toggles
    feature_lighting_enabled: bool,
    feature_shadows_enabled: bool,
    feature_bloom_enabled: bool,
    feature_materials_enabled: bool,
    realtime_viewport: bool,
    viewport_debug_mode: u32,
    viewport_debug_views: Vec<ViewportDebugView>,
    transform_tool: u8,
    camera_mode: u8,
    camera_move_speed: u32,
    show_gpu_pipeline_overlay: bool,
    show_nerds_overlay: bool,
    location_snap: u32,
    rotation_snap: u32,
    scale_snap: u32,
    // profiling button
    show_performance_overlay: bool,
    // Edit menu: what Undo/Redo show and whether the selection items are
    // enabled (Pulsar-Native#1012)
    history_revision: u64,
    has_selection: bool,
    // Assets menu: one New item per creatable file type
    creatable_file_types: usize,
}

impl ToolbarSignature {
    fn of(
        state: &LevelEditorState,
        debug_views: &[ViewportDebugView],
        camera_move_speed: u32,
    ) -> Self {
        Self {
            tool_mode: state.editor.tool_mode_registry.selected_id(),
            editor_mode: state.scene.editor_mode,
            feature_lighting_enabled: state.editor.feature_lighting_enabled,
            feature_shadows_enabled: state.editor.feature_shadows_enabled,
            feature_bloom_enabled: state.editor.feature_bloom_enabled,
            feature_materials_enabled: state.editor.feature_materials_enabled,
            realtime_viewport: state.editor.realtime_viewport,
            viewport_debug_mode: state.editor.viewport_debug_mode,
            viewport_debug_views: debug_views.to_vec(),
            transform_tool: state.editor.current_tool as u8,
            camera_mode: state.editor.camera_mode as u8,
            camera_move_speed,
            show_gpu_pipeline_overlay: state.overlays.state.show_gpu_pipeline_overlay,
            show_nerds_overlay: state.overlays.state.show_nerds_overlay,
            location_snap: state.editor.location_snap.to_bits(),
            rotation_snap: state.editor.rotation_snap.to_bits(),
            scale_snap: state.editor.scale_snap.to_bits(),
            show_performance_overlay: state.overlays.state.show_performance_overlay,
            history_revision: plugin_editor_api::undo::lock(state.scene.history()).revision(),
            has_selection: state.scene.selected_object().is_some(),
            creatable_file_types: super::level_editor_menus::creatable_file_type_count(),
        }
    }
}

pub struct ToolbarView {
    toolbar: ToolbarPanel,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
    /// Carries the viewport feature toggles to the render thread without
    /// taking `gpu_engine`; see `HelioEditorMailbox`'s doc.
    helio_mailbox: Option<HelioEditorMailbox>,
    debug_views: Vec<ViewportDebugView>,
    pub(crate) view_mode_picker: Entity<ViewModePickerState>,
    input_state: Arc<crate::ui::viewport::input_state::InputState>,
    last_signature: ToolbarSignature,
    pump_started: bool,
    custom_snaps: [Entity<InputState>; 3],
    pub(crate) snap_panels: [Entity<SnapPanel>; 3],
    _custom_snap_subscriptions: Vec<Subscription>,
    _view_mode_picker_subscription: Subscription,
}

impl ToolbarView {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        helio_mailbox: Option<HelioEditorMailbox>,
        input_state: Arc<crate::ui::viewport::input_state::InputState>,
    ) -> Self {
        let debug_views = helio_mailbox
            .as_ref()
            .map(HelioEditorMailbox::viewport_debug_views)
            .unwrap_or_default();
        let last_signature = ToolbarSignature::of(
            &state.read(),
            &debug_views,
            input_state.get_move_speed().to_bits(),
        );
        let current_state = state.read();
        let selected_view_mode =
            ViewModeDropdown::selected(current_state.editor.viewport_debug_mode);
        let view_mode_picker =
            ViewModeDropdown::new_state(window, cx, &debug_views, selected_view_mode);
        drop(current_state);
        let view_mode_picker_subscription =
            Self::subscribe_view_mode_picker(&state, &view_mode_picker, cx);
        if let Some(mailbox) = &helio_mailbox {
            let state = state.read();
            mailbox.set_gizmo_snap_settings(
                state.editor.location_snap,
                state.editor.rotation_snap,
                state.editor.scale_snap,
            );
        }
        let custom_snaps: [Entity<InputState>; 3] = ["Custom", "Custom", "Custom"]
            .map(|placeholder| cx.new(|cx| InputState::new(window, cx).placeholder(placeholder)));
        let mut custom_snap_subscriptions = Vec::with_capacity(3);
        for (index, input) in custom_snaps.iter().enumerate() {
            let state_for_input = state.clone();
            let input_for_event = input.clone();
            let mailbox_for_input = helio_mailbox.clone();
            custom_snap_subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |_this, _, event: &InputEvent, _, cx| {
                    if matches!(
                        event,
                        InputEvent::Change | InputEvent::Blur | InputEvent::PressEnter { .. }
                    ) {
                        input_for_event.update(cx, |input, _| {
                                    if let Ok(value) = input.text().to_string().parse::<f32>() {
                                        if value.is_finite() && value > 0.0 {
                                            let (location, rotation, scale, persisted) = {
                                                let mut state = state_for_input.write();
                                                match index {
                                                    0 => state.editor.location_snap = value,
                                                    1 => state.editor.rotation_snap = value,
                                                    _ => state.editor.scale_snap = value,
                                                }
                                                (
                                                    state.editor.location_snap,
                                                    state.editor.rotation_snap,
                                                    state.editor.scale_snap,
                                                    value,
                                                )
                                            };
                                            let key = match index {
                                                0 => "location_snap",
                                                1 => "rotation_snap",
                                                _ => "scale_snap",
                                            };
                                            if let Err(error) = engine_state::GlobalSettings::new()
                                                .set_and_save(
                                                    "viewport",
                                                    key,
                                                    engine_state::ConfigValue::Float(
                                                        persisted as f64,
                                                    ),
                                                )
                                            {
                                                tracing::warn!(%error, "Could not persist viewport snap setting");
                                            }
                                            if let Some(mailbox) = &mailbox_for_input {
                                                mailbox.set_gizmo_snap_settings(location, rotation, scale);
                                            }
                                }
                            }
                        });
                    }
                },
            ));
        }
        let snap_panels: [Entity<SnapPanel>; 3] =
            [SnapKind::Location, SnapKind::Rotation, SnapKind::Scale].map(|kind| {
                let input = custom_snaps[match kind {
                    SnapKind::Location => 0,
                    SnapKind::Rotation => 1,
                    SnapKind::Scale => 2,
                }]
                .clone();
                let state = state.clone();
                let mailbox = helio_mailbox.clone();
                cx.new(|cx| SnapPanel::new(cx, kind, input, state, mailbox))
            });
        Self {
            toolbar: ToolbarPanel::new(),
            state,
            gpu_engine,
            helio_mailbox,
            debug_views,
            view_mode_picker,
            input_state,
            last_signature,
            pump_started: false,
            custom_snaps,
            snap_panels,
            _custom_snap_subscriptions: custom_snap_subscriptions,
            _view_mode_picker_subscription: view_mode_picker_subscription,
        }
    }

    pub(crate) fn view_mode_label(&self, cx: &App) -> SharedString {
        match self.view_mode_picker.read(cx).selected_value() {
            Some(ViewModeSelection::Debug(mode)) => self
                .debug_views
                .iter()
                .find(|view| view.debug_mode == *mode)
                .map(|view| view.name.into())
                .unwrap_or_else(|| "Lit".into()),
            _ => "Lit".into(),
        }
    }

    fn subscribe_view_mode_picker(
        state: &Arc<parking_lot::RwLock<LevelEditorState>>,
        picker: &Entity<ViewModePickerState>,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let state = state.clone();
        cx.subscribe(
            picker,
            move |_, _, event: &DropdownEvent<SearchableVec<ViewModeOption>>, _cx| {
                let DropdownEvent::Confirm(Some(selected_mode)) = event else {
                    return;
                };

                let mut state = state.write();
                match selected_mode {
                    ViewModeSelection::Lit => {
                        state.editor.viewport_debug_mode = 0;
                        state.editor.show_wireframe = false;
                        state.editor.show_lighting = true;
                    }
                    ViewModeSelection::Debug(debug_mode) => {
                        state.editor.viewport_debug_mode = *debug_mode;
                        state.editor.show_wireframe = false;
                        state.editor.show_lighting = true;
                    }
                }
            },
        )
    }

    /// The style the cached view lays itself out with. Must match the root style
    /// of what `ToolbarPanel::render` produces: on a cache hit GPUI lays the
    /// view out from this refinement alone, without consulting its content.
    pub fn cache_style() -> StyleRefinement {
        StyleRefinement::default().w_full().h(px(36.0))
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;

        spawn_frame_pump(&cx.entity(), window, |this, window, cx| {
            let _scope = gpui::render_stats::scope("toolbar: frame pump");
            let _lock_scope = gpui::render_stats::scope("toolbar: signature state read");
            let debug_views = this
                .helio_mailbox
                .as_ref()
                .map(HelioEditorMailbox::viewport_debug_views)
                .unwrap_or_default();
            let signature = ToolbarSignature::of(
                &this.state.read(),
                &debug_views,
                this.input_state.get_move_speed().to_bits(),
            );
            drop(_lock_scope);
            if signature != this.last_signature {
                gpui::render_stats::count("toolbar: signature changed");
                this.last_signature = signature;
                let debug_views_changed = debug_views != this.debug_views;
                this.debug_views = debug_views;
                if debug_views_changed {
                    let state = this.state.read();
                    let selected_view_mode =
                        ViewModeDropdown::selected(state.editor.viewport_debug_mode);
                    this.view_mode_picker = ViewModeDropdown::new_state(
                        window,
                        cx,
                        &this.debug_views,
                        selected_view_mode,
                    );
                    this._view_mode_picker_subscription =
                        Self::subscribe_view_mode_picker(&this.state, &this.view_mode_picker, cx);
                }
                cx.notify();
            }
        });
    }
}

impl EventEmitter<PanelEvent> for ToolbarView {}

impl Render for ToolbarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("toolbar: render");
        let _t = gpui::render_stats::scope("toolbar: render");

        self.start_pump(window, cx);

        // Record what we are about to paint so the pump doesn't immediately
        // notify again for a change we have already picked up (e.g. a render
        // triggered by an action handler rather than by the pump).
        let _signature_scope = gpui::render_stats::scope("toolbar: render signature");
        self.last_signature = ToolbarSignature::of(
            &self.state.read(),
            &self.debug_views,
            self.input_state.get_move_speed().to_bits(),
        );
        drop(_signature_scope);

        let _state_scope = gpui::render_stats::scope("toolbar: render state read");
        let state = self.state.read();
        drop(_state_scope);
        // The toolbar renders on every change to its state (see the pump), so
        // this keeps the viewport's Bloom, Realtime and buffer view in step with the
        // toggles, including their initial values.
        let _mailbox_scope = gpui::render_stats::scope("toolbar: mailbox update");
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.set_viewport_bloom(state.editor.feature_bloom_enabled);
            mailbox.set_viewport_realtime(state.editor.realtime_viewport);
            mailbox.set_viewport_debug_mode(state.editor.viewport_debug_mode);
        }
        drop(_mailbox_scope);
        let selected_mode = if state.editor.viewport_debug_mode == 0
            || self
                .debug_views
                .iter()
                .any(|view| view.debug_mode == state.editor.viewport_debug_mode)
        {
            ViewModeDropdown::selected(state.editor.viewport_debug_mode)
        } else {
            ViewModeSelection::Lit
        };
        if self.view_mode_picker.read(cx).selected_value().copied() != Some(selected_mode) {
            self.view_mode_picker.update(cx, |picker, cx| {
                picker.set_selected_value(&selected_mode, window, cx);
            });
        }
        let _panel_scope = gpui::render_stats::scope("toolbar: panel element build");
        self.toolbar
            .render(&state, self.state.clone(), self.gpu_engine.clone(), cx)
    }
}
