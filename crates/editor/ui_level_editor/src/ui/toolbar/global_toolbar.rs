//! The engine-global toolbar: playback, speed, multiplayer and build controls.
//!
//! These controls belong to the engine, not to any editor. They show and drive
//! `engine_state::playback` and know nothing about which editors are open: the
//! app shell mounts [`GlobalToolbarView`] under the menu bar, and whichever
//! editor is registered as a playback host services the Play / Stop / Pause /
//! Step commands it sends.
//!
//! A per-frame pump compares a [`PlaybackState`] snapshot and notifies only on
//! a real change.

use engine_state::playback::{PlaybackState, playback};
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
use crate::ui::frame_pump::spawn_frame_pump;

/// Height of the global toolbar row. Matches the title bar so the two stack
/// into one header.
pub const GLOBAL_TOOLBAR_HEIGHT: Pixels = px(34.);

pub struct GlobalToolbarView {
    focus_handle: FocusHandle,
    last: PlaybackState,
    pump_started: bool,
}

impl GlobalToolbarView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            last: playback().state(),
            pump_started: false,
        }
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let now = playback().state();
            if now != this.last {
                this.last = now;
                cx.notify();
            }
        });
    }

    fn set(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut PlaybackState)) {
        playback().update(f);
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for GlobalToolbarView {}

impl Render for GlobalToolbarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.start_pump(window, cx);
        let state = playback().state();
        self.last = state;

        let separator = |cx: &App| div().h_4().w_px().bg(cx.theme().border.opacity(0.4));
        let background = cx.theme().background;

        h_flex()
            .id("global-toolbar")
            .track_focus(&self.focus_handle)
            .w_full()
            .h(GLOBAL_TOOLBAR_HEIGHT)
            .px_2()
            .gap_2()
            .items_center()
            .bg(background)
            .on_action(cx.listener(|this, a: &SetTimeScale, _, cx| {
                this.set(cx, |s| s.time_scale = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetMultiplayerMode, _, cx| {
                this.set(cx, |s| s.multiplayer_mode = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetBuildConfig, _, cx| {
                this.set(cx, |s| s.build_config = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetTargetPlatform, _, cx| {
                this.set(cx, |s| s.target_platform = a.0)
            }))
            .on_action(cx.listener(|this, a: &SetBuildMode, _, cx| {
                this.set(cx, |s| s.build_mode = a.0)
            }))
            .child(PlaybackControls::render(&state))
            .child(separator(cx))
            .child(TimeScaleDropdown::render(&state, cx))
            .child(separator(cx))
            .child(MultiplayerDropdown::render(&state, cx))
            .child(separator(cx))
            .child(BuildDropdowns::render(&state, cx))
            .child(separator(cx))
            .child(BuildCoreButton::render(&state, cx))
    }
}
