//! The engine-global toolbar: playback, speed, multiplayer and build controls.
//!
//! These controls belong to the engine, not to any editor. They show and drive
//! `engine_state::playback` and know nothing about which editors are open: the
//! app shell mounts [`GlobalToolbarView`] under the menu bar. Buttons publish
//! `pulsar_events::PlaybackCommand`s on the host bus; whichever editor hosts
//! play sessions acts on them and reports back through the state resource.
//!
//! The view re-renders when the `PlaybackState` or the build configurations
//! change (`ResourceHandle::changed`), not on a poll.
use engine_state::build_config::build_configurations;
use engine_state::playback::{playback, PlaybackState};
use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    dock::PanelEvent,
    h_flex,
    popover::Popover,
    ActiveTheme as _, Sizable as _,
};
use ui_build::BuildPicker;

use super::actions::SetTimeScale;
use super::multiplayer_panel::{summary, trigger_icon, MultiplayerPanel};
use super::playback_controls::PlaybackControls;
use super::time_scale_dropdown::TimeScaleDropdown;

/// Height of the global toolbar row. Matches the title bar so the two stack
/// into one header.
pub const GLOBAL_TOOLBAR_HEIGHT: Pixels = px(34.);

pub struct GlobalToolbarView {
    focus_handle: FocusHandle,
    multiplayer: Entity<MultiplayerPanel>,
    build_picker: Entity<BuildPicker>,
    /// Re-render this view when the playback state or the build
    /// configurations change; dropped (and so cancelled) with the view.
    _watches: Vec<Task<()>>,
}

/// A task that calls `cx.notify()` on the view each time `resource` changes.
fn watch<T: 'static>(
    resource: engine_state::ResourceHandle<T>,
    cx: &mut Context<GlobalToolbarView>,
) -> Task<()>
where
    T: Send + Sync,
{
    cx.spawn(async move |this, cx| {
        let mut seen = resource.version();
        loop {
            // Register before comparing so a change landing in between is
            // never missed.
            let changed = resource.changed();
            if resource.version() == seen {
                changed.await;
            }
            seen = resource.version();
            if this.update(cx, |_, cx| cx.notify()).is_err() {
                break;
            }
        }
    })
}

impl GlobalToolbarView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ui_build::init();
        let watches = vec![watch(playback(), cx), watch(build_configurations(), cx)];
        Self {
            focus_handle: cx.focus_handle(),
            multiplayer: cx.new(MultiplayerPanel::new),
            build_picker: cx.new(|cx| BuildPicker::new(window, cx)),
            _watches: watches,
        }
    }

    fn set(&mut self, f: impl FnOnce(&mut PlaybackState)) {
        playback().update(f);
    }
}

impl EventEmitter<PanelEvent> for GlobalToolbarView {}

impl Render for GlobalToolbarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = playback().get();
        let configs = build_configurations().get();

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
            .on_action(cx.listener(|this, a: &SetTimeScale, _, _| this.set(|s| s.time_scale = a.0)))
            .child(PlaybackControls::render(&state))
            .child(separator(cx))
            .child(TimeScaleDropdown::render(&state, cx))
            .child(separator(cx))
            .child({
                let panel = self.multiplayer.clone();
                Popover::<MultiplayerPanel>::new("multiplayer-popover")
                    .anchor(Corner::TopLeft)
                    .trigger(
                        Button::new("multiplayer-trigger")
                            .icon(trigger_icon(&state))
                            .label(summary(&state))
                            .small()
                            .ghost()
                            .tooltip("Multiplayer configuration"),
                    )
                    .content(move |_, _| panel.clone())
            })
            .child(div().flex_1())
            .child(ui_build::build_button(
                &state,
                &configs,
                &self.build_picker,
                cx,
            ))
    }
}
