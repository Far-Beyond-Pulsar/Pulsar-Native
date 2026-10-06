//! `ToolbarView` — the level editor toolbar as its own cached GPUI view.
//!
//! The toolbar used to be rendered inline by `LevelEditorPanel::render`, which
//! meant its whole element tree — playback controls, four dropdowns, feature
//! toggles, the Build Core split button and every `t!()` lookup behind them —
//! was rebuilt on every frame the panel was dirty. The panel is dirty on every
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
use engine_backend::subsystems::render::HelioEditorMailbox;
use gpui::*;
use ui::dock::PanelEvent;
use ui::input::{InputEvent, InputState, TextInput};

use super::snap_controls::{SnapKind, SnapPanel};
use super::ToolbarPanel;
use crate::state::EditorMode;
use crate::tool_modes::ToolModeId;
use crate::ui::frame_pump::spawn_frame_pump;
use crate::LevelEditorState;

/// Every piece of [`LevelEditorState`] the toolbar's element tree depends on.
///
/// Kept deliberately flat and `Copy`-ish so building one per frame is trivial
/// next to rebuilding the toolbar itself.
#[derive(Clone, Copy, PartialEq)]
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
    location_snap: u32,
    rotation_snap: u32,
    scale_snap: u32,
    // profiling button
    show_performance_overlay: bool,
}

impl ToolbarSignature {
    fn of(state: &LevelEditorState) -> Self {
        Self {
            tool_mode: state.editor.tool_mode_registry.selected_id(),
            editor_mode: state.scene.editor_mode,
            feature_lighting_enabled: state.editor.feature_lighting_enabled,
            feature_shadows_enabled: state.editor.feature_shadows_enabled,
            feature_bloom_enabled: state.editor.feature_bloom_enabled,
            feature_materials_enabled: state.editor.feature_materials_enabled,
            location_snap: state.editor.location_snap.to_bits(),
            rotation_snap: state.editor.rotation_snap.to_bits(),
            scale_snap: state.editor.scale_snap.to_bits(),
            show_performance_overlay: state.overlays.state.show_performance_overlay,
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
    last_signature: ToolbarSignature,
    pump_started: bool,
    custom_snaps: [Entity<InputState>; 3],
    snap_panels: [Entity<SnapPanel>; 3],
    _custom_snap_subscriptions: Vec<Subscription>,
}

impl ToolbarView {
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        gpu_engine: Arc<std::sync::Mutex<GpuRenderer>>,
        helio_mailbox: Option<HelioEditorMailbox>,
    ) -> Self {
        let last_signature = ToolbarSignature::of(&state.read());
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
                                    let (location, rotation, scale) = {
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
                                        )
                                    };
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
            last_signature,
            pump_started: false,
            custom_snaps,
            snap_panels,
            _custom_snap_subscriptions: custom_snap_subscriptions,
        }
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

        spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let _scope = gpui::render_stats::scope("toolbar: frame pump");
            let _lock_scope = gpui::render_stats::scope("toolbar: signature state read");
            let signature = ToolbarSignature::of(&this.state.read());
            drop(_lock_scope);
            if signature != this.last_signature {
                gpui::render_stats::count("toolbar: signature changed");
                this.last_signature = signature;
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
        self.last_signature = ToolbarSignature::of(&self.state.read());
        drop(_signature_scope);

        let _state_scope = gpui::render_stats::scope("toolbar: render state read");
        let state = self.state.read();
        drop(_state_scope);
        // The toolbar renders on every change to its state (see the pump), so
        // this keeps the viewport's Bloom in step with the toggle, including
        // its initial value.
        let _mailbox_scope = gpui::render_stats::scope("toolbar: mailbox update");
        if let Some(mailbox) = &self.helio_mailbox {
            mailbox.set_viewport_bloom(state.editor.feature_bloom_enabled);
        }
        drop(_mailbox_scope);
        let _panel_scope = gpui::render_stats::scope("toolbar: panel element build");
        self.toolbar.render(
            &state,
            self.state.clone(),
            self.gpu_engine.clone(),
            &self.snap_panels,
            cx,
        )
    }
}
