//! The engine-global toolbar: playback, speed, multiplayer and build controls.
//!
//! These controls belong to the engine rather than to any one editor, so the
//! app shell mounts [`GlobalToolbarView`] directly under the menu bar and every
//! editor tab shares it. The level editor still owns the state they drive
//! (`LevelEditorState::play` / `::build`), so a level editor registers itself
//! as the [`ActiveLevelEditor`] when it is created and this view operates on
//! whichever one registered last. With no level editor open the controls are
//! simply absent.
//!
//! Like [`super::ToolbarView`] the inputs live in an
//! `Arc<RwLock<LevelEditorState>>` GPUI cannot observe, so a per-frame pump
//! compares a [`GlobalToolbarSignature`] and notifies only on a real change.
//! **If you add a state read to a control rendered here, add the field to the
//! signature.**

use std::sync::Arc;

use engine_backend::services::gpu_renderer::GpuRenderer;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use ui::{ActiveTheme as _, dock::PanelEvent, h_flex};

use super::actions::{
    SetBuildConfig, SetBuildMode, SetMultiplayerMode, SetTargetPlatform, SetTimeScale,
};
use super::build::build_core::BuildCoreButton;
use super::build::build_dropdowns::BuildDropdowns;
use super::multiplayer_dropdown::MultiplayerDropdown;
use super::playback_controls::PlaybackControls;
use super::time_scale_dropdown::TimeScaleDropdown;
use crate::LevelEditorState;
use crate::state::{BuildConfig, BuildMode, EditorMode, MultiplayerMode, TargetPlatform};
use crate::ui::frame_pump::spawn_frame_pump;

/// Height of the global toolbar row. Matches the title bar so the two stack
/// into one header.
pub const GLOBAL_TOOLBAR_HEIGHT: Pixels = px(34.);

/// The level editor the global toolbar currently drives.
#[derive(Clone)]
pub struct ActiveLevelEditor {
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
}

impl Global for ActiveLevelEditor {}

/// Make `state` the level editor the global toolbar controls.
pub(crate) fn set_active_level_editor(
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    cx: &mut App,
) {
    cx.set_global(ActiveLevelEditor { state });
}

#[derive(Clone, Copy, PartialEq)]
struct GlobalToolbarSignature {
    editor_mode: EditorMode,
    pie_active: bool,
    pie_supports_control: bool,
    pie_paused: bool,
    time_scale: f32,
    multiplayer_mode: MultiplayerMode,
    build_config: BuildConfig,
    target_platform: TargetPlatform,
    build_mode: BuildMode,
    game_running: bool,
}

impl GlobalToolbarSignature {
    fn of(state: &LevelEditorState) -> Self {
        Self {
            editor_mode: state.scene.editor_mode,
            pie_active: state.play.pie.active,
            pie_supports_control: state.play.pie.supports_control,
            pie_paused: state.play.pie.paused,
            time_scale: state.play.time_scale,
            multiplayer_mode: state.play.multiplayer_mode,
            build_config: state.build.config,
            target_platform: state.build.target_platform,
            build_mode: state.build.mode,
            game_running: state.build.game_running,
        }
    }
}

/// What the last render was built from; any difference means a re-render.
#[derive(Clone, Copy, PartialEq)]
struct Snapshot {
    /// Identity of the active editor's state, so switching editors refreshes.
    editor: usize,
    signature: GlobalToolbarSignature,
}

pub struct GlobalToolbarView {
    focus_handle: FocusHandle,
    last: Option<Snapshot>,
    pump_started: bool,
}

impl GlobalToolbarView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            last: None,
            pump_started: false,
        }
    }

    /// Row height, for `AnyView::cached` style refinements.
    pub fn height() -> Pixels {
        GLOBAL_TOOLBAR_HEIGHT
    }

    fn snapshot(cx: &App) -> Option<Snapshot> {
        let active = cx.try_global::<ActiveLevelEditor>()?;
        Some(Snapshot {
            editor: Arc::as_ptr(&active.state) as usize,
            signature: GlobalToolbarSignature::of(&active.state.read()),
        })
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let snapshot = Self::snapshot(cx);
            if snapshot != this.last {
                this.last = snapshot;
                cx.notify();
            }
        });
    }

    fn with_state(
        &self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut LevelEditorState),
    ) {
        if let Some(active) = cx.try_global::<ActiveLevelEditor>() {
            f(&mut active.state.write());
        }
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for GlobalToolbarView {}

impl Render for GlobalToolbarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_pump(window, cx);
        self.last = Self::snapshot(cx);

        let active = cx.try_global::<ActiveLevelEditor>().cloned();
        let theme = cx.theme();
        let separator = |cx: &App| div().h_4().w_px().bg(cx.theme().border.opacity(0.4));

        h_flex()
            .id("global-toolbar")
            .track_focus(&self.focus_handle)
            .w_full()
            .h(GLOBAL_TOOLBAR_HEIGHT)
            .px_2()
            .gap_2()
            .items_center()
            .bg(theme.background)
            .on_action(cx.listener(|this, a: &SetTimeScale, _, cx| {
                this.with_state(cx, |s| s.play.time_scale = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetMultiplayerMode, _, cx| {
                this.with_state(cx, |s| s.play.multiplayer_mode = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetBuildConfig, _, cx| {
                this.with_state(cx, |s| s.build.config = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetTargetPlatform, _, cx| {
                this.with_state(cx, |s| s.build.target_platform = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetBuildMode, _, cx| {
                this.with_state(cx, |s| s.build.mode = a.0)
            }))
            .when_some(active, |el, active| {
                let state_arc = active.state.clone();
                let state = state_arc.read();
                el.child(PlaybackControls::render(&state, state_arc.clone(), cx))
                    .child(separator(cx))
                    .child(TimeScaleDropdown::render(&state, state_arc.clone(), cx))
                    .child(separator(cx))
                    .child(MultiplayerDropdown::render(&state, state_arc.clone(), cx))
                    .child(separator(cx))
                    .child(BuildDropdowns::render(&state, state_arc.clone(), cx))
                    .child(separator(cx))
                    .child(BuildCoreButton::render(&state, state_arc.clone(), cx))
            })
    }
}
